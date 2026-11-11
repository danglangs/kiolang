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
    let sh = Shapes::new(m);
    let data_addr = rt.pages * PAGE;
    let mut statics = Statics::new(m, data_addr, rt.table);
    let mut sites = Vec::new();
    let mut types: Vec<FuncSig> = Vec::new();
    let mut funcs = Vec::new();
    let lent = moves::borrowed(m, &|t| sh.owns(t));
    for (i, f) in m.funcs.iter().enumerate() {
        let (sig, body) = func(&rt, &sh, &mut statics, &mut types, f, &mut sites, (&lent, i));
        funcs.push((rt.types + type_index(&mut types, sig), body));
    }
    // A wrapper per function used as a value, after the program's functions, in table order.
    let mut elems = Vec::new();
    for &(f, captures) in statics.elems() {
        let (sig, body) = wrapper(&rt, &sh, f, captures, &lent[f.0 as usize]);
        elems.push(rt.funcs + funcs.len() as u32);
        funcs.push((rt.types + type_index(&mut types, sig), body));
    }
    let main = &m.funcs[m.main.0 as usize];
    let desc = statics.desc(&main.ret);
    let (data, table) = statics.finish();
    let main_index = rt.funcs + m.main.0;
    let mut app = Append {
        rt: &rt,
        types,
        funcs,
        entry: entry(&rt, &sh, table, main_index, main, desc),
        entry_desc: entry_desc(desc),
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
    /// The address of the word the `_u` operations leave an `Option`'s payload in (`rt_scratch`).
    scratch: u32,
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
            scratch: 0,
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
                    for (name, at) in [
                        ("rt_heap_lists", &mut rt.lists),
                        ("rt_live_count", &mut rt.live),
                        ("rt_scratch", &mut rt.scratch),
                    ] {
                        if rt.exports.get(name) == Some(&index) {
                            *at = constant(&body).with_context(|| format!("{name} is not one i32.const"))?;
                        }
                    }
                }
                _ => {}
            }
        }
        anyhow::ensure!(
            rt.lists != 0 && rt.live != 0 && rt.scratch != 0,
            "the runtime has no rt_heap_lists, rt_live_count or rt_scratch"
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
fn value_sig(sh: &Shapes, params: &[Ty], ret: &Ty) -> FuncSig {
    let mut ps = vec![ValType::I32];
    for p in params {
        ps.extend(sh.types(p));
    }
    (ps, sh.types(ret))
}

/// How a kio type is held in Wasm: no value (`()`, `!`), one, an unboxed enum's tag (`i32`)
/// and payload (an `i64` slot) (C8c1), or an unboxed struct's fields, one value each (C8c2).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Rep {
    None,
    One(ValType),
    Two,
    Fields(ir::StructId),
}

/// An unboxed enum's two values.
const TWO: [ValType; 2] = [ValType::I32, ValType::I64];

/// The most fields an unboxed struct has (`Val::Fields`).
const MAX_FIELDS: usize = 4;

/// Where a value is held: nowhere (`()`, `!`), in one Wasm local, in an unboxed enum's tag and
/// payload locals, or in an unboxed struct's field locals (one per field, by declaration index;
/// a `()` field's entry is unused).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Val {
    None,
    One(u32),
    Two(u32, u32),
    Fields(ir::StructId, [u32; MAX_FIELDS]),
}

/// A local holding an owned reference: a pointer, or an unboxed enum's payload (`slot`: an
/// `i64`), whose `dec` first tests the tag local against `test`'s mask when only some variants
/// carry a pointer.
#[derive(Clone, Copy, Debug)]
struct Owned {
    local: u32,
    slot: bool,
    test: Option<(u32, u64)>,
}

impl Owned {
    /// A pointer in `i32` local `l`.
    fn ptr(l: u32) -> Owned {
        Owned {
            local: l,
            slot: false,
            test: None,
        }
    }
}

/// An unboxed enum's variants: how many, which carry a payload (bit `v` for variant `v`), which
/// carry a pointer, and which carry a `()` (a slot that is always 0).
#[derive(Clone, Copy, Debug)]
struct Masks {
    n: u32,
    payload: u64,
    ptr: u64,
    unit: u64,
}

impl Masks {
    fn all(self) -> u64 {
        if self.n >= 64 {
            u64::MAX
        } else {
            (1 << self.n) - 1
        }
    }

    /// The mask a count change on the payload must test the tag against, if any: some variant
    /// carries a pointer and another a non-zero scalar. Otherwise the payload is a pointer or 0
    /// (a payload-free or `()` variant's), and `inc`/`dec` of 0 do nothing.
    fn tested(self) -> Option<u64> {
        (self.ptr != 0 && self.ptr != self.payload & !self.unit).then_some(self.ptr)
    }
}

/// How each type is held: `Option`, `Result` and every user enum whose variants carry at most
/// one slot (`small`) are unboxed outside slots (C8c1); so is every struct of at most
/// `MAX_FIELDS` fields (`flat`), one Wasm value per field (C8c2).
struct Shapes<'m> {
    m: &'m Module,
    small: Vec<bool>,
    flat: Vec<bool>,
}

impl<'m> Shapes<'m> {
    fn new(m: &'m Module) -> Self {
        let small = m
            .enums
            .iter()
            .map(|e| e.variants.len() <= 64 && e.variants.iter().all(|(_, ts)| ts.len() <= 1))
            .collect();
        let flat = m.structs.iter().map(|s| s.fields.len() <= MAX_FIELDS).collect();
        Shapes { m, small, flat }
    }

    /// Whether a value of `t` is a tag and a payload rather than a pointer.
    fn unboxed(&self, t: &Ty) -> bool {
        match t {
            Ty::Option(_) | Ty::Result(..) => true,
            Ty::Enum(id) => self.small[id.0 as usize],
            _ => false,
        }
    }

    /// The struct whose fields a value of `t` is held as, if `t` is an unboxed struct.
    fn fields(&self, t: &Ty) -> Option<ir::StructId> {
        match t {
            Ty::Struct(id) if self.flat[id.0 as usize] => Some(*id),
            _ => None,
        }
    }

    /// The field types of struct `id`.
    fn field_tys(&self, id: ir::StructId) -> impl DoubleEndedIterator<Item = &'m Ty> + ExactSizeIterator {
        self.m.structs[id.0 as usize].fields.iter().map(|(_, t)| t)
    }

    fn rep(&self, t: &Ty) -> Rep {
        match t {
            Ty::Int => Rep::One(ValType::I64),
            Ty::Bool => Rep::One(ValType::I32),
            Ty::Float => Rep::One(ValType::F64),
            Ty::Struct(id) if self.flat[id.0 as usize] => Rep::Fields(*id),
            Ty::Str | Ty::Bytes | Ty::List(_) | Ty::Map(..) | Ty::HashMap(..) | Ty::Struct(_) | Ty::Fn(..) => {
                Rep::One(ValType::I32)
            }
            Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) if self.unboxed(t) => Rep::Two,
            Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) => Rep::One(ValType::I32),
            // A handle in the component's table, never counted.
            Ty::Cap(_) => Rep::One(ValType::I32),
            Ty::Unit | Ty::Never => Rep::None,
        }
    }

    /// How a field of an unboxed struct is held: a scalar as its value, anything else (a pointer,
    /// an enum, a struct) as the `i32` pointer its slot would hold, so one level is unboxed.
    fn field_rep(&self, t: &Ty) -> Rep {
        match t {
            Ty::Int | Ty::Bool | Ty::Float | Ty::Unit | Ty::Never => self.rep(t),
            _ => Rep::One(ValType::I32),
        }
    }

    /// The Wasm value types a value of `t` is held as.
    fn types(&self, t: &Ty) -> Vec<ValType> {
        match self.rep(t) {
            Rep::None => vec![],
            Rep::One(vt) => vec![vt],
            Rep::Two => TWO.to_vec(),
            Rep::Fields(id) => self
                .field_tys(id)
                .filter_map(|t| match self.field_rep(t) {
                    Rep::One(vt) => Some(vt),
                    _ => None,
                })
                .collect(),
        }
    }

    /// Whether a value of `t` is one pointer to a counted object.
    fn is_ptr(&self, t: &Ty) -> bool {
        slot_ptr(t) && !self.unboxed(t) && self.fields(t).is_none()
    }

    /// Whether a value of `t` holds a reference: a pointer, an unboxed enum a variant of which
    /// carries one, or an unboxed struct a field of which does.
    fn owns(&self, t: &Ty) -> bool {
        self.is_ptr(t)
            || (self.unboxed(t) && self.masks(t).ptr != 0)
            || self.fields(t).is_some_and(|id| self.field_tys(id).any(slot_ptr))
    }

    /// Whether a local of type `t` is read in place where codegen borrows: it holds a pointer,
    /// or an unboxed enum or struct.
    fn lendable(&self, t: &Ty) -> bool {
        self.is_ptr(t) || self.unboxed(t) || self.fields(t).is_some()
    }

    /// The variants of unboxed enum type `t`.
    fn masks(&self, t: &Ty) -> Masks {
        let bit = |v: usize, ts: &[Ty]| match ts {
            [t] => (1 << v, u64::from(slot_ptr(t)) << v, u64::from(*t == Ty::Unit) << v),
            _ => (0, 0, 0),
        };
        let (n, bits): (usize, Vec<(u64, u64, u64)>) = match t {
            Ty::Option(x) => (2, vec![(0, 0, 0), bit(1, &[(**x).clone()])]),
            Ty::Result(x, e) => (2, vec![bit(0, &[(**x).clone()]), bit(1, &[(**e).clone()])]),
            Ty::Enum(id) => {
                let vs = &self.m.enums[id.0 as usize].variants;
                (vs.len(), vs.iter().enumerate().map(|(v, (_, ts))| bit(v, ts)).collect())
            }
            t => unreachable!("{t:?} is not an unboxed enum"),
        };
        let (payload, ptr, unit) = bits
            .iter()
            .fold((0, 0, 0), |(p, q, u), &(a, b, c)| (p | a, q | b, u | c));
        Masks {
            n: n as u32,
            payload,
            ptr,
            unit,
        }
    }
}

