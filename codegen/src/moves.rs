//! Last uses (C8b2): which reads of a local may move its reference rather than copy it.
//!
//! A read of a pointer local is a **move** when, after it, nothing reads the local's value on
//! any path: the function returns, the local's scope ends, or a plain `x = e` overwrites it
//! first. Codegen then hands over the local's reference and zeroes the slot (`FuncGen::expr`),
//! so the decs at the scope's end and on every exit find 0, a no-op.
//!
//! A backward pass over each function's IR in evaluation order, keeping the set of locals whose
//! value may still be read (live). Branches (`if`, `&&`, `||`, `match` arms) are analysed each
//! from the set after them and joined. Loops are conservative: inside a loop, every local the
//! loop mentions and does not declare is live at the end of a pass, and at `break` and
//! `continue`, so a read of a local declared outside a loop is never a move from inside it.

use lang::ir::{self, Expr, ExprKind, LocalId, Pat, PlaceStep, StmtKind};
use std::collections::HashSet;

/// The reads (`ExprKind::Local` nodes, by address) of `f` that may move.
pub(crate) fn moves(f: &ir::Func, is_ptr: fn(&ir::Ty) -> bool) -> HashSet<*const Expr> {
    let mut a = Analysis {
        f,
        is_ptr,
        moves: HashSet::new(),
        loops: Vec::new(),
    };
    let mut live = Live::new(f.locals.len());
    a.block(&f.body, &mut live);
    a.moves
}

/// A set of locals, by id.
#[derive(Clone)]
struct Live(Vec<u64>);

impl Live {
    fn new(n: usize) -> Live {
        Live(vec![0; n.div_ceil(64)])
    }
    fn insert(&mut self, id: LocalId) {
        self.0[id.0 as usize / 64] |= 1 << (id.0 % 64);
    }
    fn remove(&mut self, id: LocalId) {
        self.0[id.0 as usize / 64] &= !(1 << (id.0 % 64));
    }
    fn contains(&self, id: LocalId) -> bool {
        self.0[id.0 as usize / 64] & (1 << (id.0 % 64)) != 0
    }
    fn union(&mut self, other: &Live) {
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a |= b;
        }
    }
    fn clear(&mut self) {
        self.0.iter_mut().for_each(|w| *w = 0);
    }
}

struct Analysis<'f> {
    f: &'f ir::Func,
    is_ptr: fn(&ir::Ty) -> bool,
    moves: HashSet<*const Expr>,
    /// For each enclosing loop, innermost last: what is live at the end of a pass.
    loops: Vec<Live>,
}

