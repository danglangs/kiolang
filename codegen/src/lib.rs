//! Codegen: an `ir::Module` to a component in the `compiled` world (caps.wit).
//!
//! The program is appended to runtime.wasm, a complete core module (docs/codegen.md,
//! "Pipeline"). Nothing is inserted before what the runtime already has, so no index moves:
//! new types, functions and data go after the runtime's, and the runtime's two placeholder
//! entry functions keep their indexes and get new bodies. wit-component then wraps the one
//! module as a component.

mod data;
mod moves;

use anyhow::{anyhow, Context, Result};
use data::Statics;
use lang::ir::{self, BinOp, Builtin, ExprKind, Module, Mut, StmtKind, Ty, UnOp};
use lang::Span;
use std::collections::{HashMap, HashSet};
use wasm_encoder::reencode::{self, Reencode};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ElementSection, Elements, Encode, Function, Ieee64, Instruction,
    InstructionSink, MemorySection, TableSection, TypeSection, ValType,
};
use wasmparser::{Parser, Payload};

const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runtime.wasm"));
const PAGE: u32 = 1 << 16;

/// A compiled program: the component, and what each `trap` site reports.
pub struct Wasm {
    pub component: Vec<u8>,
    pub sites: Vec<Site>,
    /// The linear memory the module starts with, in bytes: the runtime's and the program's data.
    pub memory: usize,
}

/// A runtime error the program can raise, by the number generated code passes to `trap`.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub span: Span,
    /// The message; the host replaces `{a}` and `{b}` with `trap`'s arguments, and `{c}` with
    /// the value a `trap` at the runtime's `EXTRA` site passed before it.
    pub template: &'static str,
}

impl Site {
    /// The message for a `trap(site, a, b)` here, after `trap(EXTRA, c, _)`.
    pub fn message(&self, a: i64, b: i64, c: i64) -> String {
        self.template
            .replace("{a}", &a.to_string())
            .replace("{b}", &b.to_string())
            .replace("{c}", &c.to_string())
    }
}

/// Compiles `m`. Panics with `unimplemented!("codegen: ..")` on IR that codegen does not handle
/// yet (none today: a new IR node gets such an arm until its compiler task).
pub fn compile(m: &Module) -> Result<Wasm> {
    let rt = Runtime::scan(RUNTIME)?;
    let data_addr = rt.pages * PAGE;
    let mut statics = Statics::new(m, data_addr, rt.table);
    let mut sites = Vec::new();
    let mut types: Vec<FuncSig> = Vec::new();
    let mut funcs = Vec::new();
    let lent = moves::borrowed(m, is_ptr);
    for (i, f) in m.funcs.iter().enumerate() {
        let (sig, body) = func(&rt, m, &mut statics, &mut types, f, &mut sites, (&lent, i));
        funcs.push((rt.types + type_index(&mut types, sig), body));
    }
    // A wrapper per function used as a value, after the program's functions, in table order.
    let mut elems = Vec::new();
    for &(f, captures) in statics.elems() {
        let (sig, body) = wrapper(&rt, m, f, captures, &lent[f.0 as usize]);
        elems.push(rt.funcs + funcs.len() as u32);
        funcs.push((rt.types + type_index(&mut types, sig), body));
    }
    let main = &m.funcs[m.main.0 as usize];
    let main_desc = entry_desc(&mut statics, &main.ret);
    let (data, table) = statics.finish();
    let main_index = rt.funcs + m.main.0;
    let mut app = Append {
        rt: &rt,
        types,
        funcs,
        entry: entry(&rt, table, main_index, main),
        entry_desc: main_desc,
        data: (data_addr, data),
        elems,
    };
    let memory = (rt.pages + app.extra_pages()) as usize * PAGE as usize;
    let mut module = wasm_encoder::Module::new();
    app.parse_core_module(&mut module, Parser::new(0), RUNTIME)
        .map_err(|e| anyhow!("re-encoding the runtime: {e}"))?;
    let core = module.finish();
    let component = wit_component::ComponentEncoder::default()
        .module(&core)?
        .validate(true)
        .encode()
        .context("wrapping the module as a component")?;
    Ok(Wasm {
        component,
        sites,
        memory,
    })
}

/// What codegen needs to know of runtime.wasm before appending to it.
struct Runtime {
    types: u32,
    /// Function count, imports included: the index of the first appended function.
    funcs: u32,
    imported: u32,
    pages: u32,
    /// The size of the runtime's table, the first entry the program's functions take.
    table: u32,
    exports: HashMap<String, u32>,
    /// The addresses of the allocator's free-list heads and of the live-object count, read
    /// from the bodies of `rt_heap_lists` and `rt_live_count` (`FuncGen::alloc`).
    lists: u32,
    live: u32,
}

impl Runtime {
    fn scan(wasm: &[u8]) -> Result<Runtime> {
        let mut rt = Runtime {
            types: 0,
            funcs: 0,
            imported: 0,
            pages: 0,
            table: 0,
            exports: HashMap::new(),
            lists: 0,
            live: 0,
        };
        let mut code = 0;
        for payload in Parser::new(0).parse_all(wasm) {
            match payload? {
                Payload::TypeSection(s) => rt.types = s.count(),
                Payload::ImportSection(s) => {
                    for i in s.into_imports() {
                        rt.imported += matches!(i?.ty, wasmparser::TypeRef::Func(_)) as u32;
                    }
                }
                Payload::FunctionSection(s) => rt.funcs = rt.imported + s.count(),
                Payload::MemorySection(s) => {
                    let m = s.into_iter().next().context("the runtime has no memory")??;
                    rt.pages = u32::try_from(m.initial)?;
                }
                Payload::TableSection(s) => {
                    let t = s.into_iter().next().context("the runtime has no table")??;
                    rt.table = u32::try_from(t.ty.initial)?;
                }
                Payload::ExportSection(s) => {
                    for e in s {
                        let e = e?;
                        rt.exports.insert(e.name.to_string(), e.index);
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let index = rt.imported + code;
                    code += 1;
                    for (name, at) in [("rt_heap_lists", &mut rt.lists), ("rt_live_count", &mut rt.live)] {
                        if rt.exports.get(name) == Some(&index) {
                            *at = constant(&body).with_context(|| format!("{name} is not one i32.const"))?;
                        }
                    }
                }
                _ => {}
            }
        }
        anyhow::ensure!(
            rt.lists != 0 && rt.live != 0,
            "the runtime has no rt_heap_lists or rt_live_count"
        );
        Ok(rt)
    }

    fn export(&self, name: &str) -> u32 {
        self.exports[name]
    }
}

/// The value of a function body that is exactly `i32.const N; end`, as an address.
fn constant(body: &wasmparser::FunctionBody) -> Option<u32> {
    let mut ops = body.get_operators_reader().ok()?;
    let wasmparser::Operator::I32Const { value } = ops.read().ok()? else {
        return None;
    };
    let wasmparser::Operator::End = ops.read().ok()? else {
        return None;
    };
    (ops.eof() && body.get_locals_reader().ok()?.get_count() == 0).then_some(value as u32)
}

/// A Wasm function type.
type FuncSig = (Vec<ValType>, Vec<ValType>);

/// The index of `sig` among the program's types, added if new.
fn type_index(types: &mut Vec<FuncSig>, sig: FuncSig) -> u32 {
    let i = types.iter().position(|t| *t == sig).unwrap_or_else(|| {
        types.push(sig);
        types.len() - 1
    });
    i as u32
}

/// The Wasm type of a call through a `fn(params) -> ret` value: the function object, then the
/// parameters. Every table entry of that `fn` type has it.
fn value_sig(params: &[Ty], ret: &Ty) -> FuncSig {
    let params = std::iter::once(ValType::I32).chain(params.iter().filter_map(val_type));
    (params.collect(), val_type(ret).into_iter().collect())
}

/// The value type of a kio type, if it has one: `()` and `!` have none.
fn val_type(t: &Ty) -> Option<ValType> {
    match t {
        Ty::Int => Some(ValType::I64),
        Ty::Bool => Some(ValType::I32),
        Ty::Float => Some(ValType::F64),
        Ty::Str | Ty::Bytes | Ty::List(_) | Ty::Map(..) | Ty::HashMap(..) => Some(ValType::I32),
        Ty::Struct(_) | Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) | Ty::Fn(..) => Some(ValType::I32),
        // A handle in the component's table, never counted.
        Ty::Cap(_) => Some(ValType::I32),
        Ty::Unit | Ty::Never => None,
    }
}

/// Whether a value of `t` is a pointer to a counted object, which every expression yields
/// owned (docs/codegen.md, "Ownership").
fn is_ptr(t: &Ty) -> bool {
    matches!(
        t,
        Ty::Str
            | Ty::Bytes
            | Ty::List(_)
            | Ty::Map(..)
            | Ty::HashMap(..)
            | Ty::Struct(_)
            | Ty::Enum(_)
            | Ty::Option(_)
            | Ty::Result(..)
            | Ty::Fn(..)
    )
}

/// Whether built-in `b` is a Wasm instruction or two on a scalar receiver (`scalar_builtin`).
fn inline_builtin(b: Builtin) -> bool {
    matches!(
        b,
        Builtin::IntToFloat
            | Builtin::FloatAbs
            | Builtin::FloatFloor
            | Builtin::FloatCeil
            | Builtin::FloatSqrt
            | Builtin::FloatIsNan
            | Builtin::FloatIsFinite
            | Builtin::FloatIsInfinite
    )
}

/// Whether built-in `b` takes a function value (`FuncGen::with_fn`).
fn takes_fn(b: Builtin) -> bool {
    matches!(
        b,
        Builtin::OptionMap
            | Builtin::OptionAndThen
            | Builtin::ResultMap
            | Builtin::ResultMapErr
            | Builtin::ResultAndThen
            | Builtin::ListMap
            | Builtin::ListFilter
            | Builtin::ListAny
            | Builtin::ListAll
    )
}

/// The payload types of variant `v` of enum type `t` (a user enum, `Option` or `Result`).
fn payload(m: &Module, t: &Ty, v: u32) -> Vec<Ty> {
    match t {
        Ty::Enum(id) => m.enums[id.0 as usize].variants[v as usize].1.clone(),
        Ty::Option(x) if v == 1 => vec![(**x).clone()],
        Ty::Option(_) => vec![],
        Ty::Result(x, _) if v == 0 => vec![(**x).clone()],
        Ty::Result(_, e) => vec![(**e).clone()],
        t => unreachable!("{t:?} is not an enum"),
    }
}

/// Byte offsets of the first slot in a struct and in an enum variant (runtime/src/obj.rs), and
/// of an enum's variant number.
const RECORD_SLOTS: u64 = 8;
const ENUM_SLOTS: u64 = 16;
const VARIANT: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 8,
    align: 2,
    memory_index: 0,
};

/// The largest block the runtime's free lists hold (runtime/src/heap.rs `SMALL`): classes of
/// 8, 16, .., 64 bytes, class `(size - 1) / 8`.
const HEAP_SMALL: u64 = 64;

/// A 4-byte word at byte `offset`.
fn mem32(offset: u64) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }
}

/// The slot at byte `offset` of an object.
fn slot_at(offset: u64) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset,
        align: 3,
        memory_index: 0,
    }
}

/// The element type of a `List`.
fn elem(t: &Ty) -> &Ty {
    match t {
        Ty::List(e) => e,
        t => unreachable!("{t:?} is not a List"),
    }
}

/// An 8-byte slot in linear memory.
const SLOT: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 0,
    align: 3,
    memory_index: 0,
};

/// `WsMessage` as `ws_message`: a capability's part of its wrappers' names.
/// The local at the root of `e`, a local or a field of one (`FuncGen::in_place`).
fn root(e: &ir::Expr) -> ir::LocalId {
    match &e.kind {
        ExprKind::Local(id) => *id,
        ExprKind::Field(base, _) => root(base),
        _ => unreachable!("in place"),
    }
}

fn snake(name: &str) -> String {
    let mut s = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            s.push('_');
        }
        s.push(c.to_ascii_lowercase());
    }
    s
}

/// Turns a slot on the stack into a value of type `t`.
fn slot_out(s: &mut InstructionSink, t: &Ty) {
    match val_type(t) {
        Some(ValType::I64) => {}
        Some(ValType::F64) => {
            s.f64_reinterpret_i64();
        }
        Some(_) => {
            s.i32_wrap_i64();
        }
        None => {
            s.drop();
        }
    }
}

/// For `x = value`, if `value` is `x + a + b ..` (concatenations nested to the left, ending in
/// a read of `x`): the read of `x`, then `a`, `b`, .. in evaluation order.
fn appended(x: ir::LocalId, value: &ir::Expr) -> Option<Vec<&ir::Expr>> {
    let mut parts = Vec::new();
    let mut e = value;
    while let ExprKind::Binary(BinOp::Concat, a, b) = &e.kind {
        parts.push(&**b);
        e = a;
    }
    if parts.is_empty() || e.kind != ExprKind::Local(x) {
        return None;
    }
    parts.push(e);
    parts.reverse();
    Some(parts)
}

/// A `List` object's `ListBody` (runtime/src/obj.rs): its elements' address, `len` and `cap`.
const LIST_ELEMS: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 8,
    align: 2,
    memory_index: 0,
};
const LIST_LEN: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 12,
    align: 2,
    memory_index: 0,
};
const LIST_CAP: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 16,
    align: 2,
    memory_index: 0,
};

