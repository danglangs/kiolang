//! The program's static data: string literals and the descriptor table, laid out in one data
//! segment at `base` (docs/codegen.md, "How it is built"):
//!
//! ```text
//! base   literals, each a pinned Str [u32::MAX][STR][len][len][bytes], padded to 8 bytes,
//!        payload-free variants, each a pinned enum [u32::MAX][desc][variant][0], and
//!        functions used as values without captures, each [u32::MAX][desc][table index][0]
//! table  RawTable: (address, count) of each array below, in this order
//!        descs     [tag, 4 words] per descriptor: runtime/src/desc.rs `Desc`, repr(C, u32)
//!        slots     u32 per record or variant slot
//!        fields    [off, len] per slot: a record field's name, or empty
//!        variants  [name off, name len, slots start, slots len] per variant
//!        names     bytes
//! ```
//!
//! A literal's address is fixed when it is added and the table goes last, so both can grow
//! until `finish`.
//!
//! Also the program's entries in the Wasm table, from `table` on: one per function used as a
//! value, which codegen fills with a wrapper calling it (lib.rs `wrapper`).

use lang::ir::{Block, Expr, ExprKind, FuncId, Module, Pat, PlaceStep, StmtKind, Ty};
use std::collections::HashMap;

// `Desc` tags, in declaration order; the first six (`Unit` to `Float`) are also the scalar
// descriptors' indexes.
const STR: u32 = 3;
const SCALARS: u32 = 6;
const LIST: u32 = 6;
const MAP: u32 = 7;
const RECORD: u32 = 8;
const ENUM: u32 = 9;
const FN: u32 = 10;
const HASH_MAP: u32 = 11;

pub struct Statics<'m> {
    m: &'m Module,
    base: u32,
    literals: Vec<u8>,
    strs: HashMap<String, u32>,
    /// The address of each enum descriptor's table of pinned variants (`pins`).
    pins: HashMap<u32, u32>,
    /// The descriptor of each type that has one beyond the scalars, by the type's `Debug` text
    /// (`ir::Ty` has no `Hash`; its `Debug` is structural, so equal types have equal text).
    tys: HashMap<String, u32>,
    descs: Vec<[u32; 5]>,
    slots: Vec<u32>,
    fields: Vec<[u32; 2]>,
    variants: Vec<[u32; 4]>,
    names: Vec<u8>,
    interned: HashMap<String, [u32; 2]>,
    /// The first table entry the program's functions take.
    table: u32,
    /// Each function used as a value, in table order, with its captures' count.
    elems: Vec<(FuncId, usize)>,
    /// The descriptor and table entry of each function in `elems`, by its id.
    funcs: HashMap<u32, (u32, u32)>,
    /// The address of the pinned object of each function used as a value without captures.
    fn_pins: HashMap<u32, u32>,
}

impl<'m> Statics<'m> {
    /// The scalar descriptors, and every type and string literal in `m`; table entries from
    /// `table` on.
    pub fn new(m: &'m Module, base: u32, table: u32) -> Self {
        let mut s = Statics {
            m,
            base,
            literals: Vec::new(),
            strs: HashMap::new(),
            pins: HashMap::new(),
            tys: HashMap::new(),
            descs: (0..SCALARS).map(|tag| [tag, 0, 0, 0, 0]).collect(),
            slots: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
            names: Vec::new(),
            interned: HashMap::new(),
            table,
            elems: Vec::new(),
            funcs: HashMap::new(),
            fn_pins: HashMap::new(),
        };
        for f in &m.funcs {
            for l in &f.locals {
                s.ty(&l.ty);
            }
            s.ty(&f.ret);
            s.block(&f.body);
        }
        s
    }