impl Analysis<'_> {
    /// `live` is the set after `b` on entry and the set before it on return; so for each below.
    fn block(&mut self, b: &ir::Block, live: &mut Live) {
        if let Some(t) = &b.tail {
            self.expr(t, live);
        }
        for s in b.stmts.iter().rev() {
            self.stmt(s, live);
        }
    }

    fn stmt(&mut self, s: &ir::Stmt, live: &mut Live) {
        match &s.kind {
            StmtKind::Let(id, init) => {
                live.remove(*id);
                self.expr(init, live);
            }
            // Indexes, then the value, then the walk from the root, which reads it.
            StmtKind::Assign(place, _, value) if !place.steps.is_empty() => {
                live.insert(place.root);
                self.expr(value, live);
                self.indexes(place, live);
            }
            // The value, then the old value dropped: dead before the store.
            StmtKind::Assign(place, None, value) => {
                live.remove(place.root);
                self.expr(value, live);
            }
            // The value, then the old value read.
            StmtKind::Assign(place, Some(_), value) => {
                live.insert(place.root);
                self.expr(value, live);
            }
            StmtKind::Return(value) => {
                live.clear();
                if let Some(e) = value {
                    self.expr(e, live);
                }
            }
            // The condition runs each pass, but a `break` or `continue` in it is the enclosing
            // loop's, as in codegen.
            StmtKind::While(cond, body) => {
                let end = self.loop_live(live, Some(cond), body, None);
                self.loops.push(end.clone());
                let mut l = end.clone();
                self.block(body, &mut l);
                self.loops.pop();
                l.union(&end);
                self.expr(cond, &mut l);
                live.union(&l);
            }
            StmtKind::Loop(body) => {
                let end = self.loop_live(live, None, body, None);
                self.loops.push(end.clone());
                let mut l = end.clone();
                self.block(body, &mut l);
                self.loops.pop();
                live.union(&l);
                live.union(&end);
            }
            // The iterable (or the bounds) once before the loop; `x` set at each pass.
            StmtKind::ForList(x, xs, body) => {
                self.for_body(*x, body, live);
                self.expr(xs, live);
            }
            StmtKind::ForRange(i, a, b, body) => {
                self.for_body(*i, body, live);
                self.expr(b, live);
                self.expr(a, live);
            }
            StmtKind::Break | StmtKind::Continue => {
                *live = self.loops.last().expect("checked loop").clone();
            }
            StmtKind::Expr(e) => self.expr(e, live),
        }
    }

    fn for_body(&mut self, x: LocalId, body: &ir::Block, live: &mut Live) {
        let end = self.loop_live(live, None, body, Some(x));
        self.loops.push(end.clone());
        let mut l = end.clone();
        self.block(body, &mut l);
        self.loops.pop();
        live.union(&l);
        live.union(&end);
        live.remove(x);
    }

    /// What is live at the end of a pass of a loop over `cond` and `body` that `after` follows:
    /// `after`, and every local the loop mentions but does not declare (`x` is the `for`
    /// loop's own).
    fn loop_live(&self, after: &Live, cond: Option<&Expr>, body: &ir::Block, x: Option<LocalId>) -> Live {
        let n = self.f.locals.len();
        let (mut used, mut declared) = (Live::new(n), Live::new(n));
        let mut see = |id: LocalId, decl: bool| match decl {
            true => declared.insert(id),
            false => used.insert(id),
        };
        if let Some(c) = cond {
            each_local(c, &mut see);
        }
        each_local_block(body, &mut see);
        if let Some(x) = x {
            declared.insert(x);
        }
        let mut end = after.clone();
        for (e, (u, d)) in end.0.iter_mut().zip(used.0.iter().zip(&declared.0)) {
            *e |= u & !d;
        }
        end
    }

    fn indexes(&mut self, place: &ir::Place, live: &mut Live) {
        for step in place.steps.iter().rev() {
            if let PlaceStep::Index(i, _) = step {
                self.expr(i, live);
            }
        }
    }

    fn exprs(&mut self, xs: &[Expr], live: &mut Live) {
        for x in xs.iter().rev() {
            self.expr(x, live);
        }
    }

    fn expr(&mut self, e: &Expr, live: &mut Live) {
        match &e.kind {
            ExprKind::Local(id) => {
                if !live.contains(*id) && (self.is_ptr)(&e.ty) {
                    self.moves.insert(e);
                }
                live.insert(*id);
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Unit
            | ExprKind::FnRef(_)
            | ExprKind::MapNew => {}
            ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) => self.expr(x, live),
            // The right side runs only on some paths.
            ExprKind::Binary(ir::BinOp::And | ir::BinOp::Or, a, b) => {
                let mut r = live.clone();
                self.expr(b, &mut r);
                live.union(&r);
                self.expr(a, live);
            }
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
                self.expr(b, live);
                self.expr(a, live);
            }
            ExprKind::If(cond, then, els) => {
                let mut t = live.clone();
                self.block(then, &mut t);
                if let Some(els) = els {
                    self.expr(els, live);
                }
                live.union(&t);
                self.expr(cond, live);
            }
            ExprKind::Block(b) => self.block(b, live),
            ExprKind::Call(_, args) | ExprKind::List(args) | ExprKind::Variant(_, _, args) => self.exprs(args, live),
            ExprKind::CallValue(callee, args) => {
                self.exprs(args, live);
                self.expr(callee, live);
            }
            ExprKind::Closure(_, captured) => captured.iter().for_each(|&id| live.insert(id)),
            ExprKind::CapCall(cap, _, args) => {
                live.insert(*cap);
                self.exprs(args, live);
            }
            ExprKind::Builtin(_, recv, args) => {
                self.exprs(args, live);
                self.expr(recv, live);
            }
            // Indexes, then arguments, then the walk from the root.
            ExprKind::Mutate(place, _, args) => {
                live.insert(place.root);
                self.exprs(args, live);
                self.indexes(place, live);
            }
            ExprKind::StructLit(_, inits) => {
                for (_, x) in inits.iter().rev() {
                    self.expr(x, live);
                }
            }
            // One arm runs, after the scrutinee; a binding is set before its arm's body.
            ExprKind::Match(scrut, arms) => {
                let mut joined = Live::new(self.f.locals.len());
                for arm in arms {
                    let mut l = live.clone();
                    self.expr(&arm.body, &mut l);
                    binds(&arm.pat, &mut |id| l.remove(id));
                    joined.union(&l);
                }
                *live = joined;
                self.expr(scrut, live);
            }
        }
    }
}