/// A `Str`'s or `Bytes`' length, capacity and first byte (runtime/src/obj.rs).
const BYTES_LEN: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 8,
    align: 2,
    memory_index: 0,
};
const BYTES_CAP: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 12,
    align: 2,
    memory_index: 0,
};
const BYTES_DATA: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 16,
    align: 0,
    memory_index: 0,
};

/// An object's reference count, at byte 0.
const RC: wasm_encoder::MemArg = wasm_encoder::MemArg {
    offset: 0,
    align: 2,
    memory_index: 0,
};
/// Counts below this take the inline paths of `inc` and `dec`. Any other count, a pinned
/// object's (`u32::MAX`), or one a test build's runtime checks (0, poisoned), goes to the
/// runtime, so the inline paths change only counts the runtime would change the same way.
const FAST_RC: u32 = 0x8000_0000;

/// `rt_inc` of the pointer in local `p`, inline: `r` is a scratch `i32` local. A count in
/// `1..FAST_RC` gains one here; a pinned object is skipped; anything else calls the runtime.
fn inc(s: &mut InstructionSink, rt: &Runtime, p: u32, r: u32) {
    s.local_get(p).if_(BlockType::Empty);
    s.local_get(p)
        .i32_load(RC)
        .local_tee(r)
        .i32_const(1)
        .i32_sub()
        .i32_const((FAST_RC - 1) as i32)
        .i32_lt_u();
    s.if_(BlockType::Empty)
        .local_get(p)
        .local_get(r)
        .i32_const(1)
        .i32_add()
        .i32_store(RC);
    s.else_().local_get(r).i32_const(-1).i32_ne().if_(BlockType::Empty);
    s.local_get(p).call(rt.export("rt_inc")).end().end().end();
}

/// `rt_dec` of the pointer in local `p`, inline as `inc`: a count in `2..FAST_RC` loses one
/// here; a pinned object is skipped; the last reference (count 1, which frees) and anything
/// else call the runtime.
fn dec(s: &mut InstructionSink, rt: &Runtime, p: u32, r: u32) {
    s.local_get(p).if_(BlockType::Empty);
    s.local_get(p)
        .i32_load(RC)
        .local_tee(r)
        .i32_const(2)
        .i32_sub()
        .i32_const((FAST_RC - 2) as i32)
        .i32_lt_u();
    s.if_(BlockType::Empty)
        .local_get(p)
        .local_get(r)
        .i32_const(1)
        .i32_sub()
        .i32_store(RC);
    s.else_().local_get(r).i32_const(-1).i32_ne().if_(BlockType::Empty);
    s.local_get(p).call(rt.export("rt_dec")).end().end().end();
}

fn func<'m>(
    rt: &Runtime,
    m: &'m Module,
    statics: &mut Statics<'m>,
    types: &mut Vec<FuncSig>,
    f: &'m ir::Func,
    sites: &mut Vec<Site>,
    (lent, index): (&[Vec<bool>], usize),
) -> (FuncSig, Function) {
    // Each IR local with a value gets a Wasm local: parameters first, as Wasm numbers them,
    // then the others; `()` locals get none. Hidden locals follow.
    let mut params = Vec::new();
    let mut decls = Vec::new();
    let mut locals = Vec::new();
    for (i, l) in f.locals.iter().enumerate() {
        let index = (params.len() + decls.len()) as u32;
        let vt = val_type(&l.ty);
        locals.push(vt.map(|_| index));
        match vt {
            Some(t) if i < f.params as usize => params.push(t),
            Some(t) => decls.push(t),
            None => {}
        }
    }
    // The owned parameters are the call's, as if bound by `let` around the body; the borrowed
    // ones the caller's (C8b4).
    let owned = (0..f.params as usize)
        .filter(|&i| is_ptr(&f.locals[i].ty) && !lent[index][i])
        .filter_map(|i| locals[i])
        .collect();
    let mut g = FuncGen {
        rt,
        m,
        f,
        ret: &f.ret,
        statics,
        types,
        sites,
        locals,
        first_decl: params.len() as u32,
        decls,
        live: Vec::new(),
        free: Vec::new(),
        scratch: None,
        rc_scratch: None,
        list_scratch: None,
        owned,
        moves: moves::moves(f, is_ptr),
        lent,
        own_lent: &lent[index],
        depth: 0,
        loops: Vec::new(),
        code: Vec::new(),
    };
    let ret = val_type(&f.ret);
    g.block(&f.body, ret);
    g.release(0);
    g.sink().end();
    let mut body = Function::new_with_locals_types(g.decls);
    body.raw(g.code);
    ((params, ret.into_iter().collect()), body)
}

struct FuncGen<'a, 'm> {
    rt: &'a Runtime,
    m: &'m Module,
    f: &'m ir::Func,
    /// The function's return type, which `?` returns a `None` or `Err` of.
    ret: &'m Ty,
    statics: &'a mut Statics<'m>,
    /// The program's function types, which a call through a `fn` value adds to.
    types: &'a mut Vec<FuncSig>,
    sites: &'a mut Vec<Site>,
    /// The Wasm local of each IR local, `None` for those without a value.
    locals: Vec<Option<u32>>,
    /// Wasm locals after the parameters: the IR's, then hidden ones.
    decls: Vec<ValType>,
    first_decl: u32,
    /// Hidden locals in use, in the order taken. Every one taken while an expression or a
    /// statement is generated is dead when it ends (its value is on the stack or consumed), so
    /// it then goes to `free`, to be taken again: a function with many calls stays under Wasm's
    /// 50,000 locals.
    live: Vec<u32>,
    free: Vec<u32>,
    /// Three `i64` locals an integer check uses, after its operands are on the stack.
    scratch: Option<[u32; 3]>,
    /// Two `i32` locals for the inline `inc`, `dec` and `unique`: a pointer and its count.
    rc_scratch: Option<[u32; 2]>,
    /// Three for the inline list operations: a list (`i32`), a length (`i32`), a slot (`i64`).
    list_scratch: Option<[u32; 3]>,
    /// The Wasm locals that hold an owned pointer at this point of the code, outermost scope
    /// first: parameters, then each `let` once it has run, and hidden locals holding the
    /// operands evaluated so far. A scope's end releases its part; an exit releases the part
    /// of every scope it leaves.
    owned: Vec<u32>,
    /// The reads of pointer locals that are last uses (moves.rs): each hands over the local's
    /// reference and zeroes its slot, rather than adding a reference.
    moves: HashSet<*const ir::Expr>,
    /// Each function's borrowed parameters (moves.rs `borrowed`), and this function's.
    lent: &'a [Vec<bool>],
    own_lent: &'a [bool],
    /// Open Wasm blocks, loops and ifs that can hold kio code.
    depth: u32,
    /// For each enclosing kio loop, innermost last: the depth of its `break` and `continue`
    /// targets, and the length of `owned` outside it.
    loops: Vec<(u32, u32, usize)>,
    code: Vec<u8>,
}

/// What a runtime operation takes before its operands: nothing; its result's descriptor (a
/// `List<Str>`, `List<K>`, `Result`), or for an `Option` result also its pinned `None`, which it
/// returns rather than allocating one.
#[derive(Clone, Copy)]
enum Pre {
    Nothing,
    Desc,
    Opt,
}

#[derive(Clone, Copy)]
enum Open {
    Block,
    Loop,
    If,
}

/// An error message template; `{a}` and `{b}` are `trap`'s arguments.
const OVERFLOW: &str = "Int overflow";
const DIV_ZERO: &str = "division by zero";
/// The `Str` descriptor's index (runtime/src/desc.rs).
const STR_DESC: u32 = 3;
const OUT_OF_BOUNDS: &str = "index {a} out of bounds for length {b}";
/// `{c}` is the length, which the runtime passes through its `EXTRA` site.
const SLICE_BOUNDS: &str = "slice {a}..{b} out of bounds for length {c}";