    /// The descriptor index of `t`, added with its parts if new.
    pub fn desc(&mut self, t: &Ty) -> u32 {
        match t {
            Ty::Unit => return 0,
            Ty::Int => return 1,
            Ty::Bool => return 2,
            Ty::Float => return 5,
            Ty::Str => return STR,
            Ty::Bytes => return 4,
            Ty::Cap(_) | Ty::Never => unreachable!("{t:?} has no descriptor"),
            _ => {}
        }
        let key = format!("{t:?}");
        if let Some(&i) = self.tys.get(&key) {
            return i;
        }
        // Reserved first, so a recursive type's parts can refer to it; filled after them.
        let i = self.descs.len() as u32;
        self.descs.push([0; 5]);
        self.tys.insert(key, i);
        let m = self.m;
        self.descs[i as usize] = match t {
            Ty::List(e) => [LIST, self.desc(e), 0, 0, 0],
            Ty::Map(k, v) => [MAP, self.desc(k), self.desc(v), 0, 0],
            Ty::HashMap(k, v) => [HASH_MAP, self.desc(k), self.desc(v), 0, 0],
            Ty::Struct(id) => {
                let s = &m.structs[id.0 as usize];
                let slots: Vec<u32> = s.fields.iter().map(|(_, t)| self.desc(t)).collect();
                let [off, len] = self.name(&s.name);
                let start = self.slots.len() as u32;
                for (f, _) in &s.fields {
                    let n = self.name(f);
                    self.fields.push(n);
                }
                self.slots.extend(slots);
                [RECORD, off, len, start, s.fields.len() as u32]
            }
            Ty::Enum(id) => {
                let e = &m.enums[id.0 as usize];
                self.enumeration(&e.name, &e.variants)
            }
            Ty::Option(t) => self.enumeration(
                "Option",
                &[("None".into(), vec![]), ("Some".into(), vec![(**t).clone()])],
            ),
            Ty::Result(t, e) => self.enumeration(
                "Result",
                &[("Ok".into(), vec![(**t).clone()]), ("Err".into(), vec![(**e).clone()])],
            ),
            // Any `fn` value: the object's own descriptor names it and its captures.
            Ty::Fn(..) => [FN, 0, 0, 0, 0],
            _ => unreachable!("a scalar"),
        };
        i
    }

    /// The address of the pinned `Str` holding `s`, added if new.
    pub fn str(&mut self, s: &str) -> u32 {
        if let Some(&a) = self.strs.get(s) {
            return a;
        }
        let a = self.base + self.literals.len() as u32;
        let len = s.len() as u32;
        for w in [u32::MAX, STR, len, len] {
            self.literals.extend(w.to_le_bytes());
        }
        self.literals.extend(s.as_bytes());
        self.literals.resize(self.literals.len().next_multiple_of(8), 0);
        self.strs.insert(s.to_string(), a);
        a
    }

    /// The address of the pinned object for variant `variant`, which has no payload, of enum
    /// type `t`: its entry in the type's table (`pins`).
    pub fn pinned(&mut self, t: &Ty, variant: u32) -> u32 {
        self.pins(t) + 16 * variant
    }

    /// The address of enum type `t`'s table of pinned variants, added if new: a 16-byte entry
    /// per variant, in order, a pinned object `[u32::MAX][desc][variant][0]` for a payload-free
    /// variant and zeros (never used: such a variant is allocated) for one with a payload.
    /// Generated code boxes an unboxed enum's payload-free variant by indexing it (C8c1).
    pub fn pins(&mut self, t: &Ty) -> u32 {
        let desc = self.desc(t);
        if let Some(&a) = self.pins.get(&desc) {
            return a;
        }
        let a = self.base + self.literals.len() as u32;
        let payloads: Vec<bool> = match t {
            Ty::Option(_) => vec![false, true],
            Ty::Result(..) => vec![true, true],
            Ty::Enum(id) => self.m.enums[id.0 as usize]
                .variants
                .iter()
                .map(|(_, ts)| !ts.is_empty())
                .collect(),
            t => unreachable!("{t:?} is not an enum"),
        };
        for (v, has_payload) in payloads.into_iter().enumerate() {
            let words = match has_payload {
                true => [0; 4],
                false => [u32::MAX, desc, v as u32, 0],
            };
            for w in words {
                self.literals.extend(w.to_le_bytes());
            }
        }
        self.pins.insert(desc, a);
        a
    }

