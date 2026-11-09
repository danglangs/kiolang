//! Codegen: an `ir::Module` to a component in the `compiled` world (caps.wit).
//!
//! The program is appended to runtime.wasm, a complete core module (docs/codegen.md,
//! "Pipeline"). Nothing is inserted before what the runtime already has, so no index moves:
//! new types, functions and data go after the runtime's, and the runtime's two placeholder
//! entry functions keep their indexes and get new bodies. wit-component then wraps the one
//! module as a component.

mod data;

use anyhow::{anyhow, Context, Result};
use data::Statics;
use lang::ir::{self, BinOp, Builtin, ExprKind, Module, Mut, StmtKind, Ty, UnOp};
use lang::Span;
use std::collections::HashMap;
use std::fmt::Debug;
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

/// Compiles `m`. Panics with `unimplemented` on IR that codegen does not handle yet.
pub fn compile(m: &Module) -> Result<Wasm> {
    let rt = Runtime::scan(RUNTIME)?;
    let data_addr = rt.pages * PAGE;
    let mut statics = Statics::new(m, data_addr, rt.table);
    let mut sites = Vec::new();
    let mut types: Vec<FuncSig> = Vec::new();
    let mut funcs = Vec::new();
    for f in &m.funcs {
        let (sig, body) = func(&rt, m, &mut statics, &mut types, f, &mut sites);
        funcs.push((rt.types + type_index(&mut types, sig), body));
    }
    // A wrapper per function used as a value, after the program's functions, in table order.
    let mut elems = Vec::new();
    for &(f, captures) in statics.elems() {
        let (sig, body) = wrapper(&rt, m, f, captures);
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
    let mut module = wasm_encoder::Module::new();
    app.parse_core_module(&mut module, Parser::new(0), RUNTIME)
        .map_err(|e| anyhow!("re-encoding the runtime: {e}"))?;
    let core = module.finish();
    let component = wit_component::ComponentEncoder::default()
        .module(&core)?
        .validate(true)
        .encode()
        .context("wrapping the module as a component")?;
    Ok(Wasm { component, sites })
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
        };
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
                _ => {}
            }
        }
        Ok(rt)
    }

    fn export(&self, name: &str) -> u32 {
        self.exports[name]
    }
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
        Ty::Str | Ty::Bytes | Ty::List(_) | Ty::Map(..) => Some(ValType::I32),
        Ty::Struct(_) | Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) | Ty::Fn(..) => Some(ValType::I32),
        // A handle in the component's table, never counted.
        Ty::Cap(_) => Some(ValType::I32),
        Ty::Unit | Ty::Never => None,
        t => unimplemented!("codegen: type {}", node(t)),
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

/// The variant name of an IR node, for `unimplemented`.
fn node(n: &impl Debug) -> String {
    let s = format!("{n:?}");
    s.split(['(', ' ', '{']).next().unwrap_or_default().to_string()
}