impl FuncGen<'_, '_> {
    fn sink(&mut self) -> InstructionSink<'_> {
        InstructionSink::new(&mut self.code)
    }

    /// Calls runtime export `name`.
    fn call_rt(&mut self, name: &str) {
        match name {
            "rt_list_len" => self.list_len(),
            "rt_list_push" => self.list_push(),
            // A `Str`'s or `Bytes`' length is its header's `len` (C8b2).
            "rt_str_len" | "rt_bytes_len" => {
                self.sink().i32_load(BYTES_LEN).i64_extend_i32_u();
            }
            _ => {
                let f = self.rt.export(name);
                self.sink().call(f);
            }
        }
    }

    /// `rt_list_len`, inline: the list on the stack becomes its length (`ListBody`'s `len`).
    fn list_len(&mut self) {
        self.sink().i32_load(LIST_LEN).i64_extend_i32_u();
    }

    /// `rt_list_push` of the unique list and the slot on the stack, inline when the list has
    /// room: the slot is stored after the last element and `len` grows by one, as `Vec::push`
    /// does without growing. Otherwise the runtime pushes (and grows the buffer).
    fn list_push(&mut self) {
        let [l, n, v] = self.list_scratch();
        let f = self.rt.export("rt_list_push");
        let mut s = self.sink();
        s.local_set(v).local_set(l);
        s.local_get(l)
            .i32_load(LIST_LEN)
            .local_tee(n)
            .local_get(l)
            .i32_load(LIST_CAP)
            .i32_lt_u();
        s.if_(BlockType::Empty);
        s.local_get(l)
            .i32_load(LIST_ELEMS)
            .local_get(n)
            .i32_const(3)
            .i32_shl()
            .i32_add();
        s.local_get(v).i64_store(SLOT);
        s.local_get(l).local_get(n).i32_const(1).i32_add().i32_store(LIST_LEN);
        s.else_().local_get(l).local_get(v).call(f).end();
    }

    /// `rt_list_get` of list `l` at index `i` (locals), inline when `i` is in bounds: pushes the
    /// element as a slot, +1 if `t`, the element type, is a pointer. Out of bounds, the runtime
    /// reports the error at `site`.
    fn list_get(&mut self, l: u32, i: u32, site: u32, t: &Ty) {
        let f = self.rt.export("rt_list_get");
        let mut s = self.sink();
        s.local_get(i)
            .local_get(l)
            .i32_load(LIST_LEN)
            .i64_extend_i32_u()
            .i64_lt_u();
        s.if_(BlockType::Result(ValType::I64));
        s.local_get(l)
            .i32_load(LIST_ELEMS)
            .local_get(i)
            .i32_wrap_i64()
            .i32_const(3)
            .i32_shl()
            .i32_add();
        s.i64_load(SLOT);
        if is_ptr(t) {
            let [_, _, v] = self.list_scratch();
            self.sink().local_tee(v).i32_wrap_i64();
            self.inc_top();
            self.sink().local_get(v);
        }
        let mut s = self.sink();
        s.else_().local_get(l).local_get(i).i32_const(site as i32).call(f).end();
    }

    /// `rt_bytes_get` of `Bytes` `b` at index `i` (locals), inline when `i` is in bounds: pushes
    /// the byte. Out of bounds, the runtime reports the error at `site`.
    fn bytes_get(&mut self, b: u32, i: u32, site: u32) {
        let f = self.rt.export("rt_bytes_get");
        let mut s = self.sink();
        s.local_get(i)
            .local_get(b)
            .i32_load(BYTES_LEN)
            .i64_extend_i32_u()
            .i64_lt_u();
        s.if_(BlockType::Result(ValType::I64));
        s.local_get(b)
            .local_get(i)
            .i32_wrap_i64()
            .i32_add()
            .i64_load8_u(BYTES_DATA);
        s.else_().local_get(b).local_get(i).i32_const(site as i32).call(f).end();
    }

    /// `rt_bytes_push` of the unique `Bytes` and the `Int` on the stack, inline when the `Int` is
    /// a byte and there is room, as `list_push`: the byte is stored after the last and `len`
    /// grows by one. Otherwise the runtime pushes (growing) or reports the error at `site`.
    /// Leaves the object to store back.
    fn bytes_push(&mut self, site: u32) {
        let [l, n, v] = self.list_scratch();
        let f = self.rt.export("rt_bytes_push");
        let mut s = self.sink();
        s.local_set(v).local_set(l);
        s.local_get(v).i64_const(255).i64_le_u();
        s.local_get(l)
            .i32_load(BYTES_LEN)
            .local_tee(n)
            .local_get(l)
            .i32_load(BYTES_CAP)
            .i32_lt_u()
            .i32_and();
        s.if_(BlockType::Result(ValType::I32));
        s.local_get(l)
            .local_get(n)
            .i32_add()
            .local_get(v)
            .i64_store8(BYTES_DATA);
        s.local_get(l).local_get(n).i32_const(1).i32_add().i32_store(BYTES_LEN);
        s.local_get(l);
        s.else_().local_get(l).local_get(v).i32_const(site as i32).call(f).end();
    }

    /// Three locals for the inline list operations: a list (`i32`), a length (`i32`) and a
    /// slot (`i64`). As `rc_scratch`, they hold no kio code.
    fn list_scratch(&mut self) -> [u32; 3] {
        if self.list_scratch.is_none() {
            let (l, n, v) = (
                self.new_local(ValType::I32),
                self.new_local(ValType::I32),
                self.new_local(ValType::I64),
            );
            self.list_scratch = Some([l, n, v]);
        }
        self.list_scratch.unwrap()
    }

    /// The pointer and count locals of the inline reference counting. The code using them
    /// holds no kio code, so one pair serves the whole function.
    fn rc_scratch(&mut self) -> [u32; 2] {
        if self.rc_scratch.is_none() {
            self.rc_scratch = Some([0; 2].map(|_| self.new_local(ValType::I32)));
        }
        self.rc_scratch.unwrap()
    }

    /// Drops the reference in local `l`.
    fn dec(&mut self, l: u32) {
        let [_, r] = self.rc_scratch();
        let rt = self.rt;
        dec(&mut self.sink(), rt, l, r);
    }

    /// Adds a reference to the object in local `l`.
    fn inc(&mut self, l: u32) {
        let [_, r] = self.rc_scratch();
        let rt = self.rt;
        inc(&mut self.sink(), rt, l, r);
    }

    /// `rt_inc` of the pointer on the stack, which it pops.
    fn inc_top(&mut self) {
        let [p, _] = self.rc_scratch();
        self.sink().local_set(p);
        self.inc(p);
    }

    /// `rt_dec` of the pointer on the stack, which it pops.
    fn dec_top(&mut self) {
        let [p, _] = self.rc_scratch();
        self.sink().local_set(p);
        self.dec(p);
    }

    /// `rt_unique` of the pointer on the stack (not 0), inline when its count is 1.
    fn unique_top(&mut self) {
        let [p, _] = self.rc_scratch();
        let f = self.rt.export("rt_unique");
        let mut s = self.sink();
        s.local_tee(p).i32_load(RC).i32_const(1).i32_eq();
        s.if_(BlockType::Result(ValType::I32)).local_get(p);
        s.else_().local_get(p).call(f).end();
    }

    /// Drops the references in `owned[mark..]`, for an exit that leaves those scopes. Code
    /// generation goes on in them, so they stay in `owned`.
    fn release(&mut self, mark: usize) {
        for i in (mark..self.owned.len()).rev() {
            self.dec(self.owned[i]);
        }
    }

    /// Ends the scopes holding `owned[mark..]`: drops their references and zeroes their locals.
    fn end_scope(&mut self, mark: usize) {
        for l in self.owned.split_off(mark).into_iter().rev() {
            self.dec(l);
            self.sink().i32_const(0).local_set(l);
        }
    }

    /// Evaluates `xs` in order, each into a new hidden local, and returns them (`None` for
    /// `()` and `!`). An owned pointer among them is in `owned` from the returned mark on, so
    /// that an exit while a later one runs releases it; the caller truncates `owned` to the
    /// mark once it has used them.
    fn operands(&mut self, xs: &[&ir::Expr]) -> (usize, Vec<Option<u32>>) {
        let (mark, temps, _) = self.operands_borrowing(xs, |_| false);
        (mark, temps)
    }

    /// `operands`, except that an operand `i` for which `borrow(i)` holds and which is a read
    /// of a pointer local that no later operand mentions is not evaluated: its hidden local is
    /// the local itself, borrowed (C8b2). The third result flags those, which the caller must
    /// not drop and may use only until it runs code other than these operands. Nothing evaluated
    /// after a borrowed read can then change or move the local: only an assignment, a place
    /// write or a read in a later operand's own code could (a call cannot reach the caller's
    /// locals), and none mentions it.
    fn operands_borrowing(
        &mut self,
        xs: &[&ir::Expr],
        borrow: impl Fn(usize) -> bool,
    ) -> (usize, Vec<Option<u32>>, Vec<bool>) {
        let mark = self.owned.len();
        let mut temps = Vec::new();
        let mut borrowed = Vec::new();
        for (i, x) in xs.iter().enumerate() {
            if let ExprKind::Local(id) = x.kind {
                let later = || xs[i + 1..].iter().any(|y| moves::mentions(y, id));
                if let (true, Some(l)) = (is_ptr(&x.ty) && borrow(i), self.local(id)) {
                    if !later() {
                        temps.push(Some(l));
                        borrowed.push(true);
                        continue;
                    }
                }
            }
            self.expr(x);
            let t = val_type(&x.ty).map(|vt| self.temp(vt));
            if let Some(t) = t {
                self.sink().local_set(t);
                if is_ptr(&x.ty) {
                    self.owned.push(t);
                }
            }
            temps.push(t);
            borrowed.push(false);
        }
        (mark, temps, borrowed)
    }

    /// Pushes operand `x` from its hidden local `t`. An operand of type `!` has none, and
    /// the code after it is unreachable: `unreachable` keeps the stack's types valid.
    fn get(&mut self, x: &ir::Expr, t: Option<u32>) {
        match t {
            Some(t) => {
                self.sink().local_get(t);
            }
            None if x.ty == Ty::Never => {
                self.sink().unreachable();
            }
            None => {}
        }
    }

    /// Evaluates `xs`, then runs `f` to use them from their hidden locals without taking
    /// them, then drops the pointers among them: a runtime operation borrows its arguments.
    /// A local operand is borrowed (`operands_borrowing`), so neither copied nor dropped.
    fn borrowing(&mut self, xs: &[&ir::Expr], f: impl FnOnce(&mut Self, &[Option<u32>])) {
        let (mark, temps, borrowed) = self.operands_borrowing(xs, |_| true);
        f(self, &temps);
        self.drop_operands(xs, &temps, &borrowed);
        self.owned.truncate(mark);
    }

    /// Drops the pointer operands among `xs` that were evaluated, not borrowed.
    fn drop_operands(&mut self, xs: &[&ir::Expr], temps: &[Option<u32>], borrowed: &[bool]) {
        for ((x, t), b) in xs.iter().zip(temps).zip(borrowed) {
            if let (true, false, Some(t)) = (is_ptr(&x.ty), *b, t) {
                self.dec(*t);
            }
        }
    }

    /// Calls runtime export `name` on `xs`, which it borrows, with `site` last if it can fail.
    fn rt_op(&mut self, name: &str, xs: &[&ir::Expr], site: Option<u32>) {
        self.rt_op_slots(name, &[], xs, site.as_slice(), false)
    }

    /// The constants a runtime operation with result type `t` takes first.
    fn pre(&mut self, pre: Pre, t: &Ty) -> Vec<u32> {
        match pre {
            Pre::Nothing => vec![],
            Pre::Desc => vec![self.statics.desc(t)],
            Pre::Opt => vec![self.statics.desc(t), self.statics.pinned(t, 0)],
        }
    }

    /// `rt_op` with constants `pre` first (the result's descriptor, its pinned `None`), and the
    /// operands after the first as slots if `slots`: a map key, a list element, the default of
    /// `unwrap_or`, the error of `ok_or`. An operation with two kinds of error takes two sites.
    fn rt_op_slots(&mut self, name: &str, pre: &[u32], xs: &[&ir::Expr], sites: &[u32], slots: bool) {
        self.borrowing(xs, |g, temps| {
            for &c in pre {
                g.sink().i32_const(c as i32);
            }
            for (i, (x, t)) in xs.iter().zip(temps).enumerate() {
                g.get(x, *t);
                if slots && i > 0 {
                    g.slot_in(&x.ty);
                }
            }
            for &s in sites {
                g.sink().i32_const(s as i32);
            }
            g.call_rt(name);
        });
    }

    /// Turns a value of type `t` on the stack into a slot (`i64`): `Int` as is, `Float` as its
    /// bits, `Bool` and pointers zero-extended, `()` as 0.
    fn slot_in(&mut self, t: &Ty) {
        match val_type(t) {
            Some(ValType::I64) => {}
            Some(ValType::F64) => {
                self.sink().i64_reinterpret_f64();
            }
            Some(_) => {
                self.sink().i64_extend_i32_u();
            }
            None => {
                self.sink().i64_const(0);
            }
        }
    }

    /// Turns a slot on the stack into a value of type `t`.
    fn slot_out(&mut self, t: &Ty) {
        slot_out(&mut self.sink(), t);
    }

    /// A hidden local of type `t`, free until the expression or statement being generated ends.
    /// Nothing reads it before setting it.
    fn temp(&mut self, t: ValType) -> u32 {
        let l = match self
            .free
            .iter()
            .rposition(|&l| self.decls[(l - self.first_decl) as usize] == t)
        {
            Some(i) => self.free.swap_remove(i),
            None => self.new_local(t),
        };
        self.live.push(l);
        l
    }

    fn new_local(&mut self, t: ValType) -> u32 {
        self.decls.push(t);
        self.first_decl + self.decls.len() as u32 - 1
    }

    /// Frees the hidden locals taken since `live` had length `mark`.
    fn free_since(&mut self, mark: usize) {
        for l in self.live.drain(mark..) {
            debug_assert!(!self.owned.contains(&l), "a freed hidden local is still owned");
            self.free.push(l);
        }
    }

    fn scratch(&mut self) -> [u32; 3] {
        if self.scratch.is_none() {
            // Shared by every integer check, so never freed.
            self.scratch = Some([0; 3].map(|_| self.new_local(ValType::I64)));
        }
        self.scratch.unwrap()
    }

    fn site(&mut self, span: Span, template: &'static str) -> u32 {
        self.sites.push(Site { span, template });
        self.sites.len() as u32 - 1
    }

    fn local(&self, id: ir::LocalId) -> Option<u32> {
        self.locals[id.0 as usize]
    }

    /// Whether local `id` is a borrowed parameter: it holds the caller's reference, so it is
    /// never moved, assigned, changed in place or dropped here.
    fn is_lent(&self, id: ir::LocalId) -> bool {
        self.own_lent.get(id.0 as usize) == Some(&true)
    }

    /// Whether the read `e` of a local is a last use that moves its reference (moves.rs). A
    /// borrowed parameter's reference is not this function's to move.
    fn moves_out(&self, e: &ir::Expr) -> bool {
        match e.kind {
            ExprKind::Local(id) => self.moves.contains(&(e as *const ir::Expr)) && !self.is_lent(id),
            _ => false,
        }
    }

    /// Opens a Wasm `block`, `loop` or `if` (which takes its condition from the stack); returns
    /// its depth, for `br`.
    fn open(&mut self, kind: Open, bt: BlockType) -> u32 {
        let mut s = self.sink();
        match kind {
            Open::Block => s.block(bt),
            Open::Loop => s.loop_(bt),
            Open::If => s.if_(bt),
        };
        self.depth += 1;
        self.depth
    }

    fn close(&mut self) {
        self.sink().end();
        self.depth -= 1;
    }

    /// A branch to the Wasm block opened at depth `level`.
    fn br(&mut self, level: u32) {
        let rel = self.depth - level;
        self.sink().br(rel);
    }

    /// `b`'s statements and tail, leaving a value of type `want` on the stack. A block without
    /// a tail where a value is wanted diverges.
    fn block(&mut self, b: &ir::Block, want: Option<ValType>) {
        let mark = self.owned.len();
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(e) = &b.tail {
            self.expr(e);
        }
        self.end_scope(mark);
        if b.tail.is_none() && want.is_some() {
            self.sink().unreachable();
        }
    }

    fn stmt(&mut self, s: &ir::Stmt) {
        let mark = self.live.len();
        self.stmt_kind(s);
        self.free_since(mark);
    }

    fn stmt_kind(&mut self, s: &ir::Stmt) {
        match &s.kind {
            StmtKind::Let(id, init) => {
                self.expr(init);
                if let Some(l) = self.local(*id) {
                    self.sink().local_set(l);
                    if is_ptr(&init.ty) {
                        self.owned.push(l);
                    }
                }
            }
            StmtKind::Assign(place, op, value) if !place.steps.is_empty() => {
                self.assign_element(place, *op, value, s.span)
            }
            StmtKind::Assign(place, op, value) => {
                let Some(l) = self.local(place.root) else {
                    return self.expr(value);
                };
                debug_assert!(!self.is_lent(place.root), "a borrowed parameter assigned");
                match op {
                    None => match appended(place.root, value) {
                        Some(parts) => self.append(place.root, l, parts),
                        // The value first, then the old one dropped.
                        None => {
                            self.expr(value);
                            if is_ptr(&value.ty) {
                                self.dec(l);
                            }
                        }
                    },
                    Some(BinOp::Concat) => {
                        // `x + value`: the value first, then `rt_str_append` takes the old `x`
                        // (growing it in place if `x` held its only reference) and borrows the
                        // value, which is then dropped. A read of another local is borrowed
                        // instead (C8b2); `x += x` reads `x` +1, so `x` is copied.
                        let y = match value.kind {
                            ExprKind::Local(y) if y != place.root => self.local(y),
                            _ => None,
                        };
                        let v = y.unwrap_or_else(|| self.temp(ValType::I32));
                        if y.is_none() {
                            self.expr(value);
                            self.sink().local_set(v);
                        }
                        self.sink().local_get(l).local_get(v);
                        self.call_rt("rt_str_append");
                        if y.is_none() {
                            self.dec(v);
                        }
                    }
                    Some(op @ (BinOp::FAdd | BinOp::FSub)) => {
                        let v = self.temp(ValType::F64);
                        self.expr(value);
                        self.sink().local_set(v).local_get(l).local_get(v);
                        self.float_op(*op);
                    }
                    Some(op) => {
                        // The value comes first, as in the interpreter, then the local's value.
                        self.expr(value);
                        let v = self.scratch()[0];
                        self.sink().local_set(v).local_get(l).local_get(v);
                        self.int_op(*op, s.span);
                    }
                }
                self.sink().local_set(l);
            }
            StmtKind::Return(value) => {
                if let Some(e) = value {
                    self.expr(e);
                }
                self.release(0);
                self.sink().return_();
            }
            StmtKind::While(cond, body) => {
                let brk = self.open(Open::Block, BlockType::Empty);
                let cont = self.open(Open::Loop, BlockType::Empty);
                self.expr(cond);
                self.sink().i32_eqz();
                self.br_if(brk);
                self.loop_body(body, brk, cont);
            }
            StmtKind::Loop(body) => {
                let brk = self.open(Open::Block, BlockType::Empty);
                let cont = self.open(Open::Loop, BlockType::Empty);
                self.loop_body(body, brk, cont);
            }
            StmtKind::ForRange(i, a, b, body) => {
                // Both bounds once, before the first pass; `i` is not `mut`, so it counts.
                let i = self.local(*i).expect("an Int local");
                let end = self.temp(ValType::I64);
                self.expr(a);
                self.expr(b);
                self.sink().local_set(end).local_set(i);
                let brk = self.open(Open::Block, BlockType::Empty);
                let top = self.open(Open::Loop, BlockType::Empty);
                self.sink().local_get(i).local_get(end).i64_ge_s();
                self.br_if(brk);
                let cont = self.open(Open::Block, BlockType::Empty);
                self.loops.push((brk, cont, self.owned.len()));
                self.block(body, None);
                self.loops.pop();
                self.close();
                // Cannot overflow: `i < end`.
                self.sink().local_get(i).i64_const(1).i64_add().local_set(i);
                self.br(top);
                self.close();
                self.close();
            }
            StmtKind::ForList(x, xs, body) => self.for_list(*x, xs, body, s.span),
            StmtKind::Break => {
                let (brk, _, mark) = *self.loops.last().expect("checked loop");
                self.release(mark);
                self.br(brk);
            }
            StmtKind::Continue => {
                let (_, cont, mark) = *self.loops.last().expect("checked loop");
                self.release(mark);
                self.br(cont);
            }
            StmtKind::Expr(e) => {
                self.expr(e);
                if is_ptr(&e.ty) {
                    self.dec_top();
                } else if val_type(&e.ty).is_some() {
                    self.sink().drop();
                }
            }
        }
    }

    /// `x = x + a + b ..` (`parts` is `[x, a, b, ..]`, the read of `x` first): the parts in
    /// order as operands, then `x`'s own reference dropped (`x` is reassigned here, and nothing
    /// runs before the store but runtime calls that return or trap), then `rt_str_append` per
    /// part, which takes the read's reference: when that is the only one left, the string grows
    /// in place. A part that reads `x` holds a reference of its own, so the old value is then
    /// shared and copied, as is a pinned literal; after a part that reassigns `x`, the read
    /// holds the old value's only reference. Leaves the result to store.
    ///
    /// A part that reads another local is borrowed (`operands_borrowing`); one reading `x` is
    /// not, since `x`'s reference is dropped before the appends.
    fn append(&mut self, x: ir::LocalId, l: u32, parts: Vec<&ir::Expr>) {
        let other = |i: usize| i > 0 && parts[i].kind != ExprKind::Local(x);
        let (mark, temps, borrowed) = self.operands_borrowing(&parts, other);
        self.dec(l);
        self.get(parts[0], temps[0]);
        for (x, t) in parts.iter().zip(&temps).skip(1) {
            self.get(x, *t);
            self.call_rt("rt_str_append");
        }
        self.drop_operands(&parts[1..], &temps[1..], &borrowed[1..]);
        self.owned.truncate(mark);
    }

    fn br_if(&mut self, level: u32) {
        let rel = self.depth - level;
        self.sink().br_if(rel);
    }

    /// `for x in xs`: the loop holds its own reference to the list in a hidden local, so a
    /// change to `xs` in the body copies it and the loop sees the list as it was. That
    /// reference and `x` are released when the loop ends by any exit; `x` is also released at
    /// the end of each pass.
    fn for_list(&mut self, x: ir::LocalId, xs: &ir::Expr, body: &ir::Block, span: Span) {
        if xs.ty == Ty::Never {
            return self.expr(xs);
        }
        let mark = self.owned.len();
        let list = self.temp(ValType::I32);
        match xs.kind {
            // A borrowed parameter is never changed here and the caller holds it (C8b4).
            ExprKind::Local(id) if self.is_lent(id) => {
                let l = self.local(id).expect("a list");
                self.sink().local_get(l).local_set(list);
            }
            _ => {
                self.expr(xs);
                self.sink().local_set(list);
                self.owned.push(list);
            }
        }
        let ty = elem(&xs.ty);
        let x = self.local(x);
        let x_ptr = x.filter(|_| is_ptr(ty));
        self.owned.extend(x_ptr);
        let (n, i) = (self.temp(ValType::I64), self.temp(ValType::I64));
        self.sink().local_get(list);
        self.call_rt("rt_list_len");
        self.sink().local_set(n).i64_const(0).local_set(i);
        let brk = self.open(Open::Block, BlockType::Empty);
        let top = self.open(Open::Loop, BlockType::Empty);
        self.sink().local_get(i).local_get(n).i64_ge_s();
        self.br_if(brk);
        let cont = self.open(Open::Block, BlockType::Empty);
        if let Some(x) = x {
            // In bounds: the list cannot change while the loop holds it.
            let site = self.site(span, OUT_OF_BOUNDS);
            self.list_get(list, i, site, ty);
            self.slot_out(ty);
            self.sink().local_set(x);
        }
        self.loops.push((brk, cont, self.owned.len()));
        self.block(body, None);
        self.loops.pop();
        self.close();
        if let Some(x) = x_ptr {
            self.dec(x);
            self.sink().i32_const(0).local_set(x);
        }
        self.sink().local_get(i).i64_const(1).i64_add().local_set(i);
        self.br(top);
        self.close();
        self.close();
        // After a `break`, `x` still holds that pass's element; otherwise it is 0.
        self.end_scope(mark);
    }

    /// `a[i].f = v`, `+=` and `-=`: the indexes in order, then the value, then the place made
    /// unique along its path, then the last step's slot (bounds-checked for an index) and the
    /// write, which drops the old value. An overflow reports `span`, the statement's.
    fn assign_element(&mut self, place: &ir::Place, op: Option<BinOp>, value: &ir::Expr, span: Span) {
        let (mut xs, sites) = self.indexes(place);
        let n = xs.len();
        xs.push(value);
        let (mark, temps) = self.operands(&xs);
        let last = place.steps.len() - 1;
        let mut k = 0;
        let (cur, a) = self.walk(place, &temps, &sites, last, &mut k);
        let a = a.unwrap_or_else(|| self.temp(ValType::I32));
        self.step_slot(&place.steps[last], cur, a, &temps, &sites, &mut k);
        match (op, temps[n]) {
            (None, v) => {
                // The slot takes the value, then the old one is dropped.
                let old = is_ptr(&value.ty).then(|| self.temp(ValType::I32));
                if let Some(old) = old {
                    self.sink().local_get(a).i64_load(SLOT).i32_wrap_i64().local_set(old);
                }
                self.sink().local_get(a);
                self.get(value, v);
                self.slot_in(&value.ty);
                self.sink().i64_store(SLOT);
                if let Some(old) = old {
                    self.dec(old);
                }
            }
            (Some(_), None) => {
                self.sink().unreachable();
            }
            (Some(op), Some(v)) => {
                self.sink().local_get(a).local_get(a).i64_load(SLOT);
                if op == BinOp::Concat {
                    // `old + v`: `rt_str_append` takes the slot's reference to `old` (growing
                    // it in place if that was the only one) and borrows `v`, then dropped.
                    self.sink().i32_wrap_i64().local_get(v);
                    self.call_rt("rt_str_append");
                    self.sink().i64_extend_i32_u().i64_store(SLOT);
                    self.dec(v);
                } else if matches!(op, BinOp::FAdd | BinOp::FSub) {
                    self.sink().f64_reinterpret_i64().local_get(v);
                    self.float_op(op);
                    self.sink().i64_reinterpret_f64().i64_store(SLOT);
                } else {
                    self.sink().local_get(v);
                    self.int_op(op, span);
                    self.sink().i64_store(SLOT);
                }
            }
        }
        self.owned.truncate(mark);
    }

    /// `push`, `insert`, `remove`, `sort` and `pop` (of type `ty`) on a place: the indexes,
    /// then the arguments, then the place made unique along its path, and the change.
    fn mutate(&mut self, place: &ir::Place, op: Mut, args: &[ir::Expr], ty: &Ty, span: Span) {
        let (mut xs, sites) = self.indexes(place);
        let n = xs.len();
        xs.extend(args);
        // A map key is borrowed: it is read after the walk, which changes only the root.
        let key = matches!(op, Mut::MapInsert | Mut::MapRemove);
        let (mark, temps, borrowed) = self.operands_borrowing(&xs, |i| key && i == n);
        let (cur, slot) = self.walk(place, &temps, &sites, place.steps.len(), &mut 0);
        let args: Vec<(&ir::Expr, Option<u32>)> = args.iter().zip(temps[n..].iter().copied()).collect();
        if op == Mut::ListPop {
            // The popped element moves into the `Some`, the expression's (+1) value.
            let pre = self.pre(Pre::Opt, ty);
            for c in pre {
                self.sink().i32_const(c as i32);
            }
            self.sink().local_get(cur);
            self.call_rt("rt_list_pop");
        } else if op == Mut::ListSort {
            self.sink().local_get(cur);
            self.call_rt("rt_list_sort");
        } else if op == Mut::BytesPush {
            // `rt_bytes_push` returns the object to store back: the same one or a larger one.
            let site = self.site(span, "byte {a} out of range 0..=255");
            if let Some(a) = slot {
                self.sink().local_get(a);
            }
            self.sink().local_get(cur);
            self.get(args[0].0, args[0].1);
            self.bytes_push(site);
            match slot {
                Some(_) => self.sink().i64_extend_i32_u().i64_store(SLOT),
                None => self.sink().local_set(cur),
            };
        } else {
            // The value becomes the container's; a key is borrowed, then dropped.
            self.sink().local_get(cur);
            for &(x, t) in &args {
                self.get(x, t);
                self.slot_in(&x.ty);
            }
            self.call_rt(match op {
                Mut::ListPush => "rt_list_push",
                Mut::MapInsert => "rt_map_insert",
                Mut::MapRemove => "rt_map_remove",
                op => unreachable!("{op:?} is handled above"),
            });
            if let (Mut::MapInsert | Mut::MapRemove, (k, Some(t))) = (op, args[0]) {
                if is_ptr(&k.ty) && !borrowed[n] {
                    self.dec(t);
                }
            }
        }
        self.owned.truncate(mark);
    }

    /// The index expressions of `place`, and a bounds-error site for each.
    fn indexes<'p>(&mut self, place: &'p ir::Place) -> (Vec<&'p ir::Expr>, Vec<u32>) {
        let mut xs = Vec::new();
        let mut sites = Vec::new();
        for step in &place.steps {
            if let ir::PlaceStep::Index(i, span) = step {
                xs.push(i);
                sites.push(self.site(*span, OUT_OF_BOUNDS));
            }
        }
        (xs, sites)
    }

    /// Makes `place`'s root unique and stores it back; then, for each of its first `n` steps:
    /// finds the step's slot (bounds-checking an index, whose value is `temps[*k]`), makes the
    /// value there unique and stores it back, and descends. Returns the local holding the
    /// container reached, and one holding the address of its slot unless it is the root.
    fn walk(
        &mut self,
        place: &ir::Place,
        temps: &[Option<u32>],
        sites: &[u32],
        n: usize,
        k: &mut usize,
    ) -> (u32, Option<u32>) {
        let root = self.local(place.root).expect("a container local");
        debug_assert!(!self.is_lent(place.root), "a borrowed parameter changed in place");
        self.sink().local_get(root);
        self.unique_top();
        self.sink().local_set(root);
        if n == 0 {
            return (root, None);
        }
        let (cur, a) = (self.temp(ValType::I32), self.temp(ValType::I32));
        self.sink().local_get(root).local_set(cur);
        for step in &place.steps[..n] {
            self.step_slot(step, cur, a, temps, sites, k);
            self.sink().local_get(a).local_get(a).i64_load(SLOT).i32_wrap_i64();
            self.unique_top();
            self.sink().local_tee(cur).i64_extend_i32_u().i64_store(SLOT);
        }
        (cur, Some(a))
    }

    /// Sets local `a` to the address of the slot that `step` names in the container in `cur`:
    /// a field's, or a list element's after its bounds check (index `temps[*k]`, then `*k`
    /// moves on).
    fn step_slot(
        &mut self,
        step: &ir::PlaceStep,
        cur: u32,
        a: u32,
        temps: &[Option<u32>],
        sites: &[u32],
        k: &mut usize,
    ) {
        match step {
            ir::PlaceStep::Field(f) => {
                let off = RECORD_SLOTS as i32 + 8 * *f as i32;
                self.sink().local_get(cur).i32_const(off).i32_add();
            }
            ir::PlaceStep::Index(..) => {
                match temps[*k] {
                    Some(i) => {
                        self.sink().local_get(cur).local_get(i).i32_const(sites[*k] as i32);
                        self.call_rt("rt_list_at");
                    }
                    // An index of type `!` (`xs[{ return 7; }] = v`) never yields a value.
                    None => {
                        self.sink().unreachable();
                    }
                }
                *k += 1;
            }
        }
        self.sink().local_set(a);
    }

    /// The body of a `while` or `loop` whose `block` and `loop` are open; closes both.
    fn loop_body(&mut self, body: &ir::Block, brk: u32, cont: u32) {
        self.loops.push((brk, cont, self.owned.len()));
        self.block(body, None);
        self.loops.pop();
        self.br(cont);
        self.close();
        self.close();
    }

    /// Leaves `e`'s value on the stack, if its type has one. An expression of type `!` ends
    /// with `unreachable`, so any context's types check.
    fn expr(&mut self, e: &ir::Expr) {
        let mark = self.live.len();
        self.expr_kind(e);
        self.free_since(mark);
    }

    fn expr_kind(&mut self, e: &ir::Expr) {
        match &e.kind {
            ExprKind::Int(n) => {
                self.sink().i64_const(*n);
            }
            ExprKind::Bool(b) => {
                self.sink().i32_const(*b as i32);
            }
            ExprKind::Float(x) => {
                self.sink().f64_const(Ieee64::new(x.to_bits()));
            }
            ExprKind::Unary(UnOp::FNeg, x) => {
                self.expr(x);
                self.sink().f64_neg();
            }
            ExprKind::Str(s) => {
                let a = self.statics.str(s);
                self.sink().i32_const(a as i32);
            }
            ExprKind::Unit => {}
            ExprKind::Local(id) => {
                if let Some(l) = self.local(*id) {
                    if is_ptr(&e.ty) && self.moves_out(e) {
                        // The last use: the reference moves out, and the slot is left empty,
                        // so the scope's end and every exit drop nothing (`rt_dec(0)`).
                        self.sink().local_get(l).i32_const(0).local_set(l);
                    } else {
                        if is_ptr(&e.ty) {
                            // A copy: the local keeps its own reference.
                            self.inc(l);
                        }
                        self.sink().local_get(l);
                    }
                }
            }
            ExprKind::Unary(UnOp::Not, x) => {
                self.expr(x);
                self.sink().i32_eqz();
            }
            ExprKind::Unary(UnOp::Neg, x) => {
                self.expr(x);
                let [a, _, _] = self.scratch();
                self.sink().local_tee(a).i64_const(i64::MIN).i64_eq();
                self.trap_if(e.span, OVERFLOW, a, a);
                self.sink().i64_const(0).local_get(a).i64_sub();
            }
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), a, b) => {
                // The right side runs only when the left does not decide.
                self.expr(a);
                self.open(Open::If, BlockType::Result(ValType::I32));
                if *op == BinOp::And {
                    self.expr(b);
                    self.sink().else_().i32_const(0);
                } else {
                    self.sink().i32_const(1).else_();
                    self.expr(b);
                }
                self.close();
            }
            ExprKind::Binary(BinOp::Concat, a, b) => self.rt_op("rt_concat", &[&**a, &**b], None),
            ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne), a, b) if is_ptr(&a.ty) => {
                let desc = self.statics.desc(&a.ty);
                self.borrowing(&[&**a, &**b], |g, temps| {
                    g.sink().i32_const(desc as i32);
                    for (x, t) in [a, b].into_iter().zip(temps) {
                        g.get(x, *t);
                        g.sink().i64_extend_i32_u();
                    }
                    g.call_rt("rt_eq");
                    if *op == BinOp::Ne {
                        g.sink().i32_eqz();
                    }
                });
            }
            // `<` on `Str`: the runtime's byte order as -1, 0 or 1, compared with 0.
            ExprKind::Binary(op @ (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), a, b) if a.ty == Ty::Str => {
                self.rt_op("rt_str_cmp", &[&**a, &**b], None);
                let mut s = self.sink();
                s.i32_const(0);
                match op {
                    BinOp::Lt => s.i32_lt_s(),
                    BinOp::Le => s.i32_le_s(),
                    BinOp::Gt => s.i32_gt_s(),
                    _ => s.i32_ge_s(),
                };
            }
            ExprKind::Binary(op, a, b) => {
                self.expr(a);
                self.expr(b);
                let i = match (op, &a.ty) {
                    (_, Ty::Int) => return self.int_op(*op, e.span),
                    (_, Ty::Float) => return self.float_op(*op),
                    (BinOp::Eq, Ty::Bool) => Instruction::I32Eq,
                    (BinOp::Ne, Ty::Bool) => Instruction::I32Ne,
                    // `()` has no value: equal always.
                    (BinOp::Eq, Ty::Unit) => Instruction::I32Const(1),
                    (BinOp::Ne, Ty::Unit) => Instruction::I32Const(0),
                    (op, t) => unreachable!("checked: {op:?} on {t:?}"),
                };
                i.encode(&mut self.code);
            }
            ExprKind::If(cond, then, els) => {
                let vt = val_type(&e.ty);
                self.expr(cond);
                self.open(Open::If, vt.map_or(BlockType::Empty, BlockType::Result));
                self.block(then, vt);
                if let Some(els) = els {
                    self.sink().else_();
                    self.expr(els);
                }
                self.close();
            }
            ExprKind::Block(b) => self.block(b, val_type(&e.ty)),
            ExprKind::Call(f, args) => self.call(*f, args),
            ExprKind::FnRef(f) => {
                let a = self.statics.pinned_fn(*f, None);
                self.sink().i32_const(a as i32);
            }
            ExprKind::Closure(f, captured) if captured.is_empty() => {
                let a = self.statics.pinned_fn(*f, Some(0));
                self.sink().i32_const(a as i32);
            }
            ExprKind::Closure(f, captured) => self.closure(*f, captured),
            ExprKind::CallValue(callee, args) => self.call_value(callee, args),
            ExprKind::Builtin(b, recv, args) if takes_fn(*b) => self.with_fn(*b, recv, &args[0], &e.ty, e.span),
            ExprKind::Builtin(b, recv, _) if inline_builtin(*b) => {
                self.expr(recv);
                self.scalar_builtin(*b);
            }
            ExprKind::Builtin(b, recv, args) => {
                use Pre::{Desc, Nothing, Opt};
                let (name, pre, error) = match b {
                    Builtin::IntToFloat
                    | Builtin::FloatAbs
                    | Builtin::FloatFloor
                    | Builtin::FloatCeil
                    | Builtin::FloatSqrt
                    | Builtin::FloatIsNan
                    | Builtin::FloatIsFinite
                    | Builtin::FloatIsInfinite => unreachable!("inline"),
                    Builtin::StrParseFloat => ("rt_str_parse_float", Opt, None),
                    Builtin::FloatToStr => ("rt_float_to_str", Nothing, None),
                    Builtin::FloatToInt => ("rt_float_to_int", Opt, None),
                    Builtin::FloatRound => ("rt_float_round", Nothing, None),
                    Builtin::StrSlice => ("rt_str_slice", Nothing, Some(SLICE_BOUNDS)),
                    Builtin::BytesSlice => ("rt_bytes_slice", Nothing, Some(SLICE_BOUNDS)),
                    Builtin::StrParseInt => ("rt_str_parse_int", Opt, None),
                    Builtin::MapGet => ("rt_map_get", Opt, None),
                    Builtin::OptionUnwrapOr => ("rt_option_unwrap_or", Nothing, None),
                    Builtin::ResultUnwrapOr => ("rt_result_unwrap_or", Nothing, None),
                    Builtin::OptionIsSome => ("rt_option_is_some", Nothing, None),
                    Builtin::OptionIsNone => ("rt_option_is_none", Nothing, None),
                    Builtin::ResultIsOk => ("rt_result_is_ok", Nothing, None),
                    Builtin::ResultIsErr => ("rt_result_is_err", Nothing, None),
                    Builtin::ResultOk => ("rt_result_ok", Opt, None),
                    Builtin::ResultErr => ("rt_result_err", Opt, None),
                    Builtin::OptionOkOr => ("rt_option_ok_or", Desc, None),
                    Builtin::IntToStr => ("rt_int_to_str", Nothing, None),
                    Builtin::StrLen => ("rt_str_len", Nothing, None),
                    Builtin::StrToBytes => ("rt_str_to_bytes", Nothing, None),
                    Builtin::StrSplit => ("rt_str_split", Desc, Some("`split` needs a non-empty separator")),
                    Builtin::StrLines => ("rt_str_lines", Desc, None),
                    Builtin::StrTrim => ("rt_str_trim", Nothing, None),
                    Builtin::StrContains => ("rt_str_contains", Nothing, None),
                    Builtin::StrStartsWith => ("rt_str_starts_with", Nothing, None),
                    Builtin::StrEndsWith => ("rt_str_ends_with", Nothing, None),
                    Builtin::StrStripPrefix => ("rt_str_strip_prefix", Opt, None),
                    Builtin::StrStripSuffix => ("rt_str_strip_suffix", Opt, None),
                    Builtin::StrFind => ("rt_str_find", Opt, None),
                    Builtin::StrReplace => ("rt_str_replace", Nothing, None),
                    Builtin::ListContains => ("rt_list_contains", Nothing, None),
                    Builtin::BytesLen => ("rt_bytes_len", Nothing, None),
                    Builtin::BytesToStr => ("rt_bytes_to_str", Nothing, Some("invalid UTF-8")),
                    Builtin::ListLen => ("rt_list_len", Nothing, None),
                    Builtin::MapLen => ("rt_map_len", Nothing, None),
                    Builtin::MapKeys => ("rt_map_keys", Desc, None),
                    Builtin::StrParseJson => ("rt_json_parse", Desc, None),
                    Builtin::JsonToJson => ("rt_json_encode", Nothing, None),
                    Builtin::JsonGet => ("rt_json_get", Opt, None),
                    Builtin::JsonAt => ("rt_json_at", Opt, None),
                    Builtin::JsonAsStr => ("rt_json_as_str", Opt, None),
                    Builtin::JsonAsInt => ("rt_json_as_int", Opt, None),
                    Builtin::JsonAsFloat => ("rt_json_as_float", Opt, None),
                    Builtin::JsonAsBool => ("rt_json_as_bool", Opt, None),
                    Builtin::JsonAsList => ("rt_json_as_list", Opt, None),
                    Builtin::JsonAsMap => ("rt_json_as_map", Opt, None),
                    Builtin::OptionMap
                    | Builtin::OptionAndThen
                    | Builtin::ResultMap
                    | Builtin::ResultMapErr
                    | Builtin::ResultAndThen
                    | Builtin::ListMap
                    | Builtin::ListFilter
                    | Builtin::ListAny
                    | Builtin::ListAll => unreachable!("with_fn"),
                };
                let pre = self.pre(pre, &e.ty);
                let mut sites: Vec<u32> = error.map(|t| self.site(e.span, t)).into_iter().collect();
                if *b == Builtin::StrSlice {
                    sites.push(self.site(e.span, "slice {a}..{b} is not on a character boundary"));
                }
                let xs: Vec<&ir::Expr> = std::iter::once(&**recv).chain(args).collect();
                // These take their argument as a slot; `unwrap_or` gives a slot.
                let unwrap = matches!(b, Builtin::OptionUnwrapOr | Builtin::ResultUnwrapOr);
                let slots = unwrap || matches!(b, Builtin::MapGet | Builtin::ListContains | Builtin::OptionOkOr);
                self.rt_op_slots(name, &pre, &xs, &sites, slots);
                if unwrap {
                    self.slot_out(&e.ty);
                }
            }
            ExprKind::Index(base, i) if base.ty == Ty::Bytes => {
                let site = self.site(e.span, OUT_OF_BOUNDS);
                self.borrowing(&[&**base, &**i], |g, temps| match temps {
                    [Some(b), Some(i)] => g.bytes_get(*b, *i, site),
                    _ => {
                        g.sink().unreachable();
                    }
                });
            }
            ExprKind::Index(base, i) => {
                // The element is +1, as a slot.
                let site = self.site(e.span, OUT_OF_BOUNDS);
                self.borrowing(&[&**base, &**i], |g, temps| match temps {
                    [Some(l), Some(i)] => g.list_get(*l, *i, site, &e.ty),
                    // An operand of type `!`: the code after it is unreachable.
                    _ => {
                        g.sink().unreachable();
                    }
                });
                self.slot_out(&e.ty);
            }
            ExprKind::List(items) => {
                // The new list is held in a hidden local while its elements run, so an exit
                // in one releases it; each element is consumed by the push.
                let desc = self.statics.desc(&e.ty);
                let l = self.temp(ValType::I32);
                self.sink().i32_const(desc as i32).i32_const(items.len() as i32);
                self.call_rt("rt_list_new");
                self.sink().local_set(l);
                self.owned.push(l);
                for x in items {
                    self.sink().local_get(l);
                    self.expr(x);
                    self.slot_in(&x.ty);
                    self.call_rt("rt_list_push");
                }
                self.owned.pop();
                self.sink().local_get(l);
            }
            ExprKind::MapNew => {
                let desc = self.statics.desc(&e.ty);
                self.sink().i32_const(desc as i32);
                self.call_rt("rt_map_new");
            }
            ExprKind::Mutate(place, op, args) => self.mutate(place, *op, args, &e.ty, e.span),
            ExprKind::StructLit(_, inits) => {
                let desc = self.statics.desc(&e.ty);
                let inits: Vec<(u32, &ir::Expr)> = inits.iter().map(|(f, x)| (*f, x)).collect();
                self.object(desc, 0, RECORD_SLOTS, &inits);
            }
            ExprKind::Variant(_, v, args) if args.is_empty() => {
                let a = self.statics.pinned(&e.ty, *v);
                self.sink().i32_const(a as i32);
            }
            ExprKind::Variant(_, v, args) => {
                let desc = self.statics.desc(&e.ty);
                let args: Vec<(u32, &ir::Expr)> = args.iter().enumerate().map(|(i, x)| (i as u32, x)).collect();
                self.object(desc, *v, ENUM_SLOTS, &args);
            }
            ExprKind::Field(base, f) => self.field(base, *f, &e.ty),
            ExprKind::CapCall(cap, method, args) => self.cap_call(*cap, *method, args, &e.ty),
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms, &e.ty),
            ExprKind::Try(x) => self.try_expr(x),
        }
        if e.ty == Ty::Never {
            self.sink().unreachable();
        }
    }

    /// `f(args)`: the arguments in order, then a direct call. An owned parameter takes its
    /// argument. A borrowed one (C8b4) is lent it, and the caller keeps its reference across
    /// the call: a local, or a field of one, that no later argument mentions is passed in place
    /// (nothing evaluated before the call can then change it, and the callee cannot reach the
    /// caller's locals), and if it was the local's last use, the local is dropped and emptied
    /// after the call rather than moved; any other argument is evaluated, held like an operand,
    /// and dropped after the call.
    fn call(&mut self, f: ir::FuncId, args: &[ir::Expr]) {
        let all = self.lent;
        let lent = all[f.0 as usize].as_slice();
        let mark = self.owned.len();
        let mut temps = Vec::new();
        let mut in_place = Vec::new();
        for (i, x) in args.iter().enumerate() {
            let here = lent[i] && self.in_place(x) && !args[i + 1..].iter().any(|y| moves::mentions(y, root(x)));
            in_place.push(here);
            if here {
                temps.push(None);
                continue;
            }
            self.expr(x);
            let t = val_type(&x.ty).map(|vt| self.temp(vt));
            if let Some(t) = t {
                self.sink().local_set(t);
                if is_ptr(&x.ty) {
                    self.owned.push(t);
                }
            }
            temps.push(t);
        }
        for ((x, t), &here) in args.iter().zip(&temps).zip(&in_place) {
            match here {
                true => self.borrow(x),
                false => self.get(x, *t),
            }
        }
        self.owned.truncate(mark);
        let index = self.rt.funcs + f.0;
        self.sink().call(index);
        // Only the call ran since the arguments: it returned (or trapped).
        for (((x, t), &here), &lent) in args.iter().zip(&temps).zip(&in_place).zip(lent) {
            match (here, *t) {
                (true, _) if self.moves_out(x) => {
                    let l = self.borrowed_local(x).expect("a pointer local");
                    self.dec(l);
                    self.sink().i32_const(0).local_set(l);
                }
                (false, Some(t)) if lent => self.dec(t),
                _ => {}
            }
        }
    }

    /// A capability method: the runtime's wrapper (runtime/src/caps.rs) takes the result's
    /// descriptor, the handle, then the arguments, which it borrows.
    fn cap_call(&mut self, cap: ir::LocalId, method: u32, args: &[ir::Expr], ty: &Ty) {
        let Ty::Cap(c) = self.f.locals[cap.0 as usize].ty else {
            unreachable!("a capability call on a capability")
        };
        let def = &self.m.caps[c.0 as usize];
        let name = format!("rt_cap_{}_{}", snake(&def.name), def.methods[method as usize].0);
        let desc = self.statics.desc(ty);
        let h = self.local(cap).expect("a capability has a value");
        let xs: Vec<&ir::Expr> = args.iter().collect();
        self.borrowing(&xs, |g, temps| {
            g.sink().i32_const(desc as i32).local_get(h);
            for (x, t) in xs.iter().zip(temps) {
                g.get(x, *t);
            }
            g.call_rt(&name);
        });
    }

    /// A new struct (`first` 8) or enum variant (`first` 16) of descriptor `desc`: the object
    /// is held in a hidden local while `parts` run in order, so an exit in one releases it;
    /// each part's value goes into its slot (`first + 8 * index`), which takes it.
    fn object(&mut self, desc: u32, variant: u32, first: u64, parts: &[(u32, &ir::Expr)]) {
        let o = self.temp(ValType::I32);
        // A part that exits frees the object, which reads its pointer slots: those start 0.
        let zero: Vec<u64> = parts
            .iter()
            .filter(|(_, x)| is_ptr(&x.ty))
            .map(|(i, _)| first + 8 * u64::from(*i))
            .collect();
        self.alloc(desc, variant, first, parts.len(), &zero, o);
        self.owned.push(o);
        for (i, x) in parts {
            self.sink().local_get(o);
            self.expr(x);
            self.slot_in(&x.ty);
            self.sink().i64_store(slot_at(first + 8 * u64::from(*i)));
        }
        self.owned.pop();
        self.sink().local_get(o);
    }

    /// `rt_alloc(desc, variant)` into local `o`: a struct (`first` 8), or an enum variant or a
    /// closure (`first` 16, `variant` the word at byte 8), of `n` slots, count 1. The slots at
    /// the offsets in `zero` are 0; the caller writes the others before anything reads them.
    ///
    /// A small object is taken inline from its size class's free list (runtime/src/heap.rs,
    /// C8b5): pop the head at `lists + 4 * class`, write the header and the word at 8, add 1
    /// to the live count, as `rt_alloc` does; an empty list calls `rt_alloc`, which carves a
    /// fresh block. The size is the runtime's for the descriptor (the table's slot count), so
    /// the runtime frees the block into the same class; if the table disagrees with `n` (only
    /// crafted IR), the runtime allocates, and reports the bad variant.
    fn alloc(&mut self, desc: u32, variant: u32, first: u64, n: usize, zero: &[u64], o: u32) {
        let size = first + 8 * n as u64;
        let f = self.rt.export("rt_alloc");
        let (lists, live) = (self.rt.lists, self.rt.live);
        let known = self.statics.slots(desc, variant) == Some(n);
        let mut s = self.sink();
        if size > HEAP_SMALL || !known {
            s.i32_const(desc as i32).i32_const(variant as i32).call(f).local_set(o);
            return;
        }
        let head = mem32(u64::from(lists) + 4 * ((size - 1) / 8));
        s.i32_const(0).i32_load(head).local_tee(o).if_(BlockType::Empty);
        s.i32_const(0).local_get(o).i32_load(mem32(0)).i32_store(head);
        s.local_get(o)
            .i64_const(((u64::from(desc) << 32) | 1) as i64)
            .i64_store(slot_at(0));
        if first == ENUM_SLOTS {
            s.local_get(o).i64_const(i64::from(variant)).i64_store(slot_at(8));
        }
        for &z in zero {
            s.local_get(o).i64_const(0).i64_store(slot_at(z));
        }
        let live = mem32(u64::from(live));
        s.i32_const(0)
            .i32_const(0)
            .i32_load(live)
            .i32_const(1)
            .i32_add()
            .i32_store(live);
        s.else_();
        s.i32_const(desc as i32).i32_const(variant as i32).call(f).local_set(o);
        s.end();
    }

    /// A closure lifted to `f`: a new function object holding a copy (+1) of each captured
    /// local in its slots, which the object owns until it is freed. Nothing here can exit, so
    /// the object needs no place in `owned` while it is filled.
    fn closure(&mut self, f: ir::FuncId, captured: &[ir::LocalId]) {
        let (desc, elem) = self.statics.func(f, Some(captured.len()));
        let o = self.temp(ValType::I32);
        // Every pointer slot is written below, before anything could read it.
        self.alloc(desc, elem, ENUM_SLOTS, captured.len(), &[], o);
        let m = self.m;
        for (i, (id, l)) in captured.iter().zip(&m.funcs[f.0 as usize].locals).enumerate() {
            let Some(x) = self.local(*id) else { continue };
            if is_ptr(&l.ty) {
                self.sink().local_get(x);
                self.inc_top();
            }
            self.sink().local_get(o).local_get(x);
            self.slot_in(&l.ty);
            self.sink().i64_store(slot_at(ENUM_SLOTS + 8 * i as u64));
        }
        self.sink().local_get(o);
    }

    /// `callee(args)`: the callee, then the arguments, each held as an operand; then a
    /// `call_indirect` through the callee's table entry, passing the callee (borrowed, for its
    /// captures) and the arguments (consumed); then the callee is dropped.
    fn call_value(&mut self, callee: &ir::Expr, args: &[ir::Expr]) {
        let Ty::Fn(params, ret) = &callee.ty else {
            return self.expr(callee); // `!`: nothing after it runs
        };
        let xs: Vec<&ir::Expr> = std::iter::once(callee).chain(args).collect();
        // A local callee is borrowed: the call cannot change the caller's locals.
        let (mark, temps, borrowed) = self.operands_borrowing(&xs, |i| i == 0);
        let c = temps[0].expect("a fn value");
        self.sink().local_get(c);
        for (x, t) in args.iter().zip(&temps[1..]) {
            self.get(x, *t);
        }
        self.owned.truncate(mark);
        self.call_through(c, params, ret);
        if !borrowed[0] {
            self.dec(c);
        }
    }

    /// Calls the `fn(params) -> ret` value in local `c` through its table entry, on the object
    /// and the arguments on the stack.
    fn call_through(&mut self, c: u32, params: &[Ty], ret: &Ty) {
        let ty = self.rt.types + type_index(self.types, value_sig(params, ret));
        self.sink().local_get(c).i32_load(VARIANT).call_indirect(0, ty);
        if *ret == Ty::Never {
            self.sink().unreachable();
        }
    }

    /// A built-in taking a function value `f` (`takes_fn`), inline: the receiver, then `f`,
    /// each evaluated once and owned; then the calls, in the interpreter's order, each through
    /// `f` as `call_value` does, with the element or payload passed +1; then the receiver and
    /// `f` are dropped. Between the operands and the drops nothing leaves the function (a call
    /// returns or traps), so neither needs its place in `owned` past its evaluation.
    fn with_fn(&mut self, b: Builtin, recv: &ir::Expr, f: &ir::Expr, ty: &Ty, span: Span) {
        let (mark, temps, borrowed) = self.operands_borrowing(&[recv, f], |_| true);
        self.owned.truncate(mark);
        let (Some(r), Some(c), Ty::Fn(params, ret)) = (temps[0], temps[1], &f.ty) else {
            // The receiver or `f` is `!`: nothing after it runs.
            self.sink().unreachable();
            return;
        };
        let sig = (&params[..], &**ret);
        match b {
            Builtin::ListMap => self.list_map(r, c, &recv.ty, sig, ty, span),
            Builtin::ListFilter => self.list_filter(r, c, &recv.ty, sig, ty, span),
            Builtin::ListAny => self.list_any(true, r, c, &recv.ty, sig, span),
            Builtin::ListAll => self.list_any(false, r, c, &recv.ty, sig, span),
            _ => self.enum_map(b, r, c, &recv.ty, sig, ty),
        }
        self.drop_operands(&[recv, f], &temps, &borrowed);
    }

    /// Opens a loop over the elements of list `r`: `i` from 0 to its length, read once.
    /// Returns `(i, brk, top)`; the pass's code follows, then `next_pass`.
    fn list_loop(&mut self, r: u32) -> (u32, u32, u32) {
        let (n, i) = (self.temp(ValType::I64), self.temp(ValType::I64));
        self.sink().local_get(r);
        self.call_rt("rt_list_len");
        self.sink().local_set(n).i64_const(0).local_set(i);
        let brk = self.open(Open::Block, BlockType::Empty);
        let top = self.open(Open::Loop, BlockType::Empty);
        self.sink().local_get(i).local_get(n).i64_ge_s();
        self.br_if(brk);
        (i, brk, top)
    }

    /// Pushes element `i` of list `r` as a slot, +1. In bounds: nothing changes `r` while a
    /// built-in holds it.
    fn list_slot(&mut self, r: u32, i: u32, xs: &Ty, span: Span) {
        let site = self.site(span, OUT_OF_BOUNDS);
        self.list_get(r, i, site, elem(xs));
    }

    /// Ends a pass of `list_loop`'s loop and the loop.
    fn next_pass(&mut self, i: u32, top: u32) {
        self.sink().local_get(i).i64_const(1).i64_add().local_set(i);
        self.br(top);
        self.close();
        self.close();
    }

    /// `xs.map(f)`: a new list of type `ty`, with `xs`'s length as capacity, which takes each
    /// result.
    fn list_map(&mut self, r: u32, c: u32, xs: &Ty, (params, ret): (&[Ty], &Ty), ty: &Ty, span: Span) {
        let out = self.temp(ValType::I32);
        let desc = self.statics.desc(ty);
        self.sink().i32_const(desc as i32).local_get(r);
        self.call_rt("rt_list_len");
        self.sink().i32_wrap_i64();
        self.call_rt("rt_list_new");
        self.sink().local_set(out);
        let (i, _, top) = self.list_loop(r);
        self.sink().local_get(out).local_get(c);
        self.list_slot(r, i, xs, span);
        self.slot_out(elem(xs));
        self.call_through(c, params, ret);
        self.slot_in(ret);
        self.call_rt("rt_list_push");
        self.next_pass(i, top);
        self.sink().local_get(out);
    }

    /// `xs.filter(f)`: a new list of type `ty` holding each element for which `f` returns true,
    /// +1 again, since the call consumed the reference `rt_list_get` gave.
    fn list_filter(&mut self, r: u32, c: u32, xs: &Ty, (params, ret): (&[Ty], &Ty), ty: &Ty, span: Span) {
        let t = elem(xs);
        let (out, x) = (self.temp(ValType::I32), self.temp(ValType::I64));
        let desc = self.statics.desc(ty);
        self.sink().i32_const(desc as i32).i32_const(0);
        self.call_rt("rt_list_new");
        self.sink().local_set(out);
        let (i, _, top) = self.list_loop(r);
        self.list_slot(r, i, xs, span);
        self.sink().local_set(x).local_get(c).local_get(x);
        self.slot_out(t);
        self.call_through(c, params, ret);
        self.open(Open::If, BlockType::Empty);
        if is_ptr(t) {
            self.sink().local_get(x).i32_wrap_i64();
            self.inc_top();
        }
        self.sink().local_get(out).local_get(x);
        self.call_rt("rt_list_push");
        self.close();
        self.next_pass(i, top);
        self.sink().local_get(out);
    }

    /// `xs.any(f)` (`any`) or `xs.all(f)`: stops at the first call returning `any`, as Rust's.
    fn list_any(&mut self, any: bool, r: u32, c: u32, xs: &Ty, (params, ret): (&[Ty], &Ty), span: Span) {
        let res = self.temp(ValType::I32);
        self.sink().i32_const(!any as i32).local_set(res);
        let (i, brk, top) = self.list_loop(r);
        self.sink().local_get(c);
        self.list_slot(r, i, xs, span);
        self.slot_out(elem(xs));
        self.call_through(c, params, ret);
        self.sink().local_tee(res);
        if !any {
            self.sink().i32_eqz();
        }
        self.br_if(brk);
        self.next_pass(i, top);
        self.sink().local_get(res);
    }

    /// `map`, `and_then` and `map_err` on the `Option` or `Result` in `r`, of type `from`: on
    /// the variant `f` takes, the call on its payload (+1), whose result `map` and `map_err`
    /// wrap in that variant of `ty`; on the other, that variant rebuilt as a `ty`.
    fn enum_map(&mut self, b: Builtin, r: u32, c: u32, from: &Ty, (params, ret): (&[Ty], &Ty), ty: &Ty) {
        // `None` and `Ok` are variant 0, `Some` and `Err` variant 1.
        let (called, wrap) = match b {
            Builtin::OptionMap => (1, true),
            Builtin::OptionAndThen => (1, false),
            Builtin::ResultMap => (0, true),
            Builtin::ResultMapErr => (1, true),
            Builtin::ResultAndThen => (0, false),
            b => unreachable!("{b:?} on an enum"),
        };
        self.sink().local_get(r).i32_load(VARIANT).i32_const(called).i32_eq();
        self.open(Open::If, BlockType::Result(ValType::I32));
        let o = wrap.then(|| self.temp(ValType::I32));
        if let Some(o) = o {
            let desc = self.statics.desc(ty);
            self.sink().i32_const(desc as i32).i32_const(called);
            self.call_rt("rt_alloc");
            self.sink().local_tee(o);
        }
        self.sink().local_get(c);
        let t = &payload(self.m, from, called as u32)[0];
        self.payload_owned(r, t);
        self.call_through(c, params, ret);
        if let Some(o) = o {
            self.slot_in(ret);
            self.sink().i64_store(slot_at(ENUM_SLOTS)).local_get(o);
        }
        self.sink().else_();
        self.rebuild(r, from, ty, 1 - called);
        self.close();
    }

    /// Pushes the payload of the `Some`, `Ok` or `Err` in `r`, of type `t`, +1.
    fn payload_owned(&mut self, r: u32, t: &Ty) {
        self.sink().local_get(r).i64_load(slot_at(ENUM_SLOTS));
        self.slot_out(t);
        if is_ptr(t) {
            let v = self.temp(ValType::I32);
            self.sink().local_tee(v);
            self.inc_top();
            self.sink().local_get(v);
        }
    }

    /// Pushes a new reference to variant `v` of the `Option` or `Result` in `r`, of type
    /// `from`, as a value of type `to`, as `?` does: the pinned `None` of `to`; `r` itself if
    /// `to` is `from`; else a new object of `to`'s descriptor holding the payload, +1.
    fn rebuild(&mut self, r: u32, from: &Ty, to: &Ty, v: i32) {
        if let Ty::Option(_) = from {
            let none = self.statics.pinned(to, 0);
            self.sink().i32_const(none as i32);
        } else if from == to {
            self.sink().local_get(r);
            self.inc_top();
            self.sink().local_get(r);
        } else {
            let t = &payload(self.m, from, v as u32)[0];
            let o = self.temp(ValType::I32);
            let desc = self.statics.desc(to);
            self.sink().i32_const(desc as i32).i32_const(v);
            self.call_rt("rt_alloc");
            self.sink().local_tee(o);
            self.payload_owned(r, t);
            self.slot_in(t);
            self.sink().i64_store(slot_at(ENUM_SLOTS)).local_get(o);
        }
    }

    /// `base.f`: a new reference to the field's value. A base that is a local, or a field of
    /// one, is read in place; any other is evaluated and dropped after the read.
    fn field(&mut self, base: &ir::Expr, f: u32, ty: &Ty) {
        let b = if self.in_place(base) {
            self.borrow(base);
            None
        } else {
            self.expr(base);
            if base.ty == Ty::Never {
                return;
            }
            let b = self.temp(ValType::I32);
            self.sink().local_tee(b);
            Some(b)
        };
        self.sink().i64_load(slot_at(RECORD_SLOTS + 8 * u64::from(f)));
        self.slot_out(ty);
        if is_ptr(ty) {
            let v = self.temp(ValType::I32);
            self.sink().local_tee(v);
            self.inc_top();
            self.sink().local_get(v);
        }
        if let Some(b) = b {
            self.dec(b);
        }
    }

    /// The Wasm local of `e` if it is a read of a pointer local, to borrow in place.
    fn borrowed_local(&self, e: &ir::Expr) -> Option<u32> {
        match e.kind {
            ExprKind::Local(id) if is_ptr(&e.ty) => self.local(id),
            _ => None,
        }
    }

    /// Whether `e` is a local, or a field of one, so it can be read without a reference.
    fn in_place(&self, e: &ir::Expr) -> bool {
        match &e.kind {
            ExprKind::Local(id) => self.local(*id).is_some(),
            ExprKind::Field(base, _) => self.in_place(base),
            _ => false,
        }
    }

    /// Pushes the pointer `e`, which `in_place` accepts, holds, borrowed: no `rt_inc`.
    fn borrow(&mut self, e: &ir::Expr) {
        match &e.kind {
            ExprKind::Local(id) => {
                let l = self.local(*id).expect("in place");
                self.sink().local_get(l);
            }
            ExprKind::Field(base, f) => {
                self.borrow(base);
                self.sink()
                    .i64_load(slot_at(RECORD_SLOTS + 8 * u64::from(*f)))
                    .i32_wrap_i64();
            }
            _ => unreachable!("in place"),
        }
    }

    /// `match`: the scrutinee once, into a hidden local that the match owns until it ends (an
    /// exit from an arm releases it); then each arm in order: its pattern tested part by part,
    /// on failure a branch to the next arm; its bindings as new references, released at the
    /// arm's end like a scope's `let`s; its body.
    fn match_expr(&mut self, scrut: &ir::Expr, arms: &[ir::Arm], ty: &Ty) {
        if scrut.ty == Ty::Never {
            return self.expr(scrut);
        }
        let mark = self.owned.len();
        let s = match self.borrowed_local(scrut) {
            // A local scrutinee is read in place (C8b2): the tests and bindings read it before
            // any arm's body runs, and bindings hold references of their own.
            Some(l) => Some(l),
            None => {
                let s = val_type(&scrut.ty).map(|vt| self.temp(vt));
                self.expr(scrut);
                if let Some(s) = s {
                    self.sink().local_set(s);
                    if is_ptr(&scrut.ty) {
                        self.owned.push(s);
                    }
                }
                s
            }
        };
        let vt = val_type(ty);
        let end = self.open(Open::Block, vt.map_or(BlockType::Empty, BlockType::Result));
        for arm in arms {
            let next = self.open(Open::Block, BlockType::Empty);
            self.test(&arm.pat, &scrut.ty, s, &mut Vec::new(), next);
            let scope = self.owned.len();
            self.bind(&arm.pat, &scrut.ty, s, &mut Vec::new());
            self.expr(&arm.body);
            self.end_scope(scope);
            self.br(end);
            self.close();
        }
        // Exhaustive: some arm matched.
        self.sink().unreachable();
        self.close();
        self.end_scope(mark);
    }

    /// Pushes the part of the scrutinee in `s` that `path` (payload slot indexes, outermost
    /// first) leads to, of type `ty`, borrowed.
    fn part(&mut self, s: Option<u32>, path: &[u32], ty: &Ty) {
        let Some(s) = s else {
            return; // `()`, which only `_` and bindings match
        };
        self.sink().local_get(s);
        for (k, i) in path.iter().enumerate() {
            self.sink().i64_load(slot_at(ENUM_SLOTS + 8 * u64::from(*i)));
            if k + 1 < path.len() {
                self.sink().i32_wrap_i64();
            }
        }
        if !path.is_empty() {
            self.slot_out(ty);
        }
    }

    /// Branches to `fail` unless the part at `path`, of type `ty`, matches `p`, testing a
    /// variant's number before its payload's parts, left to right.
    fn test(&mut self, p: &ir::Pat, ty: &Ty, s: Option<u32>, path: &mut Vec<u32>, fail: u32) {
        match p {
            ir::Pat::Wild | ir::Pat::Bind(_) => return,
            ir::Pat::Int(n) => {
                self.part(s, path, ty);
                self.sink().i64_const(*n).i64_ne();
            }
            ir::Pat::Bool(b) => {
                self.part(s, path, ty);
                self.sink().i32_const(*b as i32).i32_ne();
            }
            ir::Pat::Str(lit) => {
                let a = self.statics.str(lit);
                self.sink().i32_const(STR_DESC as i32);
                self.part(s, path, ty);
                self.sink().i64_extend_i32_u().i64_const(i64::from(a));
                self.call_rt("rt_eq");
                self.sink().i32_eqz();
            }
            ir::Pat::Variant(v, ps) => {
                self.part(s, path, ty);
                self.sink().i32_load(VARIANT).i32_const(*v as i32).i32_ne();
                self.br_if(fail);
                for (i, (p, t)) in ps.iter().zip(payload(self.m, ty, *v)).enumerate() {
                    path.push(i as u32);
                    self.test(p, &t, s, path, fail);
                    path.pop();
                }
                return;
            }
        }
        self.br_if(fail);
    }

    /// Sets each binding in `p`, which matched, to a new reference to its part, left to right,
    /// and registers it in `owned`.
    fn bind(&mut self, p: &ir::Pat, ty: &Ty, s: Option<u32>, path: &mut Vec<u32>) {
        match p {
            ir::Pat::Bind(id) => {
                let Some(l) = self.local(*id) else { return };
                self.part(s, path, ty);
                self.sink().local_set(l);
                if is_ptr(ty) {
                    self.sink().local_get(l);
                    self.inc_top();
                    self.owned.push(l);
                }
            }
            ir::Pat::Variant(v, ps) => {
                for (i, (p, t)) in ps.iter().zip(payload(self.m, ty, *v)).enumerate() {
                    path.push(i as u32);
                    self.bind(p, &t, s, path);
                    path.pop();
                }
            }
            ir::Pat::Wild | ir::Pat::Int(_) | ir::Pat::Bool(_) | ir::Pat::Str(_) => {}
        }
    }

    /// `x?`: on `Some` or `Ok`, a new reference to the payload, and `x` dropped. On `None` or
    /// `Err`, every owned local released and the function returns a `None` or `Err` of its own
    /// return type: the pinned `None`; `x` itself if its type is the return type; else a new
    /// `Err` holding a reference to `x`'s error.
    fn try_expr(&mut self, x: &ir::Expr) {
        if x.ty == Ty::Never {
            return self.expr(x);
        }
        let (success, ty) = match &x.ty {
            Ty::Option(t) => (1, &**t),
            Ty::Result(t, _) => (0, &**t),
            t => unreachable!("`?` on {t:?}"),
        };
        // A local is read in place (C8b2): nothing runs between the read and the test.
        let borrowed = self.borrowed_local(x);
        let s = borrowed.unwrap_or_else(|| self.temp(ValType::I32));
        if borrowed.is_none() {
            self.expr(x);
            self.sink().local_set(s);
            self.owned.push(s);
        }
        self.sink().local_get(s).i32_load(VARIANT).i32_const(success).i32_ne();
        self.open(Open::If, BlockType::Empty);
        let r = self.temp(ValType::I32);
        let ret = self.ret;
        match &x.ty {
            Ty::Option(_) => {
                let none = self.statics.pinned(ret, 0);
                self.sink().i32_const(none as i32).local_set(r);
            }
            t if t == ret => {
                self.sink().local_get(s).local_tee(r);
                self.inc_top();
            }
            Ty::Result(_, err) => {
                if is_ptr(err) {
                    self.sink().local_get(s).i64_load(slot_at(ENUM_SLOTS)).i32_wrap_i64();
                    self.inc_top();
                }
                let desc = self.statics.desc(ret);
                self.sink().i32_const(desc as i32).i32_const(1);
                self.call_rt("rt_alloc");
                self.sink()
                    .local_tee(r)
                    .local_get(s)
                    .i64_load(slot_at(ENUM_SLOTS))
                    .i64_store(slot_at(ENUM_SLOTS));
            }
            t => unreachable!("`?` on {t:?}"),
        }
        self.release(0);
        self.sink().local_get(r).return_();
        self.close();
        self.sink().local_get(s).i64_load(slot_at(ENUM_SLOTS));
        self.slot_out(ty);
        if is_ptr(ty) {
            let v = self.temp(ValType::I32);
            self.sink().local_tee(v);
            self.inc_top();
            self.sink().local_get(v);
        }
        if borrowed.is_none() {
            self.owned.pop();
            self.dec(s);
        }
    }

    /// `a op b` on two `Float` from the stack: IEEE, as Rust's `f64`, so nothing traps. Wasm
    /// has no remainder on floats; the runtime's is Rust's `%`.
    fn float_op(&mut self, op: BinOp) {
        let i = match op {
            BinOp::FAdd => Instruction::F64Add,
            BinOp::FSub => Instruction::F64Sub,
            BinOp::FMul => Instruction::F64Mul,
            BinOp::FDiv => Instruction::F64Div,
            BinOp::FRem => return self.call_rt("rt_float_rem"),
            BinOp::FLt => Instruction::F64Lt,
            BinOp::FLe => Instruction::F64Le,
            BinOp::FGt => Instruction::F64Gt,
            BinOp::FGe => Instruction::F64Ge,
            BinOp::Eq => Instruction::F64Eq,
            BinOp::Ne => Instruction::F64Ne,
            op => unreachable!("{op:?} on Float"),
        };
        i.encode(&mut self.code);
    }

    /// A built-in on the scalar on the stack that is one Wasm instruction or two (`inline_builtin`).
    fn scalar_builtin(&mut self, b: Builtin) {
        let mut s = self.sink();
        match b {
            Builtin::IntToFloat => s.f64_convert_i64_s(),
            Builtin::FloatAbs => s.f64_abs(),
            Builtin::FloatFloor => s.f64_floor(),
            Builtin::FloatCeil => s.f64_ceil(),
            Builtin::FloatSqrt => s.f64_sqrt(),
            // Only NaN differs from itself.
            Builtin::FloatIsNan => {
                let x = self.temp(ValType::F64);
                self.sink().local_tee(x).local_get(x).f64_ne()
            }
            // `|x| < inf` is false for NaN, as `is_finite`; `|x| == inf` as `is_infinite`.
            Builtin::FloatIsFinite => s.f64_abs().f64_const(Ieee64::new(f64::INFINITY.to_bits())).f64_lt(),
            Builtin::FloatIsInfinite => s.f64_abs().f64_const(Ieee64::new(f64::INFINITY.to_bits())).f64_eq(),
            b => unreachable!("{b:?} is not inline"),
        };
    }

    /// `a op b` on two `Int` from the stack, with the interpreter's errors at `span`.
    fn int_op(&mut self, op: BinOp, span: Span) {
        let compare = match op {
            BinOp::Lt => Some(Instruction::I64LtS),
            BinOp::Le => Some(Instruction::I64LeS),
            BinOp::Gt => Some(Instruction::I64GtS),
            BinOp::Ge => Some(Instruction::I64GeS),
            BinOp::Eq => Some(Instruction::I64Eq),
            BinOp::Ne => Some(Instruction::I64Ne),
            _ => None,
        };
        if let Some(i) = compare {
            i.encode(&mut self.code);
            return;
        }
        let [a, b, r] = self.scratch();
        self.sink().local_set(b).local_set(a);
        match op {
            BinOp::Add | BinOp::Sub => {
                // Overflow when the result's sign differs from both operands' (`+`), or when
                // the operands' signs differ and the result's differs from a's (`-`).
                let mut s = self.sink();
                s.local_get(a).local_get(b);
                if op == BinOp::Add {
                    s.i64_add().local_set(r);
                    s.local_get(a)
                        .local_get(r)
                        .i64_xor()
                        .local_get(b)
                        .local_get(r)
                        .i64_xor();
                } else {
                    s.i64_sub().local_set(r);
                    s.local_get(a)
                        .local_get(b)
                        .i64_xor()
                        .local_get(a)
                        .local_get(r)
                        .i64_xor();
                }
                s.i64_and().i64_const(0).i64_lt_s();
                self.trap_if(span, OVERFLOW, a, b);
            }
            BinOp::Mul => {
                // Wasm has no overflow flag or 128-bit product: overflow when a != 0 and
                // r / a != b, except that a = -1, b = MIN overflows, and that division traps.
                let mut s = self.sink();
                s.local_get(a).local_get(b).i64_mul().local_set(r);
                s.local_get(a).i64_const(0).i64_ne();
                s.if_(BlockType::Result(ValType::I32));
                s.local_get(a).i64_const(-1).i64_eq();
                s.if_(BlockType::Result(ValType::I32));
                s.local_get(b).i64_const(i64::MIN).i64_eq();
                s.else_();
                s.local_get(r).local_get(a).i64_div_s().local_get(b).i64_ne();
                s.end();
                s.else_().i32_const(0).end();
                self.trap_if(span, OVERFLOW, a, b);
            }
            BinOp::Div | BinOp::Rem => {
                // Wasm's `div_s` traps on 0 and on MIN / -1, and `rem_s` gives 0 for MIN % -1:
                // the interpreter's checks come first, in its order.
                self.sink().local_get(b).i64_eqz();
                self.trap_if(span, DIV_ZERO, a, b);
                let mut s = self.sink();
                s.local_get(a).i64_const(i64::MIN).i64_eq();
                s.local_get(b).i64_const(-1).i64_eq().i32_and();
                self.trap_if(span, OVERFLOW, a, b);
                let mut s = self.sink();
                s.local_get(a).local_get(b);
                if op == BinOp::Div {
                    s.i64_div_s().local_set(r);
                } else {
                    s.i64_rem_s().local_set(r);
                }
            }
            op => unreachable!("checked: {op:?} on Int"),
        }
        self.sink().local_get(r);
    }

    /// Traps at a new site when the `i32` on the stack is true, passing locals `a` and `b`.
    fn trap_if(&mut self, span: Span, template: &'static str, a: u32, b: u32) {
        let site = self.site(span, template);
        let trap = self.rt.export("rt_trap");
        let mut s = self.sink();
        s.if_(BlockType::Empty);
        s.i32_const(site as i32)
            .local_get(a)
            .local_get(b)
            .call(trap)
            .unreachable();
        s.end();
    }
}