    /// The descriptor and table entry of function `f` as a value: a function item (`captures`
    /// `None`), displayed `<fn name>`, or a closure lifted to `f` whose first `n` parameters
    /// are its captures, displayed `<closure>`, its slots the captures. Added if new.
    pub fn func(&mut self, f: FuncId, captures: Option<usize>) -> (u32, u32) {
        if let Some(&d) = self.funcs.get(&f.0) {
            return d;
        }
        let func = &self.m.funcs[f.0 as usize];
        let n = captures.unwrap_or(0);
        let slots: Vec<u32> = func.locals[..n].iter().map(|l| self.desc(&l.ty)).collect();
        let [off, len] = match captures {
            None => self.name(&format!("<fn {}>", func.name)),
            Some(_) => self.name("<closure>"),
        };
        let start = self.slots.len() as u32;
        self.fields.extend(slots.iter().map(|_| [0, 0]));
        self.slots.extend(slots);
        let desc = self.descs.len() as u32;
        self.descs.push([FN, off, len, start, n as u32]);
        let elem = self.table + self.elems.len() as u32;
        self.elems.push((f, n));
        self.funcs.insert(f.0, (desc, elem));
        (desc, elem)
    }

    /// The address of the pinned object of function `f` as a value with no captures (a
    /// function item, or a closure that captures nothing), added if new.
    pub fn pinned_fn(&mut self, f: FuncId, captures: Option<usize>) -> u32 {
        if let Some(&a) = self.fn_pins.get(&f.0) {
            return a;
        }
        let (desc, elem) = self.func(f, captures);
        let a = self.base + self.literals.len() as u32;
        for w in [u32::MAX, desc, elem, 0] {
            self.literals.extend(w.to_le_bytes());
        }
        self.fn_pins.insert(f.0, a);
        a
    }

    /// The slot count of a struct, of variant `variant` of an enum, or of a closure, as the
    /// runtime reads it from the table (its object's size follows from it); `None` for a
    /// variant the enum does not have, or a descriptor with no slots.
    pub fn slots(&self, desc: u32, variant: u32) -> Option<usize> {
        let d = self.descs[desc as usize];
        match d[0] {
            RECORD | FN => Some(d[4] as usize),
            ENUM if variant < d[4] => Some(self.variants[(d[3] + variant) as usize][3] as usize),
            _ => None,
        }
    }

    /// Each function used as a value so far, in table order, and its captures' count.
    pub fn elems(&self) -> &[(FuncId, usize)] {
        &self.elems
    }

    /// The data, to be placed at `base`, and the address of its `RawTable` for `rt_init`.
    pub fn finish(self) -> (Vec<u8>, u32) {
        let table = self.base + self.literals.len() as u32;
        let descs = table + 40;
        let slots = descs + 20 * self.descs.len() as u32;
        let fields = slots + 4 * self.slots.len() as u32;
        let variants = fields + 8 * self.fields.len() as u32;
        let names = variants + 16 * self.variants.len() as u32;
        let raw = [
            descs,
            self.descs.len() as u32,
            slots,
            self.slots.len() as u32,
            fields,
            self.fields.len() as u32,
            variants,
            self.variants.len() as u32,
            names,
            self.names.len() as u32,
        ];
        let words = raw
            .into_iter()
            .chain(self.descs.into_iter().flatten())
            .chain(self.slots)
            .chain(self.fields.into_iter().flatten())
            .chain(self.variants.into_iter().flatten());
        let mut data = self.literals;
        data.extend(words.flat_map(u32::to_le_bytes));
        data.extend(self.names);
        (data, table)
    }