fn func<'m>(
    rt: &Runtime,
    m: &'m Module,
    statics: &mut Statics<'m>,
    types: &mut Vec<FuncSig>,
    f: &'m ir::Func,
    sites: &mut Vec<Site>,
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
    // The parameters are owned by the call, as if bound by `let` around the body.
    let owned = (0..f.params as usize)
        .filter(|&i| is_ptr(&f.locals[i].ty))
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
        scratch: None,
        owned,
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
    /// Three `i64` locals an integer check uses, after its operands are on the stack.
    scratch: Option<[u32; 3]>,
    /// The Wasm locals that hold an owned pointer at this point of the code, outermost scope
    /// first: parameters, then each `let` once it has run, and hidden locals holding the
    /// operands evaluated so far. A scope's end releases its part; an exit releases the part
    /// of every scope it leaves.
    owned: Vec<u32>,
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
        let f = self.rt.export(name);
        self.sink().call(f);
    }

    /// Drops the reference in local `l`.
    fn dec(&mut self, l: u32) {
        self.sink().local_get(l);
        self.call_rt("rt_dec");
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
        let mark = self.owned.len();
        let mut temps = Vec::new();
        for x in xs {
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
        (mark, temps)
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
    fn borrowing(&mut self, xs: &[&ir::Expr], f: impl FnOnce(&mut Self, &[Option<u32>])) {
        let (mark, temps) = self.operands(xs);
        f(self, &temps);
        for (x, t) in xs.iter().zip(&temps) {
            if let (true, Some(t)) = (is_ptr(&x.ty), t) {
                self.dec(*t);
            }
        }
        self.owned.truncate(mark);
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

    fn temp(&mut self, t: ValType) -> u32 {
        self.decls.push(t);
        self.first_decl + self.decls.len() as u32 - 1
    }

    fn scratch(&mut self) -> [u32; 3] {
        if self.scratch.is_none() {
            self.scratch = Some([0; 3].map(|_| self.temp(ValType::I64)));
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
                match op {
                    // The value first, then the old one dropped.
                    None => {
                        self.expr(value);
                        if is_ptr(&value.ty) {
                            self.dec(l);
                        }
                    }
                    Some(BinOp::Concat) => {
                        // `x + value`, borrowing both, as the runtime does; then the old `x`
                        // and the value are dropped.
                        let v = self.temp(ValType::I32);
                        self.expr(value);
                        self.sink().local_set(v).local_get(l).local_get(v);
                        self.call_rt("rt_concat");
                        self.dec(v);
                        self.dec(l);
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
                    self.call_rt("rt_dec");
                } else if val_type(&e.ty).is_some() {
                    self.sink().drop();
                }
            }
        }
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
        self.expr(xs);
        self.sink().local_set(list);
        self.owned.push(list);
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
            self.sink().local_get(list).local_get(i).i32_const(site as i32);
            self.call_rt("rt_list_get");
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
                    // `old + v`, borrowing both; then both are dropped.
                    let old = self.temp(ValType::I32);
                    self.sink().i32_wrap_i64().local_tee(old).local_get(v);
                    self.call_rt("rt_concat");
                    self.sink().i64_extend_i32_u().i64_store(SLOT);
                    self.dec(v);
                    self.dec(old);
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
        let (mark, temps) = self.operands(&xs);
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
            self.sink().i32_const(site as i32);
            self.call_rt("rt_bytes_push");
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
                if is_ptr(&k.ty) {
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
        self.sink().local_get(root);
        self.call_rt("rt_unique");
        self.sink().local_set(root);
        if n == 0 {
            return (root, None);
        }
        let (cur, a) = (self.temp(ValType::I32), self.temp(ValType::I32));
        self.sink().local_get(root).local_set(cur);
        for step in &place.steps[..n] {
            self.step_slot(step, cur, a, temps, sites, k);
            self.sink().local_get(a).local_get(a).i64_load(SLOT).i32_wrap_i64();
            self.call_rt("rt_unique");
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
                let i = temps[*k].expect("an Int index");
                self.sink().local_get(cur).local_get(i).i32_const(sites[*k] as i32);
                self.call_rt("rt_list_at");
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
                    self.sink().local_get(l);
                    if is_ptr(&e.ty) {
                        // A copy: the local keeps its own reference.
                        self.call_rt("rt_inc");
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
                    (op, t) => unimplemented!("codegen: {op:?} on {}", node(t)),
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
            ExprKind::Call(f, args) => {
                // The callee takes the arguments.
                let args: Vec<&ir::Expr> = args.iter().collect();
                let (mark, temps) = self.operands(&args);
                for (x, t) in args.iter().zip(temps) {
                    self.get(x, t);
                }
                self.owned.truncate(mark);
                let index = self.rt.funcs + f.0;
                self.sink().call(index);
            }
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
                    Builtin::OptionMap
                    | Builtin::OptionAndThen
                    | Builtin::ResultMap
                    | Builtin::ResultMapErr
                    | Builtin::ResultAndThen
                    | Builtin::ListMap
                    | Builtin::ListFilter
                    | Builtin::ListAny
                    | Builtin::ListAll => unimplemented!("codegen: builtin {b:?}"),
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
                self.rt_op("rt_bytes_get", &[&**base, &**i], Some(site));
            }
            ExprKind::Index(base, i) => {
                // The element is +1, as a slot.
                let site = self.site(e.span, OUT_OF_BOUNDS);
                self.rt_op("rt_list_get", &[&**base, &**i], Some(site));
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
            k => unimplemented!("codegen: expression {}", node(k)),
        }
        if e.ty == Ty::Never {
            self.sink().unreachable();
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
        self.sink().i32_const(desc as i32).i32_const(variant as i32);
        self.call_rt("rt_alloc");
        self.sink().local_set(o);
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

    /// A closure lifted to `f`: a new function object holding a copy (+1) of each captured
    /// local in its slots, which the object owns until it is freed. Nothing here can exit, so
    /// the object needs no place in `owned` while it is filled.
    fn closure(&mut self, f: ir::FuncId, captured: &[ir::LocalId]) {
        let (desc, elem) = self.statics.func(f, Some(captured.len()));
        let o = self.temp(ValType::I32);
        self.sink().i32_const(desc as i32).i32_const(elem as i32);
        self.call_rt("rt_alloc");
        self.sink().local_set(o);
        let m = self.m;
        for (i, (id, l)) in captured.iter().zip(&m.funcs[f.0 as usize].locals).enumerate() {
            let Some(x) = self.local(*id) else { continue };
            if is_ptr(&l.ty) {
                self.sink().local_get(x);
                self.call_rt("rt_inc");
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
        let (mark, temps) = self.operands(&xs);
        let c = temps[0].expect("a fn value");
        self.sink().local_get(c);
        for (x, t) in args.iter().zip(&temps[1..]) {
            self.get(x, *t);
        }
        self.owned.truncate(mark);
        let ty = self.rt.types + type_index(self.types, value_sig(params, ret));
        self.sink().local_get(c).i32_load(VARIANT).call_indirect(0, ty);
        self.dec(c);
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
            self.call_rt("rt_inc");
            self.sink().local_get(v);
        }
        if let Some(b) = b {
            self.dec(b);
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
        let s = val_type(&scrut.ty).map(|vt| self.temp(vt));
        self.expr(scrut);
        if let Some(s) = s {
            self.sink().local_set(s);
            if is_ptr(&scrut.ty) {
                self.owned.push(s);
            }
        }
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
                    self.call_rt("rt_inc");
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
        let s = self.temp(ValType::I32);
        self.expr(x);
        self.sink().local_set(s);
        self.owned.push(s);
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
                self.call_rt("rt_inc");
            }
            Ty::Result(_, err) => {
                if is_ptr(err) {
                    self.sink().local_get(s).i64_load(slot_at(ENUM_SLOTS)).i32_wrap_i64();
                    self.call_rt("rt_inc");
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
            self.call_rt("rt_inc");
            self.sink().local_get(v);
        }
        self.owned.pop();
        self.dec(s);
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
            op => unimplemented!("codegen: {op:?} on Int"),
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
/// parameters, and calls `f` with a new reference to each capture in the object first.
fn wrapper(rt: &Runtime, m: &Module, f: ir::FuncId, captures: usize) -> (FuncSig, Function) {
    let func = &m.funcs[f.0 as usize];
    let (caps, params) = func.locals[..func.params as usize].split_at(captures);
    let params: Vec<Ty> = params.iter().map(|l| l.ty.clone()).collect();
    let sig = value_sig(&params, &func.ret);
    let mut body = Function::new([]);
    let mut s = body.instructions();
    for (i, l) in caps.iter().enumerate() {
        let at = slot_at(ENUM_SLOTS + 8 * i as u64);
        if is_ptr(&l.ty) {
            s.local_get(0).i64_load(at).i32_wrap_i64().call(rt.export("rt_inc"));
        }
        if val_type(&l.ty).is_some() {
            s.local_get(0).i64_load(at);
            slot_out(&mut s, &l.ty);
        }
    }
    for i in 1..sig.0.len() as u32 {
        s.local_get(i);
    }
    s.call(rt.funcs + f.0).end();
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