/// The body of the runtime's `kio_main`: register the descriptor table, run `main` with the
/// handle of grant `i` (`rt_grant(i, kind)`) as its parameter `i`, return its value as a slot.
fn entry(rt: &Runtime, table: u32, index: u32, main: &ir::Func) -> Function {
    let mut f = Function::new([]);
    f.instructions().i32_const(table as i32).call(rt.export("rt_init"));
    for (i, l) in main.locals[..main.params as usize].iter().enumerate() {
        let Ty::Cap(kind) = l.ty else {
            unreachable!("main takes capabilities only")
        };
        f.instructions()
            .i32_const(i as i32)
            .i32_const(kind.0 as i32)
            .call(rt.export("rt_grant"));
    }
    f.instructions().call(index);
    match val_type(&main.ret) {
        Some(ValType::I64) => {}
        Some(ValType::F64) => {
            f.instructions().i64_reinterpret_f64();
        }
        Some(_) => {
            f.instructions().i64_extend_i32_u();
        }
        None => {
            f.instructions().i64_const(0);
        }
    }
    f.instructions().end();
    f
}

/// The body of `kio_main_desc`: the descriptor of main's value, which the runtime's `run`
/// displays (a `Result` that is `Err` as a failure showing its error).
fn entry_desc(statics: &mut Statics, ret: &Ty) -> Function {
    let desc = statics.desc(ret);
    let mut f = Function::new([]);
    f.instructions().i32_const(desc as i32).end();
    f
}