    fn enumeration(&mut self, name: &str, variants: &[(String, Vec<Ty>)]) -> [u32; 5] {
        let slots: Vec<Vec<u32>> = variants
            .iter()
            .map(|(_, ts)| ts.iter().map(|t| self.desc(t)).collect())
            .collect();
        let [off, len] = self.name(name);
        let start = self.variants.len() as u32;
        for ((v, _), slots) in variants.iter().zip(slots) {
            let [voff, vlen] = self.name(v);
            self.variants
                .push([voff, vlen, self.slots.len() as u32, slots.len() as u32]);
            self.fields.extend(slots.iter().map(|_| [0, 0]));
            self.slots.extend(slots);
        }
        [ENUM, off, len, start, variants.len() as u32]
    }

    fn name(&mut self, s: &str) -> [u32; 2] {
        if let Some(&n) = self.interned.get(s) {
            return n;
        }
        let n = [self.names.len() as u32, s.len() as u32];
        self.names.extend(s.as_bytes());
        self.interned.insert(s.to_string(), n);
        n
    }

    /// Adds `t`'s descriptor, if it is a value's type.
    fn ty(&mut self, t: &Ty) {
        if !matches!(t, Ty::Cap(_) | Ty::Never) {
            self.desc(t);
        }
    }

    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let(_, e) | StmtKind::Return(Some(e)) | StmtKind::Expr(e) => self.expr(e),
                StmtKind::Assign(place, _, e) => {
                    self.steps(&place.steps);
                    self.expr(e);
                }
                StmtKind::While(e, b) | StmtKind::ForList(_, e, b) => {
                    self.expr(e);
                    self.block(b);
                }
                StmtKind::ForRange(_, a, e, b) => {
                    self.expr(a);
                    self.expr(e);
                    self.block(b);
                }
                StmtKind::Loop(b) => self.block(b),
                StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            }
        }
        if let Some(e) = &b.tail {
            self.expr(e);
        }
    }

    fn steps(&mut self, steps: &[PlaceStep]) {
        for s in steps {
            if let PlaceStep::Index(e, _) = s {
                self.expr(e);
            }
        }
    }

    fn pat(&mut self, p: &Pat) {
        match p {
            Pat::Str(s) => {
                self.str(s);
            }
            Pat::Variant(_, ps) | Pat::Or(ps) => ps.iter().for_each(|p| self.pat(p)),
            Pat::Wild | Pat::Bind(_) | Pat::Int(_) | Pat::Bool(_) => {}
        }
    }

    fn expr(&mut self, e: &Expr) {
        self.ty(&e.ty);
        let exprs = |xs: &[Expr], s: &mut Self| xs.iter().for_each(|x| s.expr(x));
        match &e.kind {
            ExprKind::Str(s) => {
                self.str(s);
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit
            | ExprKind::Local(_)
            | ExprKind::MapNew
            | ExprKind::FnRef(_)
            | ExprKind::Closure(..) => {}
            ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) => self.expr(x),
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Assert(a, b) | ExprKind::AssertEq(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::If(c, then, els) => {
                self.expr(c);
                self.block(then);
                if let Some(x) = els {
                    self.expr(x);
                }
            }
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(_, xs) | ExprKind::CapCall(_, _, xs) | ExprKind::List(xs) | ExprKind::Variant(_, _, xs) => {
                exprs(xs, self)
            }
            ExprKind::Builtin(_, x, xs) | ExprKind::CallValue(x, xs) => {
                self.expr(x);
                exprs(xs, self);
            }
            ExprKind::Mutate(place, _, xs) => {
                self.steps(&place.steps);
                exprs(xs, self);
            }
            ExprKind::StructLit(_, inits) => inits.iter().for_each(|(_, x)| self.expr(x)),
            ExprKind::Match(x, arms) => {
                self.expr(x);
                for arm in arms {
                    self.pat(&arm.pat);
                    self.expr(&arm.body);
                }
            }
        }
    }
}