/// Whether a slot holding a value of `t` is a pointer to a counted object (an enum is boxed
/// in a slot).
fn slot_ptr(t: &Ty) -> bool {
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
            | Builtin::OptionUnwrapOrElse
            | Builtin::ResultUnwrapOrElse
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

/// The largest block generated code pops (runtime/src/heap.rs `SMALL`): classes of
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

/// Turns a slot on the stack into a value held as `rep`, which is not `Two` (`FuncGen::slot_out`
/// unboxes).
fn slot_out(s: &mut InstructionSink, rep: Rep) {
    match rep {
        Rep::One(ValType::I64) => {}
        Rep::One(ValType::F64) => {
            s.f64_reinterpret_i64();
        }
        Rep::One(_) => {
            s.i32_wrap_i64();
        }
        Rep::None => {
            s.drop();
        }
        Rep::Two | Rep::Fields(_) => unreachable!("an unboxed value is unboxed by FuncGen"),
    }
}

/// Unboxes the struct object (of unboxed struct `id`) in `i32` local `o` (C8c2): pushes each
/// field in order, a pointer field gaining a reference when `own`. `p` and `r` are scratch
/// `i32` locals; the object keeps its own reference.
fn unbox_struct(
    s: &mut InstructionSink,
    rt: &Runtime,
    sh: &Shapes,
    id: ir::StructId,
    o: u32,
    own: bool,
    [p, r]: [u32; 2],
) {
    for (i, t) in sh.field_tys(id).enumerate() {
        let rep = sh.field_rep(t);
        if rep == Rep::None {
            continue;
        }
        s.local_get(o).i64_load(slot_at(RECORD_SLOTS + 8 * i as u64));
        slot_out(s, rep);
        if own && slot_ptr(t) {
            s.local_tee(p);
            inc(s, rt, p, r);
        }
    }
}

/// The locals of a value of unboxed struct `id`: `next` gives one for each field that has a
/// value (in field order, with its type); a `()` field's entry is unused.
fn field_locals(sh: &Shapes, id: ir::StructId, mut next: impl FnMut(ValType) -> u32) -> [u32; MAX_FIELDS] {
    let mut fl = [u32::MAX; MAX_FIELDS];
    for (i, t) in sh.field_tys(id).enumerate() {
        if let Rep::One(vt) = sh.field_rep(t) {
            fl[i] = next(vt);
        }
    }
    fl
}

/// Boxes the fields of unboxed struct `id` held in locals `fl` into the object in `i32` local
/// `o`, which `rt_alloc` or `FuncGen::alloc` made with its slots unwritten or zero: each field
/// as a slot (a `()` field's stays 0), the object taking the fields' references.
fn store_fields(s: &mut InstructionSink, sh: &Shapes, id: ir::StructId, fl: &[u32; MAX_FIELDS], o: u32) {
    for (i, t) in sh.field_tys(id).enumerate() {
        let rep = sh.field_rep(t);
        if rep == Rep::None {
            continue;
        }
        s.local_get(o).local_get(fl[i]);
        match rep {
            Rep::One(ValType::I64) => {}
            Rep::One(ValType::F64) => {
                s.i64_reinterpret_f64();
            }
            _ => {
                s.i64_extend_i32_u();
            }
        }
        s.i64_store(slot_at(RECORD_SLOTS + 8 * i as u64));
    }
}

/// Pushes whether the variant in `i32` local `tag` has its bit in `mask` (not every bit).
fn in_mask(s: &mut InstructionSink, tag: u32, mask: u64) {
    if mask.count_ones() == 1 {
        s.local_get(tag).i32_const(mask.trailing_zeros() as i32).i32_eq();
    } else {
        s.i64_const(mask as i64)
            .local_get(tag)
            .i64_extend_i32_u()
            .i64_shr_u()
            .i32_wrap_i64()
            .i32_const(1)
            .i32_and();
    }
}

/// Unboxes the enum object in `i32` local `p` (C8c1): pushes its tag, then its payload (0 for
/// a payload-free variant), which gains a reference when `own` and its variant carries a
/// pointer. `tag`, `pay` (an `i64`), `q` and `r` are scratch locals.
fn unbox(s: &mut InstructionSink, rt: &Runtime, m: Masks, p: u32, own: bool, [tag, pay, q, r]: [u32; 4]) {
    s.local_get(p).i32_load(VARIANT).local_set(tag);
    // Only a variant with a payload has the slot.
    if m.payload == m.all() {
        s.local_get(p).i64_load(slot_at(ENUM_SLOTS)).local_set(pay);
    } else if m.payload == 0 {
        s.i64_const(0).local_set(pay);
    } else {
        in_mask(s, tag, m.payload);
        s.if_(BlockType::Result(ValType::I64))
            .local_get(p)
            .i64_load(slot_at(ENUM_SLOTS))
            .else_()
            .i64_const(0)
            .end()
            .local_set(pay);
    }
    if own && m.ptr != 0 {
        if let Some(mask) = m.tested() {
            in_mask(s, tag, mask);
            s.if_(BlockType::Empty);
        }
        s.local_get(pay).i32_wrap_i64().local_set(q);
        inc(s, rt, q, r);
        if m.tested().is_some() {
            s.end();
        }
    }
    s.local_get(tag).local_get(pay);
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
    sh: &Shapes<'m>,
    statics: &mut Statics<'m>,
    types: &mut Vec<FuncSig>,
    f: &'m ir::Func,
    sites: &mut Vec<Site>,
    (lent, index): (&[Vec<bool>], usize),
) -> (FuncSig, Function) {
    // Each IR local with a value gets a Wasm local (two for an unboxed enum): parameters
    // first, as Wasm numbers them, then the others; `()` locals get none. Hidden locals follow.
    let mut params = Vec::new();
    let mut decls = Vec::new();
    let mut locals = Vec::new();
    for (i, l) in f.locals.iter().enumerate() {
        let at = (params.len() + decls.len()) as u32;
        let rep = sh.rep(&l.ty);
        let into = if i < f.params as usize { &mut params } else { &mut decls };
        into.extend(sh.types(&l.ty));
        locals.push(match rep {
            Rep::None => Val::None,
            Rep::One(_) => Val::One(at),
            Rep::Two => Val::Two(at, at + 1),
            Rep::Fields(id) => {
                let mut next = at;
                Val::Fields(id, field_locals(sh, id, |_| (next, next += 1).0))
            }
        });
    }
    let mut g = FuncGen {
        rt,
        sh,
        m: sh.m,
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
        enum_scratch: None,
        owned: Vec::new(),
        moves: moves::moves(f, &|t| sh.owns(t)),
        lent,
        own_lent: &lent[index],
        depth: 0,
        loops: Vec::new(),
        code: Vec::new(),
    };
    // The owned parameters are the call's, as if bound by `let` around the body; the borrowed
    // ones the caller's (C8b4).
    for (i, l) in f.locals[..f.params as usize].iter().enumerate() {
        if !lent[index][i] {
            g.own(g.locals[i], &l.ty);
        }
    }
    let ret = sh.rep(&f.ret);
    g.block(&f.body, ret != Rep::None);
    g.release(0);
    g.sink().end();
    let mut body = Function::new_with_locals_types(g.decls);
    body.raw(g.code);
    ((params, sh.types(&f.ret)), body)
}

struct FuncGen<'a, 'm> {
    rt: &'a Runtime,
    sh: &'a Shapes<'m>,
    m: &'m Module,
    f: &'m ir::Func,
    /// The function's return type, which `?` returns a `None` or `Err` of.
    ret: &'m Ty,
    statics: &'a mut Statics<'m>,
    /// The program's function types, which a call through a `fn` value adds to.
    types: &'a mut Vec<FuncSig>,
    sites: &'a mut Vec<Site>,
    /// The Wasm locals of each IR local.
    locals: Vec<Val>,
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
    /// Three for boxing and unboxing an enum (C8c1): the object (`i32`), a tag (`i32`), a
    /// payload (`i64`).
    enum_scratch: Option<[u32; 3]>,
    /// The Wasm locals that hold an owned reference at this point of the code, outermost scope
    /// first: parameters, then each `let` once it has run, and hidden locals holding the
    /// operands evaluated so far. A scope's end releases its part; an exit releases the part
    /// of every scope it leaves.
    owned: Vec<Owned>,
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

/// What a runtime operation takes before its operands: nothing, or its result's descriptor (a
/// `List<Str>`, `List<K>`, `Result`).
#[derive(Clone, Copy)]
enum Pre {
    Nothing,
    Desc,
}

/// The variant an allocation writes: a constant, or the tag in an `i32` local (C8c1).
#[derive(Clone, Copy)]
enum Tag {
    Const(u32),
    Local(u32),
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
/// `Rng`'s argument checks (`rng_check`); the texts are `lang::eval::cap_arg_error`'s.
const RNG_BYTES: &str = "Rng.bytes({a}): the count must be 0 to 65536";
const RNG_INT: &str = "Rng.int({a}, {b}): lo must be less than hi";

impl<'m> FuncGen<'_, 'm> {
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
    /// element as a slot, borrowed (the caller's `slot_out(.., true)` takes a reference). Out
    /// of bounds, the runtime reports the error at `site`.
    fn list_get(&mut self, l: u32, i: u32, site: u32) {
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

    /// An object, a tag and a payload for boxing and unboxing an enum; as `rc_scratch`, they
    /// hold no kio code.
    fn enum_scratch(&mut self) -> [u32; 3] {
        if self.enum_scratch.is_none() {
            let (o, tag, pay) = (
                self.new_local(ValType::I32),
                self.new_local(ValType::I32),
                self.new_local(ValType::I64),
            );
            self.enum_scratch = Some([o, tag, pay]);
        }
        self.enum_scratch.unwrap()
    }

    /// Pushes the value held in `v`.
    fn get_val(&mut self, v: Val) {
        match v {
            Val::None => {}
            Val::One(l) => {
                self.sink().local_get(l);
            }
            Val::Two(tag, pay) => {
                self.sink().local_get(tag).local_get(pay);
            }
            Val::Fields(id, fl) => {
                for (l, _) in self.field_vals(id, fl) {
                    self.sink().local_get(l);
                }
            }
        }
    }

    /// Pops the value on the stack into `v`.
    fn set_val(&mut self, v: Val) {
        match v {
            Val::None => {}
            Val::One(l) => {
                self.sink().local_set(l);
            }
            Val::Two(tag, pay) => {
                self.sink().local_set(pay).local_set(tag);
            }
            Val::Fields(id, fl) => {
                for (l, _) in self.field_vals(id, fl).into_iter().rev() {
                    self.sink().local_set(l);
                }
            }
        }
    }

    /// The field locals of unboxed struct `id` held in `fl` that have a value, in field order,
    /// each with its field's type.
    fn field_vals(&self, id: ir::StructId, fl: [u32; MAX_FIELDS]) -> Vec<(u32, &'m Ty)> {
        self.sh
            .field_tys(id)
            .enumerate()
            .filter(|(_, t)| self.sh.field_rep(t) != Rep::None)
            .map(|(i, t)| (fl[i], t))
            .collect()
    }

    /// Hidden locals for a value of type `t`.
    fn temp_val(&mut self, t: &Ty) -> Val {
        match self.sh.rep(t) {
            Rep::None => Val::None,
            Rep::One(vt) => Val::One(self.temp(vt)),
            Rep::Two => Val::Two(self.temp(ValType::I32), self.temp(ValType::I64)),
            Rep::Fields(id) => {
                let sh = self.sh;
                Val::Fields(id, field_locals(sh, id, |vt| self.temp(vt)))
            }
        }
    }

    /// The owned references a value of type `t` held in `v` has: at most one, except an unboxed
    /// struct's, one per pointer field.
    fn owned_of(&self, v: Val, t: &Ty) -> Vec<Owned> {
        match v {
            Val::One(l) if self.sh.is_ptr(t) => vec![Owned::ptr(l)],
            Val::Two(tag, pay) => {
                let m = self.sh.masks(t);
                (m.ptr != 0)
                    .then(|| Owned {
                        local: pay,
                        slot: true,
                        test: m.tested().map(|mask| (tag, mask)),
                    })
                    .into_iter()
                    .collect()
            }
            Val::Fields(id, fl) => self
                .field_vals(id, fl)
                .into_iter()
                .filter(|(_, t)| slot_ptr(t))
                .map(|(l, _)| Owned::ptr(l))
                .collect(),
            _ => vec![],
        }
    }

    /// Registers the references held in `v`, of type `t`, in `owned`.
    fn own(&mut self, v: Val, t: &Ty) {
        let os = self.owned_of(v, t);
        self.owned.extend(os);
    }

    /// The references local `id` holds.
    fn local_owned(&self, id: ir::LocalId) -> Vec<Owned> {
        self.owned_of(self.local(id), &self.f.locals[id.0 as usize].ty)
    }

    /// Drops the references in `os`.
    fn dec_all(&mut self, os: &[Owned]) {
        for &o in os {
            self.dec_owned(o);
        }
    }

    /// Drops the references in `os` and empties their locals.
    fn dec_and_zero(&mut self, os: &[Owned]) {
        for &o in os {
            self.dec_owned(o);
            self.zero(o);
        }
    }

    /// Drops the reference in `o`.
    fn dec_owned(&mut self, o: Owned) {
        if let Some((tag, mask)) = o.test {
            in_mask(&mut self.sink(), tag, mask);
            self.sink().if_(BlockType::Empty);
        }
        let l = if o.slot {
            let [p, _] = self.rc_scratch();
            self.sink().local_get(o.local).i32_wrap_i64().local_set(p);
            p
        } else {
            o.local
        };
        self.dec(l);
        if o.test.is_some() {
            self.sink().end();
        }
    }

    /// Empties `o`'s local, so later drops find nothing.
    fn zero(&mut self, o: Owned) {
        if o.slot {
            self.sink().i64_const(0).local_set(o.local);
        } else {
            self.sink().i32_const(0).local_set(o.local);
        }
    }

    /// Drops the references a value of type `t` held in `v` has.
    fn drop_val(&mut self, v: Val, t: &Ty) {
        let os = self.owned_of(v, t);
        self.dec_all(&os);
    }

    /// Drops the value of type `t` on the stack.
    fn drop_top(&mut self, t: &Ty) {
        match self.sh.rep(t) {
            Rep::None => {}
            Rep::One(_) if self.sh.is_ptr(t) => self.dec_top(),
            Rep::One(_) => {
                self.sink().drop();
            }
            Rep::Two => {
                let [_, tag, pay] = self.enum_scratch();
                self.set_val(Val::Two(tag, pay));
                self.drop_val(Val::Two(tag, pay), t);
            }
            Rep::Fields(_) => {
                let v = self.temp_val(t);
                self.set_val(v);
                self.drop_val(v, t);
            }
        }
    }

    /// Pushes a copy (+1) of the value of type `t` held in `v`.
    fn copy_val(&mut self, v: Val, t: &Ty) {
        match v {
            Val::One(l) if self.sh.is_ptr(t) => self.inc(l),
            Val::Two(tag, pay) => self.inc_payload(tag, pay, t),
            Val::Fields(..) => {
                for o in self.owned_of(v, t) {
                    self.inc(o.local);
                }
            }
            _ => {}
        }
        self.get_val(v);
    }

    /// Adds a reference to the payload in `pay` of the unboxed enum of type `t` whose tag is in
    /// `tag`, when its variant carries a pointer.
    fn inc_payload(&mut self, tag: u32, pay: u32, t: &Ty) {
        let m = self.sh.masks(t);
        if m.ptr == 0 {
            return;
        }
        if let Some(mask) = m.tested() {
            in_mask(&mut self.sink(), tag, mask);
            self.sink().if_(BlockType::Empty);
        }
        let [p, _] = self.rc_scratch();
        self.sink().local_get(pay).i32_wrap_i64().local_set(p);
        self.inc(p);
        if m.tested().is_some() {
            self.sink().end();
        }
    }

    /// Pushes the slot in `pay`, +1 if it holds a pointer (a value of type `t`).
    fn slot_copy(&mut self, pay: u32, t: &Ty) {
        if slot_ptr(t) {
            let [p, _] = self.rc_scratch();
            self.sink().local_get(pay).i32_wrap_i64().local_set(p);
            self.inc(p);
        }
        self.sink().local_get(pay);
    }

    /// Boxes the unboxed enum of type `t` on the stack (its tag, then its payload, whose
    /// reference the object takes): a payload-free variant is its pinned object, from the
    /// type's table of them (`Statics::pins`); any other is allocated.
    fn box_top(&mut self, t: &Ty) {
        let m = self.sh.masks(t);
        let desc = self.statics.desc(t);
        let [o, tag, pay] = self.enum_scratch();
        self.sink().local_set(pay).local_set(tag);
        if m.payload == m.all() {
            return self.box_alloc(desc, tag, pay, o);
        }
        let pins = self.statics.pins(t);
        if m.payload != 0 {
            in_mask(&mut self.sink(), tag, m.payload);
            self.sink().if_(BlockType::Result(ValType::I32));
            self.box_alloc(desc, tag, pay, o);
            self.sink().else_();
        }
        self.sink()
            .i32_const(pins as i32)
            .local_get(tag)
            .i32_const(4)
            .i32_shl()
            .i32_add();
        if m.payload != 0 {
            self.sink().end();
        }
    }

    /// A new object of descriptor `desc` for the one-slot variant in `tag` holding the slot in
    /// `pay`, left on the stack.
    fn box_alloc(&mut self, desc: u32, tag: u32, pay: u32, o: u32) {
        self.alloc(desc, Tag::Local(tag), ENUM_SLOTS, 1, &[], o);
        self.sink()
            .local_get(o)
            .local_get(pay)
            .i64_store(slot_at(ENUM_SLOTS))
            .local_get(o);
    }

    /// Unboxes the enum object (of type `t`) on the stack: its tag, then its payload, +1 when
    /// `own`. The object keeps its own reference.
    fn unbox_top(&mut self, t: &Ty, own: bool) {
        let m = self.sh.masks(t);
        let [o, tag, pay] = self.enum_scratch();
        let [q, r] = self.rc_scratch();
        let rt = self.rt;
        self.sink().local_set(o);
        unbox(&mut self.sink(), rt, m, o, own, [tag, pay, q, r]);
    }

    /// Unboxes the owned (+1) enum or struct object on the stack and drops the object: its
    /// payload's or fields' references become ours.
    fn unbox_owned_top(&mut self, t: &Ty) {
        if self.sh.fields(t).is_some() {
            let o = self.temp(ValType::I32);
            self.sink().local_tee(o);
            self.unbox_struct_top(t, true);
            self.dec(o);
            return;
        }
        let [o, _, _] = self.enum_scratch();
        self.unbox_top(t, true);
        self.dec(o);
    }

    /// Boxes the unboxed struct of type `t` on the stack (its fields, whose references the
    /// object takes) into a new object (C8c2).
    fn box_struct_top(&mut self, t: &Ty) {
        let Val::Fields(id, fl) = self.temp_val(t) else {
            unreachable!("an unboxed struct")
        };
        self.set_val(Val::Fields(id, fl));
        let desc = self.statics.desc(t);
        let o = self.temp(ValType::I32);
        let n = self.m.structs[id.0 as usize].fields.len();
        // Every slot is written below (a `()` field's with 0) before anything reads the object.
        self.alloc(desc, Tag::Const(0), RECORD_SLOTS, n, &[], o);
        let sh = self.sh;
        store_fields(&mut self.sink(), sh, id, &fl, o);
        for (i, ty) in sh.field_tys(id).enumerate() {
            if sh.field_rep(ty) == Rep::None {
                self.sink()
                    .local_get(o)
                    .i64_const(0)
                    .i64_store(slot_at(RECORD_SLOTS + 8 * i as u64));
            }
        }
        self.sink().local_get(o);
    }

    /// Unboxes the struct object (of type `t`) on the stack into its fields, each pointer field
    /// +1 when `own`. The object keeps its own reference.
    fn unbox_struct_top(&mut self, t: &Ty, own: bool) {
        let id = self.sh.fields(t).expect("an unboxed struct");
        let o = self.temp(ValType::I32);
        let [p, r] = self.rc_scratch();
        let (rt, sh) = (self.rt, self.sh);
        self.sink().local_set(o);
        unbox_struct(&mut self.sink(), rt, sh, id, o, own, [p, r]);
    }

    /// Pushes the payload the last `_u` runtime operation left in `rt_scratch`'s word.
    fn scratch_payload(&mut self) {
        let at = slot_at(u64::from(self.rt.scratch));
        self.sink().i32_const(0).i64_load(at);
    }

    /// Pushes operand `x`, held in `t` (an unboxed enum or struct), boxed, and returns the hidden
    /// local holding the object, for the caller to drop after use. A borrowed operand's payload
    /// or fields are copied into the object; an evaluated one's move (its locals emptied).
    fn box_operand(&mut self, x: &ir::Expr, t: Val, borrowed: bool) -> u32 {
        let b = self.temp(ValType::I32);
        match t {
            Val::Two(tag, pay) => {
                if borrowed {
                    self.inc_payload(tag, pay, &x.ty);
                }
                self.sink().local_get(tag).local_get(pay);
                if !borrowed {
                    self.sink().i64_const(0).local_set(pay);
                }
                self.box_top(&x.ty);
            }
            Val::Fields(..) => {
                if borrowed {
                    self.copy_val(t, &x.ty);
                } else {
                    self.get_val(t);
                    for o in self.owned_of(t, &x.ty) {
                        self.zero(o);
                    }
                }
                self.box_struct_top(&x.ty);
            }
            // `!`: nothing after it runs.
            _ => {
                self.sink().unreachable();
            }
        }
        self.sink().local_tee(b);
        b
    }

    /// Whether a value of type `t` is boxed for a runtime operation (`box_operand`).
    fn boxes(&self, t: &Ty) -> bool {
        self.sh.unboxed(t) || self.sh.fields(t).is_some()
    }

    /// The Wasm block type for a value of type `t`.
    fn block_type(&mut self, t: &Ty) -> BlockType {
        match self.sh.rep(t) {
            Rep::None => BlockType::Empty,
            Rep::One(vt) => BlockType::Result(vt),
            Rep::Two | Rep::Fields(_) => {
                let results = self.sh.types(t);
                BlockType::FunctionType(self.rt.types + type_index(self.types, (vec![], results)))
            }
        }
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
            self.dec_owned(self.owned[i]);
        }
    }

    /// Ends the scopes holding `owned[mark..]`: drops their references and zeroes their locals.
    fn end_scope(&mut self, mark: usize) {
        for o in self.owned.split_off(mark).into_iter().rev() {
            self.dec_owned(o);
            self.zero(o);
        }
    }

    /// Evaluates `xs` in order, each into new hidden locals, and returns them (`Val::None` for
    /// `()` and `!`). An owned reference among them is in `owned` from the returned mark on, so
    /// that an exit while a later one runs releases it; the caller truncates `owned` to the
    /// mark once it has used them. An operand `i` for which `borrow(i)` holds and which is a read
    /// of a pointer or unboxed-enum local that no later operand mentions is not evaluated: its
    /// hidden locals are the local's own, borrowed (C8b2). The third result flags those, which
    /// the caller must not drop and may use only until it runs code other than these operands.
    /// Nothing evaluated after a borrowed read can then change or move the local: only an
    /// assignment, a place write or a read in a later operand's own code could (a call cannot
    /// reach the caller's locals), and none mentions it.
    fn operands_borrowing(&mut self, xs: &[&ir::Expr], borrow: impl Fn(usize) -> bool) -> (usize, Vec<Val>, Vec<bool>) {
        self.operands_slotting(xs, borrow, |_| false)
    }

    /// `operands_borrowing`, except that an operand `i` for which `slot(i)` holds is evaluated
    /// as a slot (`slot_expr`: a variant, struct literal or boxed field written out is the
    /// object itself, no unboxing and boxing again) into one `i64` hidden local, which the
    /// caller pushes as is, without `slot_in`.
    fn operands_slotting(
        &mut self,
        xs: &[&ir::Expr],
        borrow: impl Fn(usize) -> bool,
        slot: impl Fn(usize) -> bool,
    ) -> (usize, Vec<Val>, Vec<bool>) {
        let mark = self.owned.len();
        let mut temps = Vec::new();
        let mut borrowed = Vec::new();
        for (i, x) in xs.iter().enumerate() {
            if let ExprKind::Local(id) = x.kind {
                let later = || xs[i + 1..].iter().any(|y| moves::mentions(y, id));
                let v = self.local(id);
                if borrow(i) && v != Val::None && self.sh.lendable(&x.ty) && !later() {
                    temps.push(v);
                    borrowed.push(true);
                    continue;
                }
            }
            let t = if slot(i) {
                self.slot_expr(x);
                let l = self.temp(ValType::I64);
                self.sink().local_set(l);
                if slot_ptr(&x.ty) {
                    self.owned.push(Owned {
                        local: l,
                        slot: true,
                        test: None,
                    });
                }
                Val::One(l)
            } else {
                self.expr(x);
                let t = self.temp_val(&x.ty);
                self.set_val(t);
                self.own(t, &x.ty);
                t
            };
            temps.push(t);
            borrowed.push(false);
        }
        (mark, temps, borrowed)
    }

    /// Pushes operand `x` from its hidden locals `t`. An operand of type `!` has none, and
    /// the code after it is unreachable: `unreachable` keeps the stack's types valid.
    fn get(&mut self, x: &ir::Expr, t: Val) {
        if t == Val::None && x.ty == Ty::Never {
            self.sink().unreachable();
        }
        self.get_val(t);
    }

    /// Evaluates `xs`, then runs `f` to use them from their hidden locals without taking
    /// them, then drops the references among them: a runtime operation borrows its arguments.
    /// A local operand is borrowed (`operands_borrowing`, the flags `f` gets), so neither
    /// copied nor dropped.
    fn borrowing(&mut self, xs: &[&ir::Expr], f: impl FnOnce(&mut Self, &[Val], &[bool])) {
        let (mark, temps, borrowed) = self.operands_borrowing(xs, |_| true);
        f(self, &temps, &borrowed);
        self.drop_operands(xs, &temps, &borrowed);
        self.owned.truncate(mark);
    }

    /// Drops the operands among `xs` that were evaluated, not borrowed.
    fn drop_operands(&mut self, xs: &[&ir::Expr], temps: &[Val], borrowed: &[bool]) {
        for ((x, t), b) in xs.iter().zip(temps).zip(borrowed) {
            if !b {
                self.drop_val(*t, &x.ty);
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
        }
    }

    /// `rt_op` with constants `pre` first (the result's descriptor), and the operands after the
    /// first as slots if `slots`: a map key, a list element. An unboxed enum operand is boxed
    /// for the call and the object dropped after it. An operation with two kinds of error takes
    /// two sites.
    fn rt_op_slots(&mut self, name: &str, pre: &[u32], xs: &[&ir::Expr], sites: &[u32], slots: bool) {
        self.borrowing(xs, |g, temps, borrowed| {
            for &c in pre {
                g.sink().i32_const(c as i32);
            }
            let mut boxes = Vec::new();
            for (i, (x, t)) in xs.iter().zip(temps).enumerate() {
                if g.boxes(&x.ty) {
                    boxes.push(g.box_operand(x, *t, borrowed[i]));
                    if slots && i > 0 {
                        g.sink().i64_extend_i32_u();
                    }
                } else {
                    g.get(x, *t);
                    if slots && i > 0 {
                        g.slot_in(&x.ty);
                    }
                }
            }
            for &s in sites {
                g.sink().i32_const(s as i32);
            }
            g.call_rt(name);
            for b in boxes {
                g.dec(b);
            }
        });
    }

    /// Turns a value of type `t` on the stack into a slot (`i64`): `Int` as is, `Float` as its
    /// bits, `Bool` and pointers zero-extended, `()` as 0, an unboxed enum boxed (the object
    /// takes its payload's reference).
    fn slot_in(&mut self, t: &Ty) {
        match self.sh.rep(t) {
            Rep::One(ValType::I64) => {}
            Rep::One(ValType::F64) => {
                self.sink().i64_reinterpret_f64();
            }
            Rep::One(_) => {
                self.sink().i64_extend_i32_u();
            }
            Rep::None => {
                self.sink().i64_const(0);
            }
            Rep::Two => {
                self.box_top(t);
                self.sink().i64_extend_i32_u();
            }
            Rep::Fields(_) => {
                self.box_struct_top(t);
                self.sink().i64_extend_i32_u();
            }
        }
    }

    /// Evaluates `x` as a slot: its value through `slot_in`, except that an unboxed enum's
    /// variant or an unboxed struct's literal written out (`xs.push(Some(v))`,
    /// `[Tree::Leaf(1)]`, `[P { x: 1 }]`) is built as the object directly rather than unboxed
    /// and boxed again.
    fn slot_expr(&mut self, x: &ir::Expr) {
        match &x.kind {
            ExprKind::Variant(_, v, args) if self.sh.unboxed(&x.ty) => {
                match args.as_slice() {
                    [] => {
                        let a = self.statics.pinned(&x.ty, *v);
                        self.sink().i32_const(a as i32);
                    }
                    [arg] => {
                        let desc = self.statics.desc(&x.ty);
                        self.object(desc, *v, ENUM_SLOTS, &[(0, arg)]);
                    }
                    _ => unreachable!("an unboxed enum's variants carry at most one slot"),
                }
                self.sink().i64_extend_i32_u();
            }
            ExprKind::StructLit(_, inits) if self.sh.fields(&x.ty).is_some() => {
                let desc = self.statics.desc(&x.ty);
                let inits: Vec<(u32, &ir::Expr)> = inits.iter().map(|(f, x)| (*f, x)).collect();
                self.object(desc, 0, RECORD_SLOTS, &inits);
                self.sink().i64_extend_i32_u();
            }
            // A boxed enum or struct in an unboxed struct local's field is the object itself:
            // moved out at the local's last use, else +1.
            ExprKind::Field(base, f) if self.boxes(&x.ty) && self.field_local(base, *f).is_some() => {
                let l = self.field_local(base, *f).expect("checked");
                self.sink().local_get(l);
                if self.moves_out(base) {
                    self.sink().i32_const(0).local_set(l);
                } else {
                    self.inc(l);
                }
                self.sink().i64_extend_i32_u();
            }
            _ => {
                self.expr(x);
                self.slot_in(&x.ty);
            }
        }
    }

    /// Turns a slot on the stack into a value of type `t`, which gains a reference when `own`
    /// (a pointer +1; an unboxed enum's payload or an unboxed struct's pointer fields +1, the
    /// object untouched). Without `own`, a pointer is the slot's reference and an enum or
    /// struct a borrowed view.
    fn slot_out(&mut self, t: &Ty, own: bool) {
        let rep = self.sh.rep(t);
        match rep {
            Rep::Two => {
                self.sink().i32_wrap_i64();
                self.unbox_top(t, own);
            }
            Rep::Fields(_) => {
                self.sink().i32_wrap_i64();
                self.unbox_struct_top(t, own);
            }
            Rep::One(ValType::I32) if own && self.sh.is_ptr(t) => {
                let [p, _] = self.rc_scratch();
                self.sink().i32_wrap_i64().local_tee(p);
                self.inc(p);
            }
            rep => slot_out(&mut self.sink(), rep),
        }
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
            debug_assert!(
                !self.owned.iter().any(|o| o.local == l),
                "a freed hidden local is still owned"
            );
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

    fn local(&self, id: ir::LocalId) -> Val {
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

    /// `b`'s statements and tail, leaving its value on the stack if `want`. A block without a
    /// tail where a value is wanted diverges.
    fn block(&mut self, b: &ir::Block, want: bool) {
        let mark = self.owned.len();
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(e) = &b.tail {
            self.expr(e);
        }
        self.end_scope(mark);
        if b.tail.is_none() && want {
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
                let v = self.local(*id);
                self.set_val(v);
                self.own(v, &init.ty);
            }
            StmtKind::Assign(place, op, value) if !place.steps.is_empty() => {
                self.assign_element(place, *op, value, s.span)
            }
            StmtKind::Assign(place, op, value) => {
                let v = self.local(place.root);
                if v == Val::None {
                    return self.expr(value);
                }
                debug_assert!(!self.is_lent(place.root), "a borrowed parameter assigned");
                match (op, v) {
                    (None, _) => match appended(place.root, value) {
                        Some(parts) => {
                            let Val::One(l) = v else { unreachable!("a Str") };
                            self.append(place.root, l, parts)
                        }
                        // The value first, then the old one dropped.
                        None => {
                            self.expr(value);
                            let os = self.local_owned(place.root);
                            self.dec_all(&os);
                        }
                    },
                    (Some(BinOp::Concat), Val::One(l)) => {
                        // `x + value`: the value first, then `rt_str_append` takes the old `x`
                        // (growing it in place if `x` held its only reference) and borrows the
                        // value, which is then dropped. A read of another local is borrowed
                        // instead (C8b2); `x += x` reads `x` +1, so `x` is copied.
                        let y = match value.kind {
                            ExprKind::Local(y) if y != place.root => match self.local(y) {
                                Val::One(l) => Some(l),
                                _ => None,
                            },
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
                    (Some(op @ (BinOp::FAdd | BinOp::FSub)), Val::One(l)) => {
                        let v = self.temp(ValType::F64);
                        self.expr(value);
                        self.sink().local_set(v).local_get(l).local_get(v);
                        self.float_op(*op);
                    }
                    (Some(op), Val::One(l)) => {
                        // The value comes first, as in the interpreter, then the local's value.
                        self.expr(value);
                        let v = self.scratch()[0];
                        self.sink().local_set(v).local_get(l).local_get(v);
                        self.int_op(*op, s.span);
                    }
                    (Some(op), v) => unreachable!("{op:?} on {v:?}"),
                }
                self.set_val(v);
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
                let Val::One(i) = self.local(*i) else {
                    unreachable!("an Int local")
                };
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
                self.block(body, false);
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
                self.drop_top(&e.ty);
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
                let Val::One(l) = self.local(id) else {
                    unreachable!("a list")
                };
                self.sink().local_get(l).local_set(list);
            }
            _ => {
                self.expr(xs);
                self.sink().local_set(list);
                self.own(Val::One(list), &xs.ty);
            }
        }
        let ty = elem(&xs.ty);
        let x = self.local(x);
        let x_owned = self.owned_of(x, ty);
        self.owned.extend(x_owned.iter().copied());
        let (n, i) = (self.temp(ValType::I64), self.temp(ValType::I64));
        self.sink().local_get(list);
        self.call_rt("rt_list_len");
        self.sink().local_set(n).i64_const(0).local_set(i);
        let brk = self.open(Open::Block, BlockType::Empty);
        let top = self.open(Open::Loop, BlockType::Empty);
        self.sink().local_get(i).local_get(n).i64_ge_s();
        self.br_if(brk);
        let cont = self.open(Open::Block, BlockType::Empty);
        if x != Val::None {
            // In bounds: the list cannot change while the loop holds it.
            let site = self.site(span, OUT_OF_BOUNDS);
            self.list_get(list, i, site);
            self.slot_out(ty, true);
            self.set_val(x);
        }
        self.loops.push((brk, cont, self.owned.len()));
        self.block(body, false);
        self.loops.pop();
        self.close();
        self.dec_and_zero(&x_owned);
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
        if let (Val::Fields(id, fl), [ir::PlaceStep::Field(f)]) = (self.local(place.root), place.steps.as_slice()) {
            let fty = &self.m.structs[id.0 as usize].fields[*f as usize].1;
            return self.assign_field_local(fl[*f as usize], fty, op, value, span);
        }
        let (mut xs, sites) = self.indexes(place);
        let n = xs.len();
        xs.push(value);
        // A stored value is evaluated as the slot it becomes.
        let (mark, temps, _) = self.operands_slotting(&xs, |_| false, |i| i == n && op.is_none());
        let last = place.steps.len() - 1;
        let mut k = 0;
        let (cur, a) = self.walk(place, &temps, &sites, last, &mut k);
        let a = a.unwrap_or_else(|| self.temp(ValType::I32));
        self.step_slot(&place.steps[last], cur, a, &temps, &sites, &mut k);
        match (op, temps[n]) {
            (None, v) => {
                // The slot takes the value, then the old one is dropped.
                let old = slot_ptr(&value.ty).then(|| self.temp(ValType::I32));
                if let Some(old) = old {
                    self.sink().local_get(a).i64_load(SLOT).i32_wrap_i64().local_set(old);
                }
                self.sink().local_get(a);
                self.get(value, v);
                self.sink().i64_store(SLOT);
                if let Some(old) = old {
                    self.dec(old);
                }
            }
            (Some(_), Val::None) => {
                self.sink().unreachable();
            }
            (Some(op), Val::Two(..) | Val::Fields(..)) => unreachable!("{op:?} on an enum or struct"),
            (Some(op), Val::One(v)) => {
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

    /// `s.f = v`, `+=` and `-=` on field `l` (of type `fty`) of an unboxed struct local (C8c2):
    /// the value first, then the field's local is written, with no `unique` check; a boxed enum
    /// or struct field is boxed into it, and the old object dropped after the value is made.
    fn assign_field_local(&mut self, l: u32, fty: &Ty, op: Option<BinOp>, value: &ir::Expr, span: Span) {
        match op {
            None => {
                self.expr(value);
                if self.sh.field_rep(fty) == Rep::None {
                    return;
                }
                self.box_field(fty);
                if slot_ptr(fty) {
                    self.dec(l);
                }
                self.sink().local_set(l);
            }
            Some(BinOp::Concat) => {
                // `rt_str_append` takes the field's reference (growing the string in place if it
                // was the only one) and borrows the value, which is then dropped.
                let v = self.temp(ValType::I32);
                self.expr(value);
                self.sink().local_set(v).local_get(l).local_get(v);
                self.call_rt("rt_str_append");
                self.sink().local_set(l);
                self.dec(v);
            }
            Some(op @ (BinOp::FAdd | BinOp::FSub)) => {
                let v = self.temp(ValType::F64);
                self.expr(value);
                self.sink().local_set(v).local_get(l).local_get(v);
                self.float_op(op);
                self.sink().local_set(l);
            }
            Some(op) => {
                self.expr(value);
                let v = self.scratch()[0];
                self.sink().local_set(v).local_get(l).local_get(v);
                self.int_op(op, span);
                self.sink().local_set(l);
            }
        }
    }

    /// `push`, `insert`, `remove`, `sort` and `pop` (of type `ty`) on a place: the indexes,
    /// then the arguments, then the place made unique along its path, and the change.
    fn mutate(&mut self, place: &ir::Place, op: Mut, args: &[ir::Expr], ty: &Ty, span: Span) {
        let (mut xs, sites) = self.indexes(place);
        let n = xs.len();
        xs.extend(args);
        // A map key is borrowed: it is read after the walk, which changes only the root. So is
        // the function `sort_by` and `sort_by_key` call.
        let key = matches!(
            op,
            Mut::MapInsert | Mut::MapRemove | Mut::ListSortBy | Mut::ListSortByKey
        );
        // A pushed or inserted value is evaluated as the slot it becomes.
        let stored = |i: usize| (op == Mut::ListPush && i == n) || (op == Mut::MapInsert && i == n + 1);
        let (mark, temps, borrowed) = self.operands_slotting(&xs, |i| key && i == n, stored);
        let (cur, slot) = self.walk(place, &temps, &sites, place.steps.len(), &mut 0);
        let args: Vec<(&ir::Expr, Val)> = args.iter().zip(temps[n..].iter().copied()).collect();
        if op == Mut::ListPop {
            // The popped element moves into the payload: the expression's `Some`, unboxed.
            debug_assert!(self.sh.unboxed(ty));
            self.sink().local_get(cur);
            self.call_rt("rt_list_pop_u");
            self.scratch_payload();
        } else if op == Mut::ListSort {
            self.sink().local_get(cur);
            self.call_rt("rt_list_sort");
        } else if op == Mut::ListReverse {
            self.sink().local_get(cur);
            self.call_rt("rt_list_reverse");
        } else if let (Mut::ListSortBy | Mut::ListSortByKey, (f, t)) = (op, args[0]) {
            let (Val::One(c), Ty::Fn(params, ret)) = (t, &f.ty) else {
                // `f` is `!`: nothing after it runs.
                self.sink().unreachable();
                self.owned.truncate(mark);
                return;
            };
            match op {
                Mut::ListSortBy => self.sort_by(cur, c, (params, ret), span),
                _ => self.sort_by_key(cur, c, (params, ret), span),
            }
            if !borrowed[n] {
                self.dec(c);
            }
        } else if op == Mut::BytesExtend {
            // As `push`: the result is the object to store back. The argument is an owned
            // operand, lent to the runtime and then dropped, never the place's own local: in
            // `b.extend(b)` it holds a reference, so the walk copies the place first.
            if let Some(a) = slot {
                self.sink().local_get(a);
            }
            self.sink().local_get(cur);
            self.get(args[0].0, args[0].1);
            self.call_rt("rt_bytes_extend");
            match slot {
                Some(_) => self.sink().i64_extend_i32_u().i64_store(SLOT),
                None => self.sink().local_set(cur),
            };
            if let Val::One(t) = args[0].1 {
                self.dec(t);
            }
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
            for (i, &(x, t)) in args.iter().enumerate() {
                self.get(x, t);
                if !stored(n + i) {
                    self.slot_in(&x.ty);
                }
            }
            self.call_rt(match op {
                Mut::ListPush => "rt_list_push",
                Mut::MapInsert => "rt_map_insert",
                Mut::MapRemove => "rt_map_remove",
                op => unreachable!("{op:?} is handled above"),
            });
            if let (Mut::MapInsert | Mut::MapRemove, (k, Val::One(t))) = (op, args[0]) {
                if self.sh.is_ptr(&k.ty) && !borrowed[n] {
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
    fn walk(&mut self, place: &ir::Place, temps: &[Val], sites: &[u32], n: usize, k: &mut usize) -> (u32, Option<u32>) {
        debug_assert!(!self.is_lent(place.root), "a borrowed parameter changed in place");
        let (root, steps) = match self.local(place.root) {
            Val::One(root) => (root, &place.steps[..n]),
            // An unboxed struct's field local holds the first container (C8c2): the struct
            // itself needs no `unique` check.
            Val::Fields(_, fl) => {
                let ir::PlaceStep::Field(f) = place.steps[0] else {
                    unreachable!("a struct's step is a field")
                };
                (fl[f as usize], &place.steps[1..n])
            }
            v => unreachable!("{v:?} is not a container local"),
        };
        self.sink().local_get(root);
        self.unique_top();
        self.sink().local_set(root);
        if steps.is_empty() {
            return (root, None);
        }
        let (cur, a) = (self.temp(ValType::I32), self.temp(ValType::I32));
        self.sink().local_get(root).local_set(cur);
        for step in steps {
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
    fn step_slot(&mut self, step: &ir::PlaceStep, cur: u32, a: u32, temps: &[Val], sites: &[u32], k: &mut usize) {
        match step {
            ir::PlaceStep::Field(f) => {
                let off = RECORD_SLOTS as i32 + 8 * *f as i32;
                self.sink().local_get(cur).i32_const(off).i32_add();
            }
            ir::PlaceStep::Index(..) => {
                match temps[*k] {
                    Val::One(i) => {
                        self.sink().local_get(cur).local_get(i).i32_const(sites[*k] as i32);
                        self.call_rt("rt_list_at");
                    }
                    // An index of type `!` (`xs[{ return 7; }] = v`) never yields a value.
                    _ => {
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
        self.block(body, false);
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
                let v = self.local(*id);
                if self.moves_out(e) {
                    // The last use: the reference moves out, and the local is left empty, so
                    // the scope's end and every exit drop nothing (`rt_dec(0)`).
                    self.get_val(v);
                    for o in self.owned_of(v, &e.ty) {
                        self.zero(o);
                    }
                } else {
                    // A copy: the local keeps its own reference.
                    self.copy_val(v, &e.ty);
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
            // `==` on objects is the runtime's; unboxed enums are boxed for it (C8c1).
            ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne), a, b) if slot_ptr(&a.ty) => {
                let desc = self.statics.desc(&a.ty);
                self.borrowing(&[&**a, &**b], |g, temps, borrowed| {
                    g.sink().i32_const(desc as i32);
                    let mut boxes = Vec::new();
                    for (i, (x, t)) in [a, b].into_iter().zip(temps).enumerate() {
                        if g.boxes(&x.ty) {
                            boxes.push(g.box_operand(x, *t, borrowed[i]));
                        } else {
                            g.get(x, *t);
                        }
                        g.sink().i64_extend_i32_u();
                    }
                    g.call_rt("rt_eq");
                    if *op == BinOp::Ne {
                        g.sink().i32_eqz();
                    }
                    for bx in boxes {
                        g.dec(bx);
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
                let want = self.sh.rep(&e.ty) != Rep::None;
                let bt = self.block_type(&e.ty);
                self.expr(cond);
                self.open(Open::If, bt);
                self.block(then, want);
                if let Some(els) = els {
                    self.sink().else_();
                    self.expr(els);
                }
                self.close();
            }
            ExprKind::Block(b) => {
                let want = self.sh.rep(&e.ty) != Rep::None;
                self.block(b, want)
            }
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
            ExprKind::Builtin(
                b @ (Builtin::IntMin | Builtin::IntMax | Builtin::FloatMin | Builtin::FloatMax),
                recv,
                args,
            ) => {
                self.expr(recv);
                self.expr(&args[0]);
                self.min_max(*b);
            }
            ExprKind::Builtin(b, recv, _) if inline_builtin(*b) => {
                self.expr(recv);
                self.scalar_builtin(*b);
            }
            // The `Option` and `Result` built-ins test the tag (C8c1).
            ExprKind::Builtin(Builtin::OptionUnwrapOr | Builtin::ResultUnwrapOr, recv, args) => {
                self.unwrap_or(recv, &args[0], &e.ty)
            }
            ExprKind::Builtin(Builtin::OptionIsNone | Builtin::ResultIsOk, recv, _) => self.is_variant(recv, 0),
            ExprKind::Builtin(Builtin::OptionIsSome | Builtin::ResultIsErr, recv, _) => self.is_variant(recv, 1),
            ExprKind::Builtin(Builtin::ResultOk, recv, _) => self.result_variant(recv, 0, &e.ty),
            ExprKind::Builtin(Builtin::ResultErr, recv, _) => self.result_variant(recv, 1, &e.ty),
            ExprKind::Builtin(Builtin::OptionOkOr, recv, args) => self.ok_or(recv, &args[0], &e.ty),
            ExprKind::Builtin(b, recv, args) => {
                use Pre::{Desc, Nothing};
                // An `Option` result comes unboxed from a `_u` operation (`rt_scratch`); any
                // other enum result (a `Result`, a `Json`) comes boxed and is unboxed here.
                let (name, pre, error) = match b {
                    Builtin::IntToFloat
                    | Builtin::FloatAbs
                    | Builtin::FloatFloor
                    | Builtin::FloatCeil
                    | Builtin::FloatSqrt
                    | Builtin::FloatIsNan
                    | Builtin::FloatIsFinite
                    | Builtin::FloatIsInfinite => unreachable!("inline"),
                    Builtin::StrParseFloat => ("rt_str_parse_float_u", Nothing, None),
                    Builtin::FloatToStr => ("rt_float_to_str", Nothing, None),
                    Builtin::FloatToInt => ("rt_float_to_int_u", Nothing, None),
                    Builtin::FloatRound => ("rt_float_round", Nothing, None),
                    Builtin::StrSlice => ("rt_str_slice", Nothing, Some(SLICE_BOUNDS)),
                    Builtin::BytesSlice => ("rt_bytes_slice", Nothing, Some(SLICE_BOUNDS)),
                    Builtin::StrParseInt => ("rt_str_parse_int_u", Nothing, None),
                    Builtin::MapGet => ("rt_map_get_u", Nothing, None),
                    Builtin::OptionUnwrapOr
                    | Builtin::ResultUnwrapOr
                    | Builtin::OptionIsSome
                    | Builtin::OptionIsNone
                    | Builtin::ResultIsOk
                    | Builtin::ResultIsErr
                    | Builtin::ResultOk
                    | Builtin::ResultErr
                    | Builtin::OptionOkOr => unreachable!("inline on the tag"),
                    Builtin::IntToStr => ("rt_int_to_str", Nothing, None),
                    Builtin::StrLen => ("rt_str_len", Nothing, None),
                    Builtin::StrToBytes => ("rt_str_to_bytes", Nothing, None),
                    Builtin::StrSplit => (
                        "rt_str_split",
                        Desc,
                        Some("`split` needs a non-empty separator; to split into characters, use `s.chars()`"),
                    ),
                    Builtin::StrLines => ("rt_str_lines", Desc, None),
                    Builtin::StrChars => ("rt_str_chars", Desc, None),
                    Builtin::StrTrim => ("rt_str_trim", Nothing, None),
                    Builtin::StrContains => ("rt_str_contains", Nothing, None),
                    Builtin::StrStartsWith => ("rt_str_starts_with", Nothing, None),
                    Builtin::StrEndsWith => ("rt_str_ends_with", Nothing, None),
                    Builtin::StrStripPrefix => ("rt_str_strip_prefix_u", Nothing, None),
                    Builtin::StrStripSuffix => ("rt_str_strip_suffix_u", Nothing, None),
                    Builtin::StrFind => ("rt_str_find_u", Nothing, None),
                    Builtin::StrReplace => ("rt_str_replace", Nothing, None),
                    Builtin::ListContains => ("rt_list_contains", Nothing, None),
                    Builtin::BytesLen => ("rt_bytes_len", Nothing, None),
                    Builtin::BytesToStr => ("rt_bytes_to_str", Nothing, Some("invalid UTF-8")),
                    Builtin::ListLen => ("rt_list_len", Nothing, None),
                    Builtin::MapLen => ("rt_map_len", Nothing, None),
                    Builtin::MapKeys => ("rt_map_keys", Desc, None),
                    Builtin::StrParseJson => ("rt_json_parse", Desc, None),
                    Builtin::JsonToJson => ("rt_json_encode", Nothing, None),
                    Builtin::JsonGet => ("rt_json_get_u", Nothing, None),
                    Builtin::JsonAt => ("rt_json_at_u", Nothing, None),
                    Builtin::JsonAsStr => ("rt_json_as_str_u", Nothing, None),
                    Builtin::JsonAsInt => ("rt_json_as_int_u", Nothing, None),
                    Builtin::JsonAsFloat => ("rt_json_as_float_u", Nothing, None),
                    Builtin::JsonAsBool => ("rt_json_as_bool_u", Nothing, None),
                    Builtin::JsonAsList => ("rt_json_as_list_u", Nothing, None),
                    Builtin::JsonAsMap => ("rt_json_as_map_u", Nothing, None),
                    Builtin::ListSlice => ("rt_list_slice", Nothing, Some(SLICE_BOUNDS)),
                    Builtin::ListJoin => ("rt_list_join", Nothing, None),
                    Builtin::StrToLowercase => ("rt_str_to_lowercase", Nothing, None),
                    Builtin::StrToUppercase => ("rt_str_to_uppercase", Nothing, None),
                    Builtin::StrSha256 | Builtin::BytesSha256 => ("rt_sha256", Nothing, None),
                    Builtin::BytesToHex => ("rt_bytes_to_hex", Nothing, None),
                    Builtin::StrFromHex => ("rt_str_from_hex_u", Nothing, None),
                    Builtin::StrCtEq | Builtin::BytesCtEq => ("rt_ct_eq", Nothing, None),
                    Builtin::IntMin | Builtin::IntMax | Builtin::FloatMin | Builtin::FloatMax => {
                        unreachable!("min_max")
                    }
                    Builtin::OptionMap
                    | Builtin::OptionAndThen
                    | Builtin::ResultMap
                    | Builtin::ResultMapErr
                    | Builtin::ResultAndThen
                    | Builtin::ListMap
                    | Builtin::ListFilter
                    | Builtin::ListAny
                    | Builtin::ListAll
                    | Builtin::OptionUnwrapOrElse
                    | Builtin::ResultUnwrapOrElse => unreachable!("with_fn"),
                };
                let pre = self.pre(pre, &e.ty);
                let mut sites: Vec<u32> = error.map(|t| self.site(e.span, t)).into_iter().collect();
                if *b == Builtin::StrSlice {
                    sites.push(self.site(e.span, "slice {a}..{b} is not on a character boundary"));
                }
                let xs: Vec<&ir::Expr> = std::iter::once(&**recv).chain(args).collect();
                // These take their argument as a slot.
                let slots = matches!(b, Builtin::MapGet | Builtin::ListContains);
                self.rt_op_slots(name, &pre, &xs, &sites, slots);
                if name.ends_with("_u") {
                    self.scratch_payload();
                } else if self.boxes(&e.ty) {
                    self.unbox_owned_top(&e.ty);
                }
            }
            ExprKind::Index(base, i) if base.ty == Ty::Bytes => {
                let site = self.site(e.span, OUT_OF_BOUNDS);
                self.borrowing(&[&**base, &**i], |g, temps, _| match temps {
                    [Val::One(b), Val::One(i)] => g.bytes_get(*b, *i, site),
                    _ => {
                        g.sink().unreachable();
                    }
                });
            }
            ExprKind::Index(base, i) => {
                // The element as a slot, then +1 as a value, before a temporary base is dropped.
                let site = self.site(e.span, OUT_OF_BOUNDS);
                self.borrowing(&[&**base, &**i], |g, temps, _| {
                    match temps {
                        [Val::One(l), Val::One(i)] => g.list_get(*l, *i, site),
                        // An operand of type `!`: the code after it is unreachable.
                        _ => {
                            g.sink().unreachable();
                        }
                    }
                    g.slot_out(&e.ty, true);
                });
            }
            ExprKind::List(items) => {
                // The new list is held in a hidden local while its elements run, so an exit
                // in one releases it; each element is consumed by the push.
                let desc = self.statics.desc(&e.ty);
                let l = self.temp(ValType::I32);
                self.sink().i32_const(desc as i32).i32_const(items.len() as i32);
                self.call_rt("rt_list_new");
                self.sink().local_set(l);
                self.owned.push(Owned::ptr(l));
                for x in items {
                    self.sink().local_get(l);
                    self.slot_expr(x);
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
            // An unboxed struct's literal is its fields: the values in source order, each held as
            // an operand, then pushed in field order (an enum or struct value boxed, as its field
            // local holds it); no object for the struct itself (C8c2).
            ExprKind::StructLit(id, inits) if self.sh.fields(&e.ty).is_some() => {
                let xs: Vec<&ir::Expr> = inits.iter().map(|(_, x)| x).collect();
                let fields = &self.m.structs[id.0 as usize].fields;
                let boxed: Vec<bool> = inits.iter().map(|(g, _)| self.boxes(&fields[*g as usize].1)).collect();
                let (mark, temps, _) = self.operands_slotting(&xs, |_| false, |i| boxed[i]);
                for f in 0..fields.len() {
                    let i = inits
                        .iter()
                        .position(|(g, _)| *g as usize == f)
                        .expect("every field written");
                    self.get(xs[i], temps[i]);
                    if boxed[i] {
                        self.sink().i32_wrap_i64();
                    }
                }
                self.owned.truncate(mark);
            }
            ExprKind::StructLit(_, inits) => {
                let desc = self.statics.desc(&e.ty);
                let inits: Vec<(u32, &ir::Expr)> = inits.iter().map(|(f, x)| (*f, x)).collect();
                self.object(desc, 0, RECORD_SLOTS, &inits);
            }
            // An unboxed enum's variant is its tag and its payload as a slot: no object (C8c1).
            ExprKind::Variant(_, v, args) if self.sh.unboxed(&e.ty) => {
                self.sink().i32_const(*v as i32);
                match args.as_slice() {
                    [] => {
                        self.sink().i64_const(0);
                    }
                    [x] => self.slot_expr(x),
                    _ => unreachable!("an unboxed enum's variants carry at most one slot"),
                }
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
            ExprKind::CapCall(cap, method, args) => self.cap_call(*cap, *method, args, &e.ty, e.span),
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
                temps.push(Val::None);
                continue;
            }
            self.expr(x);
            let t = self.temp_val(&x.ty);
            self.set_val(t);
            self.own(t, &x.ty);
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
                    let os = self.local_owned(root(x));
                    debug_assert!(!os.is_empty(), "a local holding a reference");
                    self.dec_and_zero(&os);
                }
                (false, t) if lent => self.drop_val(t, &x.ty),
                _ => {}
            }
        }
    }

    /// A capability method: the runtime's wrapper (runtime/src/caps.rs) takes the result's
    /// descriptor, the handle, then the arguments, which it borrows.
    fn cap_call(&mut self, cap: ir::LocalId, method: u32, args: &[ir::Expr], ty: &Ty, span: Span) {
        let Ty::Cap(c) = self.f.locals[cap.0 as usize].ty else {
            unreachable!("a capability call on a capability")
        };
        let def = &self.m.caps[c.0 as usize];
        let name = format!("rt_cap_{}_{}", snake(&def.name), def.methods[method as usize].0);
        let rng = (def.name == "Rng").then(|| def.methods[method as usize].0.clone());
        let desc = self.statics.desc(ty);
        let Val::One(h) = self.local(cap) else {
            unreachable!("a capability has a value")
        };
        let xs: Vec<&ir::Expr> = args.iter().collect();
        // An enum argument (`Option<Str>`) is boxed for the wrapper; an enum result comes boxed.
        self.borrowing(&xs, |g, temps, borrowed| {
            if let Some(m) = &rng {
                g.rng_check(span, m, temps);
            }
            g.sink().i32_const(desc as i32).local_get(h);
            let mut boxes = Vec::new();
            for (i, (x, t)) in xs.iter().zip(temps).enumerate() {
                if g.boxes(&x.ty) {
                    boxes.push(g.box_operand(x, *t, borrowed[i]));
                } else {
                    g.get(x, *t);
                }
            }
            g.call_rt(&name);
            for b in boxes {
                g.dec(b);
            }
        });
        if self.boxes(ty) {
            self.unbox_owned_top(ty);
        }
    }

    /// `Rng.bytes(n)` needs `n` in 0 to 65,536 and `Rng.int(lo, hi)` needs `lo < hi`: a runtime
    /// error at the call with the interpreter's message (lang/src/eval.rs `cap_arg_error`); the
    /// host checks again and stops a run whose code skipped this. An argument of type `!` has
    /// no local, and the call after it is unreachable.
    fn rng_check(&mut self, span: Span, method: &str, temps: &[Val]) {
        match (method, temps) {
            ("bytes", [Val::One(n)]) => {
                let n = *n;
                self.sink()
                    .local_get(n)
                    .i64_const(0)
                    .i64_lt_s()
                    .local_get(n)
                    .i64_const(lang::eval::RNG_BYTES_MAX)
                    .i64_gt_s()
                    .i32_or();
                self.trap_if(span, RNG_BYTES, n, n);
            }
            ("int", [Val::One(lo), Val::One(hi)]) => {
                self.sink().local_get(*lo).local_get(*hi).i64_ge_s();
                self.trap_if(span, RNG_INT, *lo, *hi);
            }
            _ => {}
        }
    }

    /// A new struct (`first` 8) or enum variant (`first` 16) of descriptor `desc`: the object
    /// is held in a hidden local while `parts` run in order, so an exit in one releases it;
    /// each part's value goes into its slot (`first + 8 * index`), which takes it.
    fn object(&mut self, desc: u32, variant: u32, first: u64, parts: &[(u32, &ir::Expr)]) {
        let o = self.temp(ValType::I32);
        // A part that exits frees the object, which reads its pointer slots: those start 0.
        let zero: Vec<u64> = parts
            .iter()
            .filter(|(_, x)| slot_ptr(&x.ty))
            .map(|(i, _)| first + 8 * u64::from(*i))
            .collect();
        self.alloc(desc, Tag::Const(variant), first, parts.len(), &zero, o);
        self.owned.push(Owned::ptr(o));
        for (i, x) in parts {
            self.sink().local_get(o);
            self.slot_expr(x);
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
    /// crafted IR), the runtime allocates, and reports the bad variant. A variant in a local
    /// (boxing an unboxed enum, C8c1) is one of its one-slot variants.
    fn alloc(&mut self, desc: u32, variant: Tag, first: u64, n: usize, zero: &[u64], o: u32) {
        let size = first + 8 * n as u64;
        let f = self.rt.export("rt_alloc");
        let (lists, live) = (self.rt.lists, self.rt.live);
        let known = match variant {
            Tag::Const(v) => self.statics.slots(desc, v) == Some(n),
            Tag::Local(_) => n == 1,
        };
        let mut s = self.sink();
        let push_variant = |s: &mut InstructionSink| {
            match variant {
                Tag::Const(v) => s.i32_const(v as i32),
                Tag::Local(l) => s.local_get(l),
            };
        };
        if size > HEAP_SMALL || !known {
            s.i32_const(desc as i32);
            push_variant(&mut s);
            s.call(f).local_set(o);
            return;
        }
        let head = mem32(u64::from(lists) + 4 * ((size - 1) / 8));
        s.i32_const(0).i32_load(head).local_tee(o).if_(BlockType::Empty);
        s.i32_const(0).local_get(o).i32_load(mem32(0)).i32_store(head);
        s.local_get(o)
            .i64_const(((u64::from(desc) << 32) | 1) as i64)
            .i64_store(slot_at(0));
        if first == ENUM_SLOTS {
            s.local_get(o);
            match variant {
                Tag::Const(v) => s.i64_const(i64::from(v)),
                Tag::Local(l) => s.local_get(l).i64_extend_i32_u(),
            };
            s.i64_store(slot_at(8));
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
        s.i32_const(desc as i32);
        push_variant(&mut s);
        s.call(f).local_set(o);
        s.end();
    }

    /// A closure lifted to `f`: a new function object holding a copy (+1) of each captured
    /// local in its slots, which the object owns until it is freed. Nothing here can exit, so
    /// the object needs no place in `owned` while it is filled.
    fn closure(&mut self, f: ir::FuncId, captured: &[ir::LocalId]) {
        let (desc, elem) = self.statics.func(f, Some(captured.len()));
        let o = self.temp(ValType::I32);
        // Every pointer slot is written below, before anything could read it.
        self.alloc(desc, Tag::Const(elem), ENUM_SLOTS, captured.len(), &[], o);
        let m = self.m;
        for (i, (id, l)) in captured.iter().zip(&m.funcs[f.0 as usize].locals).enumerate() {
            let x = self.local(*id);
            if x == Val::None {
                continue;
            }
            self.sink().local_get(o);
            self.copy_val(x, &l.ty);
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
        let Val::One(c) = temps[0] else {
            unreachable!("a fn value")
        };
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
        let ty = self.rt.types + type_index(self.types, value_sig(self.sh, params, ret));
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
        let (r, Val::One(c), Ty::Fn(params, ret)) = (temps[0], temps[1], &f.ty) else {
            // `f` is `!`: nothing after it runs.
            self.sink().unreachable();
            return;
        };
        let sig = (&params[..], &**ret);
        match (b, r) {
            // The receiver is `!`: nothing after it runs.
            (_, Val::None) => {
                self.sink().unreachable();
            }
            (Builtin::ListMap, Val::One(r)) => self.list_map(r, c, &recv.ty, sig, ty, span),
            (Builtin::ListFilter, Val::One(r)) => self.list_filter(r, c, &recv.ty, sig, ty, span),
            (Builtin::ListAny, Val::One(r)) => self.list_any(true, r, c, &recv.ty, sig, span),
            (Builtin::ListAll, Val::One(r)) => self.list_any(false, r, c, &recv.ty, sig, span),
            (Builtin::OptionUnwrapOrElse | Builtin::ResultUnwrapOrElse, Val::Two(tag, pay)) => {
                self.unwrap_or_else((tag, pay), c, &recv.ty, sig, ty)
            }
            (_, Val::Two(tag, pay)) => self.enum_map(b, (tag, pay), c, &recv.ty, sig, ty),
            (b, r) => unreachable!("{b:?} on {r:?}"),
        }
        self.drop_operands(&[recv, f], &temps, &borrowed);
    }

    /// `o.unwrap_or(d)` or `r.unwrap_or(d)`: `d` runs first, as the argument it is; then on a
    /// `Some` or `Ok` the payload (+1) and `d` dropped, else `d`.
    fn unwrap_or(&mut self, recv: &ir::Expr, d: &ir::Expr, ty: &Ty) {
        let (mark, temps, borrowed) = self.operands_borrowing(&[recv, d], |i| i == 0);
        let Val::Two(tag, pay) = temps[0] else {
            // The receiver is `!`: nothing after it runs.
            self.sink().unreachable();
            self.owned.truncate(mark);
            return;
        };
        let success = matches!(recv.ty, Ty::Option(_)) as i32;
        self.sink().local_get(tag).i32_const(success).i32_eq();
        let bt = self.block_type(ty);
        self.open(Open::If, bt);
        self.payload_owned(pay, ty);
        self.drop_val(temps[1], &d.ty);
        self.sink().else_();
        self.get(d, temps[1]);
        self.close();
        if !borrowed[0] {
            self.drop_val(temps[0], &recv.ty);
        }
        self.owned.truncate(mark);
    }

    /// `is_some`, `is_none`, `is_ok` or `is_err`: the tag against `v`.
    fn is_variant(&mut self, recv: &ir::Expr, v: i32) {
        self.borrowing(&[recv], |g, temps, _| match temps[0] {
            Val::Two(tag, _) => {
                g.sink().local_get(tag).i32_const(v).i32_eq();
            }
            _ => {
                g.sink().unreachable();
            }
        });
    }

    /// `r.ok()` (`v` 0) or `r.err()` (`v` 1) as the `Option` type `ty`: `Some` of the payload
    /// (+1) when `r` is variant `v`, else `None`.
    fn result_variant(&mut self, recv: &ir::Expr, v: i32, ty: &Ty) {
        self.borrowing(&[recv], |g, temps, _| {
            let Val::Two(tag, pay) = temps[0] else {
                g.sink().unreachable();
                return;
            };
            g.sink().local_get(tag).i32_const(v).i32_eq();
            let bt = g.block_type(ty);
            g.open(Open::If, bt);
            g.sink().i32_const(1);
            let t = &payload(g.m, &recv.ty, v as u32)[0];
            g.slot_copy(pay, t);
            g.sink().else_().i32_const(0).i64_const(0);
            g.close();
        });
    }

    /// `o.ok_or(e)` as the `Result` type `ty`: `Ok` of the payload (+1) with `e` dropped, or
    /// `Err(e)`.
    fn ok_or(&mut self, recv: &ir::Expr, e: &ir::Expr, ty: &Ty) {
        let (mark, temps, borrowed) = self.operands_borrowing(&[recv, e], |i| i == 0);
        let (Val::Two(tag, pay), Ty::Option(t)) = (temps[0], &recv.ty) else {
            self.sink().unreachable();
            self.owned.truncate(mark);
            return;
        };
        self.sink().local_get(tag).i32_const(1).i32_eq();
        let bt = self.block_type(ty);
        self.open(Open::If, bt);
        self.sink().i32_const(0);
        self.slot_copy(pay, t);
        self.drop_val(temps[1], &e.ty);
        self.sink().else_().i32_const(1);
        self.get(e, temps[1]);
        self.slot_in(&e.ty);
        self.close();
        if !borrowed[0] {
            self.drop_val(temps[0], &recv.ty);
        }
        self.owned.truncate(mark);
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

    /// Pushes element `i` of list `r` as a slot, borrowed. In bounds: nothing changes `r` while
    /// a built-in holds it.
    fn list_slot(&mut self, r: u32, i: u32, span: Span) {
        let site = self.site(span, OUT_OF_BOUNDS);
        self.list_get(r, i, site);
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
        self.list_slot(r, i, span);
        self.slot_out(elem(xs), true);
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
        self.list_slot(r, i, span);
        self.sink().local_set(x).local_get(c).local_get(x);
        self.slot_out(t, true);
        self.call_through(c, params, ret);
        self.open(Open::If, BlockType::Empty);
        if slot_ptr(t) {
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
        self.list_slot(r, i, span);
        self.slot_out(elem(xs), true);
        self.call_through(c, params, ret);
        self.sink().local_tee(res);
        if !any {
            self.sink().i32_eqz();
        }
        self.br_if(brk);
        self.next_pass(i, top);
        self.sink().local_get(res);
    }

    /// `map`, `and_then` and `map_err` on the unboxed `Option` or `Result` in `(tag, pay)`, of
    /// type `from`: on the variant `f` takes, the call on its payload (+1), whose result `map`
    /// and `map_err` make the payload of that variant of `ty`; on the other, that variant of
    /// `ty` with the payload copied. No object is built.
    fn enum_map(
        &mut self,
        b: Builtin,
        (tag, pay): (u32, u32),
        c: u32,
        from: &Ty,
        (params, ret): (&[Ty], &Ty),
        ty: &Ty,
    ) {
        // `None` and `Ok` are variant 0, `Some` and `Err` variant 1.
        let (called, wrap) = match b {
            Builtin::OptionMap => (1, true),
            Builtin::OptionAndThen => (1, false),
            Builtin::ResultMap => (0, true),
            Builtin::ResultMapErr => (1, true),
            Builtin::ResultAndThen => (0, false),
            b => unreachable!("{b:?} on an enum"),
        };
        self.sink().local_get(tag).i32_const(called).i32_eq();
        let bt = self.block_type(ty);
        self.open(Open::If, bt);
        if wrap {
            self.sink().i32_const(called);
        }
        self.sink().local_get(c);
        let t = &payload(self.m, from, called as u32)[0];
        self.payload_owned(pay, t);
        self.call_through(c, params, ret);
        if wrap {
            self.slot_in(ret);
        }
        self.sink().else_();
        self.rebuild(pay, from, 1 - called);
        self.close();
    }

    /// `unwrap_or_else` on the unboxed `Option` or `Result` in `(tag, pay)`, of type `from`:
    /// the payload of a `Some` or `Ok` (+1); else the call, on nothing (`None`) or the `Err`'s
    /// payload (+1).
    fn unwrap_or_else(&mut self, (tag, pay): (u32, u32), c: u32, from: &Ty, (params, ret): (&[Ty], &Ty), ty: &Ty) {
        // `Some` is variant 1, `Ok` variant 0.
        let ok = matches!(from, Ty::Option(_)) as i32;
        self.sink().local_get(tag).i32_const(ok).i32_eq();
        let bt = self.block_type(ty);
        self.open(Open::If, bt);
        self.payload_owned(pay, ty);
        self.sink().else_().local_get(c);
        if let Ty::Result(_, e) = from {
            self.payload_owned(pay, e);
        }
        self.call_through(c, params, ret);
        self.close();
    }

    /// `xs.sort_by(f)` on the unique list in `l`, `f` in `c`: lang/src/sort.rs's merge sort,
    /// driven here so that each question is a call through `f` on the two elements (+1).
    fn sort_by(&mut self, l: u32, c: u32, (params, ret): (&[Ty], &Ty), span: Span) {
        let (st, q, i) = (
            self.temp(ValType::I32),
            self.temp(ValType::I64),
            self.temp(ValType::I64),
        );
        let site = self.site(span, OUT_OF_BOUNDS);
        self.sink().local_get(l);
        self.call_rt("rt_sort_new");
        self.sink().local_set(st);
        let brk = self.open(Open::Block, BlockType::Empty);
        let top = self.open(Open::Loop, BlockType::Empty);
        self.sink().local_get(st);
        self.call_rt("rt_sort_next");
        self.sink().local_tee(q).i64_const(-1).i64_eq();
        self.br_if(brk);
        // `rt_sort_answer(st, f(xs[q >> 32], xs[q & 0xffff_ffff]))`
        self.sink().local_get(st).local_get(c);
        self.sink().local_get(q).i64_const(32).i64_shr_u().local_set(i);
        self.list_get(l, i, site);
        self.slot_out(&params[0], true);
        self.sink().local_get(q).i64_const(0xffff_ffff).i64_and().local_set(i);
        self.list_get(l, i, site);
        self.slot_out(&params[1], true);
        self.call_through(c, params, ret);
        self.call_rt("rt_sort_answer");
        self.br(top);
        self.close();
        self.close();
        self.sink().local_get(st).local_get(l);
        self.call_rt("rt_sort_finish");
    }

    /// `xs.sort_by_key(f)` on the unique list in `l`, `f` in `c`: the keys, `f` of each
    /// element in order (as `map`), then the runtime's stable sort by them.
    fn sort_by_key(&mut self, l: u32, c: u32, (params, ret): (&[Ty], &Ty), span: Span) {
        let keys = self.temp(ValType::I32);
        let xs = Ty::List(Box::new(params[0].clone()));
        let ks = Ty::List(Box::new(ret.clone()));
        self.list_map(l, c, &xs, (params, ret), &ks, span);
        self.sink().local_set(keys).local_get(l).local_get(keys);
        self.call_rt("rt_list_sort_by_key");
        self.dec(keys);
    }

    /// `a.min(b)` or `a.max(b)` on the two `Int` or `Float` on the stack. `Float` as Rust's:
    /// a NaN is ignored unless both are; Wasm's `f64.min` orders `-0.0` below `0.0`, as the
    /// interpreter does.
    fn min_max(&mut self, b: Builtin) {
        if let Builtin::IntMin | Builtin::IntMax = b {
            let [x, y, _] = self.scratch();
            let mut s = self.sink();
            s.local_set(y).local_set(x);
            s.local_get(x).local_get(y).local_get(x).local_get(y);
            match b {
                Builtin::IntMin => s.i64_lt_s(),
                _ => s.i64_gt_s(),
            };
            s.select();
            return;
        }
        let (x, y) = (self.temp(ValType::F64), self.temp(ValType::F64));
        let mut s = self.sink();
        s.local_set(y).local_set(x).local_get(x).local_get(y);
        match b {
            Builtin::FloatMin => s.f64_min(),
            _ => s.f64_max(),
        };
        // NaN only when one is: `y` if `x` is NaN, then `x` if `y` is.
        s.local_get(y).local_get(x).local_get(x).f64_eq().select();
        s.local_get(x).local_get(y).local_get(y).f64_eq().select();
    }

    /// Pushes the payload slot in `pay` as a value of type `t`, +1.
    fn payload_owned(&mut self, pay: u32, t: &Ty) {
        self.sink().local_get(pay);
        self.slot_out(t, true);
    }

    /// Pushes variant `v` of the unboxed `Option` or `Result` whose payload is in `pay`, of
    /// type `from`, as the other type of the same shape (`?`, `map`, `and_then`): `None` as
    /// `(0, 0)`, an `Err` or `Ok` as its tag and the payload copied (+1).
    fn rebuild(&mut self, pay: u32, from: &Ty, v: i32) {
        self.sink().i32_const(v);
        if let Ty::Option(_) = from {
            self.sink().i64_const(0);
        } else {
            let t = &payload(self.m, from, v as u32)[0];
            self.slot_copy(pay, t);
        }
    }

    /// `base.f`: a new reference to the field's value. A base that is a local, or a field of
    /// one, is read in place; any other is evaluated and dropped after the read. A field of an
    /// unboxed struct local is its local (C8c2): a pointer field is moved out at the local's
    /// last use and copied otherwise; a boxed enum or struct in it is unboxed with a reference.
    fn field(&mut self, base: &ir::Expr, f: u32, ty: &Ty) {
        if let Some(l) = self.field_local(base, f) {
            match self.sh.field_rep(ty) {
                Rep::None => {}
                _ if self.boxes(ty) => {
                    self.sink().local_get(l);
                    self.unbox_field(ty, true);
                }
                _ if slot_ptr(ty) && self.moves_out(base) => {
                    self.sink().local_get(l).i32_const(0).local_set(l);
                }
                _ => {
                    self.sink().local_get(l);
                    if slot_ptr(ty) {
                        self.inc(l);
                    }
                }
            }
            return;
        }
        if self.in_place(base) {
            self.borrow_ptr(base);
            self.sink().i64_load(slot_at(RECORD_SLOTS + 8 * u64::from(f)));
            self.slot_out(ty, true);
            return;
        }
        self.expr(base);
        if base.ty == Ty::Never {
            return;
        }
        if let Rep::Fields(id) = self.sh.rep(&base.ty) {
            // A temporary: the field's value is taken from it (a pointer field's reference
            // moves out; an object in it is unboxed and dropped), the other fields dropped.
            let v = self.temp_val(&base.ty);
            self.set_val(v);
            let Val::Fields(_, fl) = v else {
                unreachable!("an unboxed struct")
            };
            let l = fl[f as usize];
            match self.sh.field_rep(ty) {
                Rep::None => {}
                _ if self.boxes(ty) => {
                    self.sink().local_get(l);
                    self.unbox_owned_top(ty);
                }
                _ => {
                    self.sink().local_get(l);
                }
            }
            for (k, t) in self.field_vals(id, fl) {
                if k != l && slot_ptr(t) {
                    self.dec(k);
                }
            }
            return;
        }
        let b = self.temp(ValType::I32);
        self.sink().local_tee(b);
        self.sink().i64_load(slot_at(RECORD_SLOTS + 8 * u64::from(f)));
        self.slot_out(ty, true);
        self.dec(b);
    }

    /// The local holding field `f` of `base`, when `base` is a read of an unboxed struct local.
    fn field_local(&self, base: &ir::Expr, f: u32) -> Option<u32> {
        match base.kind {
            ExprKind::Local(id) => match self.local(id) {
                Val::Fields(_, fl) => Some(fl[f as usize]),
                _ => None,
            },
            _ => None,
        }
    }

    /// Turns the object pointer a field local holds (a boxed enum or struct of type `t`) on the
    /// stack into the value, with a reference when `own`.
    fn unbox_field(&mut self, t: &Ty, own: bool) {
        match self.sh.rep(t) {
            Rep::Two => self.unbox_top(t, own),
            Rep::Fields(_) => self.unbox_struct_top(t, own),
            rep => unreachable!("{rep:?} is not boxed in a field local"),
        }
    }

    /// Turns the value of type `t` on the stack into what a field local of that type holds: an
    /// unboxed enum or struct boxed, anything else as is.
    fn box_field(&mut self, t: &Ty) {
        match self.sh.rep(t) {
            Rep::Two => self.box_top(t),
            Rep::Fields(_) => self.box_struct_top(t),
            _ => {}
        }
    }

    /// Pushes the pointer to the object that `e` (which `in_place` accepts, of a boxed type)
    /// names, borrowed: a local's, or a field of one read in place.
    fn borrow_ptr(&mut self, e: &ir::Expr) {
        match &e.kind {
            ExprKind::Local(id) => match self.local(*id) {
                Val::One(l) => {
                    self.sink().local_get(l);
                }
                v => unreachable!("{v:?} is not an object"),
            },
            ExprKind::Field(base, f) => {
                if let Some(l) = self.field_local(base, *f) {
                    self.sink().local_get(l);
                } else {
                    self.borrow_ptr(base);
                    self.sink()
                        .i64_load(slot_at(RECORD_SLOTS + 8 * u64::from(*f)))
                        .i32_wrap_i64();
                }
            }
            _ => unreachable!("in place"),
        }
    }

    /// The Wasm locals of `e` if it is a read of a pointer, unboxed-enum or unboxed-struct local,
    /// to borrow in place.
    fn borrowed_local(&self, e: &ir::Expr) -> Option<Val> {
        match e.kind {
            ExprKind::Local(id) if self.sh.lendable(&e.ty) => Some(self.local(id)),
            _ => None,
        }
    }

    /// Whether `e` is a local, or a field of one, so it can be read without a reference.
    fn in_place(&self, e: &ir::Expr) -> bool {
        match &e.kind {
            ExprKind::Local(id) => self.local(*id) != Val::None,
            ExprKind::Field(base, _) => self.in_place(base),
            _ => false,
        }
    }

    /// Pushes the value `e`, which `in_place` accepts, holds, borrowed: no `rt_inc`; an unboxed
    /// enum or struct field as a view of its object's tag and payload or fields.
    fn borrow(&mut self, e: &ir::Expr) {
        match &e.kind {
            ExprKind::Local(id) => {
                let v = self.local(*id);
                self.get_val(v);
            }
            ExprKind::Field(base, f) => {
                if let Some(l) = self.field_local(base, *f) {
                    if self.sh.field_rep(&e.ty) != Rep::None {
                        self.sink().local_get(l);
                        if self.boxes(&e.ty) {
                            self.unbox_field(&e.ty, false);
                        }
                    }
                } else {
                    self.borrow_ptr(base);
                    self.sink().i64_load(slot_at(RECORD_SLOTS + 8 * u64::from(*f)));
                    self.slot_out(&e.ty, false);
                }
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
            Some(v) => v,
            None => {
                let s = self.temp_val(&scrut.ty);
                self.expr(scrut);
                self.set_val(s);
                self.own(s, &scrut.ty);
                s
            }
        };
        let bt = self.block_type(ty);
        let end = self.open(Open::Block, bt);
        for arm in arms {
            let next = self.open(Open::Block, BlockType::Empty);
            let mut which = Vec::new();
            self.test(&arm.pat, &scrut.ty, s, &mut Vec::new(), next, &mut which);
            let scope = self.owned.len();
            self.bind(&arm.pat, &scrut.ty, s, &mut Vec::new(), &mut which.into_iter());
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

    /// Pushes the slot of the scrutinee in `s` that `path` (payload slot indexes, outermost
    /// first, not empty) leads to. An unboxed scrutinee's one slot is its payload local.
    fn part_slot(&mut self, s: Val, path: &[u32]) {
        let rest = match s {
            Val::One(l) => {
                self.sink()
                    .local_get(l)
                    .i64_load(slot_at(ENUM_SLOTS + 8 * u64::from(path[0])));
                &path[1..]
            }
            Val::Two(_, pay) => {
                debug_assert_eq!(path[0], 0);
                self.sink().local_get(pay);
                &path[1..]
            }
            Val::None | Val::Fields(..) => unreachable!("`()` and structs have no parts"),
        };
        for &i in rest {
            self.sink()
                .i32_wrap_i64()
                .i64_load(slot_at(ENUM_SLOTS + 8 * u64::from(i)));
        }
    }

    /// Pushes the scalar or `Str` part of the scrutinee in `s` at `path`, of type `ty`,
    /// borrowed.
    fn part_value(&mut self, s: Val, path: &[u32], ty: &Ty) {
        if path.is_empty() {
            let Val::One(l) = s else {
                unreachable!("a scalar or Str")
            };
            self.sink().local_get(l);
        } else {
            self.part_slot(s, path);
            self.slot_out(ty, false);
        }
    }

    /// Branches to `fail` unless the part at `path`, of type `ty`, matches `p`, testing a
    /// variant's number before its payload's parts, left to right. For each `|` pattern, in
    /// order, pushes to `which` a local that holds the number of its alternative that matched.
    fn test(&mut self, p: &ir::Pat, ty: &Ty, s: Val, path: &mut Vec<u32>, fail: u32, which: &mut Vec<u32>) {
        match p {
            ir::Pat::Wild | ir::Pat::Bind(_) => return,
            ir::Pat::Int(n) => {
                self.part_value(s, path, ty);
                self.sink().i64_const(*n).i64_ne();
            }
            ir::Pat::Bool(b) => {
                self.part_value(s, path, ty);
                self.sink().i32_const(*b as i32).i32_ne();
            }
            ir::Pat::Str(lit) => {
                let a = self.statics.str(lit);
                self.sink().i32_const(STR_DESC as i32);
                self.part_value(s, path, ty);
                self.sink().i64_extend_i32_u().i64_const(i64::from(a));
                self.call_rt("rt_eq");
                self.sink().i32_eqz();
            }
            ir::Pat::Variant(v, ps) => {
                // The variant: an unboxed scrutinee's tag local, else the object's word at 8.
                match (s, path.is_empty()) {
                    (Val::Two(tag, _), true) => {
                        self.sink().local_get(tag);
                    }
                    (Val::One(l), true) => {
                        self.sink().local_get(l).i32_load(VARIANT);
                    }
                    (Val::None | Val::Fields(..), true) => unreachable!("`()` and structs have no variants"),
                    (s, false) => {
                        self.part_slot(s, path);
                        self.sink().i32_wrap_i64().i32_load(VARIANT);
                    }
                }
                self.sink().i32_const(*v as i32).i32_ne();
                self.br_if(fail);
                for (i, (p, t)) in ps.iter().zip(payload(self.m, ty, *v)).enumerate() {
                    path.push(i as u32);
                    self.test(p, &t, s, path, fail, which);
                    path.pop();
                }
                return;
            }
            // A chain of tests: each alternative that fails tries the next; the last fails the
            // pattern. The first that matches records its number and skips the rest.
            ir::Pat::Or(alts) => {
                let w = self.temp(ValType::I32);
                which.push(w);
                let matched = self.open(Open::Block, BlockType::Empty);
                for (k, alt) in alts.iter().enumerate() {
                    let last = k + 1 == alts.len();
                    let next = if last {
                        fail
                    } else {
                        self.open(Open::Block, BlockType::Empty)
                    };
                    self.test(alt, ty, s, path, next, which);
                    self.sink().i32_const(k as i32).local_set(w);
                    self.br(matched);
                    if !last {
                        self.close();
                    }
                }
                self.close();
                return;
            }
        }
        self.br_if(fail);
    }

    /// Sets each binding in `p`, which matched, to a new reference to its part, left to right,
    /// and registers it in `owned`. `which` yields the locals `test` pushed, in its order.
    fn bind(&mut self, p: &ir::Pat, ty: &Ty, s: Val, path: &mut Vec<u32>, which: &mut impl Iterator<Item = u32>) {
        match p {
            ir::Pat::Bind(id) => {
                let l = self.local(*id);
                if l == Val::None {
                    return;
                }
                if path.is_empty() {
                    self.copy_val(s, ty);
                } else {
                    self.part_slot(s, path);
                    self.slot_out(ty, true);
                }
                self.set_val(l);
                self.own(l, ty);
            }
            ir::Pat::Variant(v, ps) => {
                for (i, (p, t)) in ps.iter().zip(payload(self.m, ty, *v)).enumerate() {
                    path.push(i as u32);
                    self.bind(p, &t, s, path, which);
                    path.pop();
                }
            }
            // The bindings of the alternative that matched; every alternative binds the same
            // locals, so the arm's body is shared.
            ir::Pat::Or(alts) => {
                let w = which.next().expect("one local per `|` pattern");
                for (k, alt) in alts.iter().enumerate() {
                    self.sink().local_get(w).i32_const(k as i32).i32_eq();
                    self.open(Open::If, BlockType::Empty);
                    self.bind(alt, ty, s, path, which);
                    self.close();
                }
            }
            ir::Pat::Wild | ir::Pat::Int(_) | ir::Pat::Bool(_) | ir::Pat::Str(_) => {}
        }
    }

    /// `x?` on an unboxed `Option` or `Result`: on `Some` or `Ok`, the payload (a temporary's
    /// moved out, a local's copied), and a temporary `x` dropped. On `None` or `Err`, every
    /// owned local released and the function returns `(0, 0)` (`None`) or `(1, error)` as its
    /// own return type, the error moved out of a temporary or copied from a local.
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
        let s = match borrowed {
            Some(v) => v,
            None => {
                let v = self.temp_val(&x.ty);
                self.expr(x);
                self.set_val(v);
                self.own(v, &x.ty);
                v
            }
        };
        let Val::Two(tag, pay) = s else {
            unreachable!("an Option or Result")
        };
        let temp = match borrowed {
            None => self.owned_of(s, &x.ty).pop(),
            Some(_) => None,
        };
        self.sink().local_get(tag).i32_const(success).i32_ne();
        self.open(Open::If, BlockType::Empty);
        match &x.ty {
            Ty::Option(_) => {
                self.sink().i32_const(0).i64_const(0);
            }
            Ty::Result(_, err) => {
                debug_assert!(matches!(self.ret, Ty::Result(_, e) if e == err));
                self.sink().i32_const(1);
                if borrowed.is_some() {
                    self.slot_copy(pay, err);
                } else {
                    self.sink().local_get(pay).i64_const(0).local_set(pay);
                }
            }
            t => unreachable!("`?` on {t:?}"),
        }
        self.release(0);
        self.sink().return_();
        self.close();
        self.sink().local_get(pay);
        if borrowed.is_none() && self.sh.is_ptr(ty) {
            // The temporary's payload moves out: nothing left to drop.
            self.sink().i32_wrap_i64().i64_const(0).local_set(pay);
        } else {
            self.slot_out(ty, true);
        }
        if let Some(o) = temp {
            self.owned.pop();
            if !self.sh.is_ptr(ty) {
                self.dec_owned(o);
            }
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
/// handle of grant `i` (`rt_grant(i, kind)`) as its parameter `i`, return its value as a slot;
/// an unboxed enum (of descriptor `desc`) boxed, for the runtime to display.
fn entry(rt: &Runtime, sh: &Shapes, table: u32, index: u32, main: &ir::Func, desc: u32) -> Function {
    // A tag, an object and a payload, for the boxing; then a local per field of an unboxed
    // struct result.
    let (tag, o, pay) = (0, 1, 2);
    let mut locals = vec![(2, ValType::I32), (1, ValType::I64)];
    let fl = match sh.rep(&main.ret) {
        Rep::Fields(id) => {
            let mut next = 3;
            Some(field_locals(sh, id, |vt| {
                locals.push((1, vt));
                (next, next += 1).0
            }))
        }
        _ => None,
    };
    let mut f = Function::new(locals);
    let mut s = f.instructions();
    s.i32_const(table as i32).call(rt.export("rt_init"));
    for (i, l) in main.locals[..main.params as usize].iter().enumerate() {
        let Ty::Cap(kind) = l.ty else {
            unreachable!("main takes capabilities only")
        };
        s.i32_const(i as i32)
            .i32_const(kind.0 as i32)
            .call(rt.export("rt_grant"));
    }
    s.call(index);
    match sh.rep(&main.ret) {
        Rep::One(ValType::I64) => {}
        Rep::One(ValType::F64) => {
            s.i64_reinterpret_f64();
        }
        Rep::One(_) => {
            s.i64_extend_i32_u();
        }
        Rep::None => {
            s.i64_const(0);
        }
        Rep::Two => {
            // `rt_alloc` sizes the object for the variant; only one with a payload has the slot.
            let m = sh.masks(&main.ret);
            s.local_set(pay).local_set(tag);
            s.i32_const(desc as i32)
                .local_get(tag)
                .call(rt.export("rt_alloc"))
                .local_set(o);
            if m.payload != 0 {
                let test = m.payload != m.all();
                if test {
                    in_mask(&mut s, tag, m.payload);
                    s.if_(BlockType::Empty);
                }
                s.local_get(o).local_get(pay).i64_store(slot_at(ENUM_SLOTS));
                if test {
                    s.end();
                }
            }
            s.local_get(o).i64_extend_i32_u();
        }
        Rep::Fields(id) => {
            // `rt_alloc` zeroes the slots, so a `()` field's stays 0.
            let fl = fl.expect("locals for the fields");
            for (i, t) in sh.field_tys(id).enumerate().rev() {
                if sh.field_rep(t) != Rep::None {
                    s.local_set(fl[i]);
                }
            }
            s.i32_const(desc as i32)
                .i32_const(0)
                .call(rt.export("rt_alloc"))
                .local_set(o);
            store_fields(&mut s, sh, id, &fl, o);
            s.local_get(o).i64_extend_i32_u();
        }
    }
    s.end();
    f
}

/// The body of `kio_main_desc`: the descriptor of main's value, which the runtime's `run`
/// displays (a `Result` that is `Err` as a failure showing its error).
fn entry_desc(desc: u32) -> Function {
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
fn wrapper(rt: &Runtime, sh: &Shapes, f: ir::FuncId, captures: usize, lent: &[bool]) -> (FuncSig, Function) {
    let func = &sh.m.funcs[f.0 as usize];
    let (caps, params) = func.locals[..func.params as usize].split_at(captures);
    let params: Vec<Ty> = params.iter().map(|l| l.ty.clone()).collect();
    let sig = value_sig(sh, &params, &func.ret);
    // Scratch after the parameters: a pointer and its count for the inline `inc` and `dec`, a
    // tag and a second pointer, and a payload, for unboxing a capture (C8c1).
    let n = sig.0.len() as u32;
    let (p, r, tag, q, pay) = (n, n + 1, n + 2, n + 3, n + 4);
    let mut body = Function::new([(4, ValType::I32), (1, ValType::I64)]);
    let mut s = body.instructions();
    for (i, l) in caps.iter().enumerate() {
        let at = slot_at(ENUM_SLOTS + 8 * i as u64);
        match sh.rep(&l.ty) {
            Rep::None => {}
            Rep::Two => {
                s.local_get(0).i64_load(at).i32_wrap_i64().local_set(p);
                unbox(&mut s, rt, sh.masks(&l.ty), p, !lent[i], [tag, pay, q, r]);
            }
            Rep::Fields(id) => {
                s.local_get(0).i64_load(at).i32_wrap_i64().local_set(q);
                unbox_struct(&mut s, rt, sh, id, q, !lent[i], [p, r]);
            }
            rep => {
                if sh.is_ptr(&l.ty) && !lent[i] {
                    s.local_get(0).i64_load(at).i32_wrap_i64().local_set(p);
                    inc(&mut s, rt, p, r);
                }
                s.local_get(0).i64_load(at);
                slot_out(&mut s, rep);
            }
        }
    }
    for i in 1..n {
        s.local_get(i);
    }
    s.call(rt.funcs + f.0);
    // The result stays on the stack under the drops.
    let mut w = 0;
    for (l, &lent) in func.locals[captures..func.params as usize]
        .iter()
        .zip(&lent[captures..])
    {
        match sh.rep(&l.ty) {
            Rep::None => {}
            Rep::One(_) => {
                w += 1;
                if lent {
                    dec(&mut s, rt, w, r);
                }
            }
            Rep::Two => {
                w += 2;
                let m = sh.masks(&l.ty);
                if lent && m.ptr != 0 {
                    let test = m.tested();
                    if let Some(mask) = test {
                        in_mask(&mut s, w - 1, mask);
                        s.if_(BlockType::Empty);
                    }
                    s.local_get(w).i32_wrap_i64().local_set(p);
                    dec(&mut s, rt, p, r);
                    if test.is_some() {
                        s.end();
                    }
                }
            }
            Rep::Fields(id) => {
                for t in sh.field_tys(id) {
                    if sh.field_rep(t) == Rep::None {
                        continue;
                    }
                    w += 1;
                    if lent && slot_ptr(t) {
                        dec(&mut s, rt, w, r);
                    }
                }
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