/// The table entry of function `f` used as a value: takes the function object and `f`'s own
/// parameters, and calls `f` with each capture in the object first. A call through a value
/// passes every argument owned and the object borrowed, so for `f`'s borrowed parameters
/// (`lent`, C8b4) the wrapper adapts: a borrowed capture is passed without a new reference
/// (the caller holds the object across the call), and a borrowed parameter's argument is
/// dropped after the call.
fn wrapper(rt: &Runtime, m: &Module, f: ir::FuncId, captures: usize, lent: &[bool]) -> (FuncSig, Function) {
    let func = &m.funcs[f.0 as usize];
    let (caps, params) = func.locals[..func.params as usize].split_at(captures);
    let params: Vec<Ty> = params.iter().map(|l| l.ty.clone()).collect();
    let sig = value_sig(&params, &func.ret);
    // Two locals after the parameters for the inline `inc`: a pointer and its count.
    let (p, r) = (sig.0.len() as u32, sig.0.len() as u32 + 1);
    let mut body = Function::new([(2, ValType::I32)]);
    let mut s = body.instructions();
    for (i, l) in caps.iter().enumerate() {
        let at = slot_at(ENUM_SLOTS + 8 * i as u64);
        if is_ptr(&l.ty) && !lent[i] {
            s.local_get(0).i64_load(at).i32_wrap_i64().local_set(p);
            inc(&mut s, rt, p, r);
        }
        if val_type(&l.ty).is_some() {
            s.local_get(0).i64_load(at);
            slot_out(&mut s, &l.ty);
        }
    }
    for i in 1..sig.0.len() as u32 {
        s.local_get(i);
    }
    s.call(rt.funcs + f.0);
    // The result stays on the stack under the drops.
    let mut w = 0;
    for (l, &lent) in func.locals[captures..func.params as usize]
        .iter()
        .zip(&lent[captures..])
    {
        if val_type(&l.ty).is_some() {
            w += 1;
            if lent {
                dec(&mut s, rt, w, r);
            }
        }
    }
    s.end();
    (sig, body)
}

