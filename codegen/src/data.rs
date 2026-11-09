//! The program's static data: string literals and the descriptor table, laid out in one data
//! segment at `base` (docs/codegen.md, "How it is built"):
//!
//! ```text
//! base   literals, each a pinned Str [u32::MAX][STR][len][len][bytes], padded to 8 bytes
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

use lang::ir::{Block, Expr, ExprKind, Module, Pat, PlaceStep, StmtKind, Ty};
use std::collections::HashMap;

// `Desc` tags, in declaration order; the first five are also the scalar descriptors' indexes.
const STR: u32 = 3;
const LIST: u32 = 5;
const MAP: u32 = 6;
const RECORD: u32 = 7;
const ENUM: u32 = 8;

pub struct Statics<'m> {
    m: &'m Module,
    base: u32,
    literals: Vec<u8>,
    strs: HashMap<String, u32>,
    /// The descriptor of each type that has one beyond the scalars.
    tys: Vec<(Ty, u32)>,
    descs: Vec<[u32; 5]>,
    slots: Vec<u32>,
    fields: Vec<[u32; 2]>,
    variants: Vec<[u32; 4]>,
    names: Vec<u8>,
    interned: HashMap<String, [u32; 2]>,
}

impl<'m> Statics<'m> {
    /// The scalar descriptors, and every type and string literal in `m`.
    pub fn new(m: &'m Module, base: u32) -> Self {
        let mut s = Statics {
            m,
            base,
            literals: Vec::new(),
            strs: HashMap::new(),
            tys: Vec::new(),
            descs: (0..5).map(|tag| [tag, 0, 0, 0, 0]).collect(),
            slots: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
            names: Vec::new(),
            interned: HashMap::new(),
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
            Ty::Str => return STR,
            Ty::Bytes => return 4,
            Ty::Cap(_) | Ty::Never => unreachable!("{t:?} has no descriptor"),
            _ => {}
        }
        if let Some((_, i)) = self.tys.iter().find(|(u, _)| u == t) {
            return *i;
        }
        // Reserved first, so a recursive type's parts can refer to it; filled after them.
        let i = self.descs.len() as u32;
        self.descs.push([0; 5]);
        self.tys.push((t.clone(), i));
        let m = self.m;
        self.descs[i as usize] = match t {
            Ty::List(e) => [LIST, self.desc(e), 0, 0, 0],
            Ty::Map(k, v) => [MAP, self.desc(k), self.desc(v), 0, 0],
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

    fn expr(&mut self, e: &Expr) {
        self.ty(&e.ty);
        let exprs = |xs: &[Expr], s: &mut Self| xs.iter().for_each(|x| s.expr(x));
        match &e.kind {
            ExprKind::Str(s) => {
                self.str(s);
            }
            ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Unit | ExprKind::Local(_) | ExprKind::MapNew => {}
            ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) => self.expr(x),
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
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
            ExprKind::Builtin(_, x, xs) => {
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
                    if let Pat::Str(s) = &arm.pat {
                        self.str(s);
                    }
                    self.expr(&arm.body);
                }
            }
        }
    }
}