fn binds(p: &Pat, f: &mut impl FnMut(LocalId)) {
    match p {
        Pat::Bind(id) => f(*id),
        Pat::Variant(_, ps) => ps.iter().for_each(|p| binds(p, f)),
        Pat::Wild | Pat::Int(_) | Pat::Str(_) | Pat::Bool(_) => {}
    }
}

/// Whether `e` reads, writes or declares local `id` anywhere in it.
pub(crate) fn mentions(e: &Expr, id: LocalId) -> bool {
    let mut found = false;
    each_local(e, &mut |l, _| found |= l == id);
    found
}

/// Calls `f` with every local `e` mentions, and whether that mention declares it (`let`, a
/// `for` variable, a pattern's binding).
fn each_local(e: &Expr, f: &mut impl FnMut(LocalId, bool)) {
    match &e.kind {
        ExprKind::Local(id) => f(*id, false),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Unit
        | ExprKind::FnRef(_)
        | ExprKind::MapNew => {}
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) => each_local(x, f),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
            each_local(a, f);
            each_local(b, f);
        }
        ExprKind::If(cond, then, els) => {
            each_local(cond, f);
            each_local_block(then, f);
            if let Some(els) = els {
                each_local(els, f);
            }
        }
        ExprKind::Block(b) => each_local_block(b, f),
        ExprKind::Call(_, args) | ExprKind::List(args) | ExprKind::Variant(_, _, args) => {
            args.iter().for_each(|x| each_local(x, f))
        }
        ExprKind::CallValue(callee, args) | ExprKind::Builtin(_, callee, args) => {
            each_local(callee, f);
            args.iter().for_each(|x| each_local(x, f));
        }
        ExprKind::Closure(_, captured) => captured.iter().for_each(|&id| f(id, false)),
        ExprKind::CapCall(cap, _, args) => {
            f(*cap, false);
            args.iter().for_each(|x| each_local(x, f));
        }
        ExprKind::Mutate(place, _, args) => {
            each_local_place(place, f);
            args.iter().for_each(|x| each_local(x, f));
        }
        ExprKind::StructLit(_, inits) => inits.iter().for_each(|(_, x)| each_local(x, f)),
        ExprKind::Match(scrut, arms) => {
            each_local(scrut, f);
            for arm in arms {
                binds(&arm.pat, &mut |id| f(id, true));
                each_local(&arm.body, f);
            }
        }
    }
}

fn each_local_place(place: &ir::Place, f: &mut impl FnMut(LocalId, bool)) {
    f(place.root, false);
    for step in &place.steps {
        if let PlaceStep::Index(i, _) = step {
            each_local(i, f);
        }
    }
}

fn each_local_block(b: &ir::Block, f: &mut impl FnMut(LocalId, bool)) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let(id, init) => {
                each_local(init, f);
                f(*id, true);
            }
            StmtKind::Assign(place, _, value) => {
                each_local_place(place, f);
                each_local(value, f);
            }
            StmtKind::Return(value) => value.iter().for_each(|e| each_local(e, f)),
            StmtKind::While(cond, body) => {
                each_local(cond, f);
                each_local_block(body, f);
            }
            StmtKind::Loop(body) => each_local_block(body, f),
            StmtKind::ForList(x, xs, body) => {
                each_local(xs, f);
                f(*x, true);
                each_local_block(body, f);
            }
            StmtKind::ForRange(i, a, b, body) => {
                each_local(a, f);
                each_local(b, f);
                f(*i, true);
                each_local_block(body, f);
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Expr(e) => each_local(e, f),
        }
    }
    if let Some(t) = &b.tail {
        each_local(t, f);
    }
}