/// Re-encodes the runtime with the program appended.
struct Append<'a> {
    rt: &'a Runtime,
    /// The program's function types, each once, and each function's type index and body.
    types: Vec<FuncSig>,
    funcs: Vec<(u32, Function)>,
    entry: Function,
    entry_desc: Function,
    /// The program's data: its address, at the old end of memory, and its bytes.
    data: (u32, Vec<u8>),
    /// The functions of the program's table entries, which follow the runtime's in table 0.
    elems: Vec<u32>,
}

impl Append<'_> {
    fn extra_pages(&self) -> u32 {
        (self.data.1.len() as u32).div_ceil(PAGE)
    }
}

type ReResult = std::result::Result<(), reencode::Error<std::convert::Infallible>>;

impl Reencode for Append<'_> {
    type Error = std::convert::Infallible;

    fn parse_type_section(&mut self, types: &mut TypeSection, s: wasmparser::TypeSectionReader<'_>) -> ReResult {
        reencode::utils::parse_type_section(self, types, s)?;
        for (params, results) in &self.types {
            types.ty().function(params.clone(), results.clone());
        }
        Ok(())
    }

    fn parse_function_section(
        &mut self,
        funcs: &mut wasm_encoder::FunctionSection,
        s: wasmparser::FunctionSectionReader<'_>,
    ) -> ReResult {
        reencode::utils::parse_function_section(self, funcs, s)?;
        for (ty, _) in &self.funcs {
            funcs.function(*ty);
        }
        Ok(())
    }

    fn parse_table_section(&mut self, tables: &mut TableSection, s: wasmparser::TableSectionReader<'_>) -> ReResult {
        for (i, t) in s.into_iter().enumerate() {
            let t = t?;
            let mut ty = self.table_type(t.ty)?;
            if i == 0 {
                let n = self.elems.len() as u64;
                ty.minimum += n;
                ty.maximum = ty.maximum.map(|m| m + n);
            }
            match t.init {
                wasmparser::TableInit::RefNull => tables.table(ty),
                wasmparser::TableInit::Expr(e) => tables.table_with_init(ty, &self.const_expr(e)?),
            };
        }
        Ok(())
    }

    fn parse_element_section(
        &mut self,
        elems: &mut ElementSection,
        s: wasmparser::ElementSectionReader<'_>,
    ) -> ReResult {
        reencode::utils::parse_element_section(self, elems, s)?;
        if !self.elems.is_empty() {
            let at = ConstExpr::i32_const(self.rt.table as i32);
            elems.active(Some(0), &at, Elements::Functions(self.elems.as_slice().into()));
        }
        Ok(())
    }

    fn parse_memory_section(&mut self, mems: &mut MemorySection, s: wasmparser::MemorySectionReader<'_>) -> ReResult {
        for m in s {
            let mut m = self.memory_type(m?)?;
            m.minimum += u64::from(self.extra_pages());
            mems.memory(m);
        }
        Ok(())
    }

    fn parse_code_section(&mut self, code: &mut CodeSection, s: wasmparser::CodeSectionReader<'_>) -> ReResult {
        let (main, desc) = (self.rt.export("kio_main"), self.rt.export("kio_main_desc"));
        for (i, body) in s.into_iter().enumerate() {
            let body = body?;
            match self.rt.imported + i as u32 {
                f if f == main => code.function(&self.entry),
                f if f == desc => code.function(&self.entry_desc),
                _ => code.raw(body.as_bytes()),
            };
        }
        for (_, f) in &self.funcs {
            code.function(f);
        }
        Ok(())
    }

    fn parse_data_section(&mut self, data: &mut DataSection, s: wasmparser::DataSectionReader<'_>) -> ReResult {
        reencode::utils::parse_data_section(self, data, s)?;
        let (addr, bytes) = &self.data;
        data.active(0, &ConstExpr::i32_const(*addr as i32), bytes.iter().copied());
        Ok(())
    }

    fn data_count(&mut self, count: u32) -> std::result::Result<u32, reencode::Error<Self::Error>> {
        Ok(count + 1)
    }
}

#[cfg(test)]
mod tests;
