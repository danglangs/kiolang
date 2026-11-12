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

/// The reads (`ExprKind::Local` nodes, by address) of `f` that may move; `owns` says which
/// types hold a reference.
pub(crate) fn moves(f: &ir::Func, owns: &dyn Fn(&ir::Ty) -> bool) -> HashSet<*const Expr> {
    let mut a = Analysis {
        f,
        owns,
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
    owns: &'f dyn Fn(&ir::Ty) -> bool,
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
                if !live.contains(*id) && (self.owns)(&e.ty) {
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
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Assert(a, b) | ExprKind::AssertEq(a, b) => {
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
        // Every alternative binds the same locals.
        Pat::Or(alts) => binds(&alts[0], f),
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
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Assert(a, b) | ExprKind::AssertEq(a, b) => {
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

/// Borrowed locals (C8b4, C8c8): for each function, which of its locals are borrowed, by
/// `LocalId`. A borrowed local holds a reference that is someone else's, so the function
/// neither drops nor moves it: a borrowed parameter is passed without an `rt_inc`, the caller
/// keeping its reference across the call; a borrowed `for` variable is the list's slot, kept by
/// the loop's reference to the list (a change to the list inside the body copies it, since the
/// loop holds it); a borrowed `match` binding is the scrutinee's part, kept by the scrutinee
/// local.
///
/// A pointer parameter is borrowed when its function reads it only where codegen uses a local
/// in place, without a reference of its own: an operand of a runtime operation, `==` or `+`, an
/// index or field base, a `match` scrutinee, the operand of `?`, a called `fn` value, the
/// receiver and function of a built-in taking one, a map key, the iterable of `for`, or an
/// argument for another borrowed parameter. A read anywhere else (a `let`, a return or a
/// block's value, an element, an owned argument, a capture) takes a reference, which an owned
/// parameter can hand over (a move) and a borrowed one would have to add; and a parameter that
/// is assigned or changed in place (a place write, a mutation, `+=`) stays owned, since that
/// drops or replaces the reference. Arguments for borrowed parameters are borrowed reads, so
/// this is a fixpoint: every pointer parameter starts borrowed and loses it when its function
/// reads it elsewhere, until nothing changes.
///
/// A `for` variable is borrowed under the same reading rule. A `match` binding is, when also
/// its scrutinee is a local that cannot change while the arm runs: a borrowed local itself, or
/// one no arm's body mentions (only the function's own code can assign, move or change a local,
/// and only the arm runs). Neither takes part in the fixpoint: a call sees only parameters.
pub(crate) fn borrowed(m: &ir::Module, owns: &dyn Fn(&ir::Ty) -> bool) -> Vec<Vec<bool>> {
    let mut b: Vec<Vec<bool>> = m
        .funcs
        .iter()
        .map(|f| {
            let mut v: Vec<bool> = f.locals[..f.params as usize].iter().map(|l| owns(&l.ty)).collect();
            v.resize(f.locals.len(), false);
            v
        })
        .collect();
    loop {
        let mut changed = false;
        for (i, f) in m.funcs.iter().enumerate() {
            let mut u = Uses {
                borrowed: &b,
                owned: Live::new(f.locals.len()),
                for_vars: Vec::new(),
                bindings: Vec::new(),
            };
            u.block(&f.body);
            let Uses {
                owned,
                for_vars,
                bindings,
                ..
            } = u;
            let lent = &mut b[i];
            for (p, l) in lent.iter_mut().enumerate().take(f.params as usize) {
                if *l && owned.contains(LocalId(p as u32)) {
                    *l = false;
                    changed = true;
                }
            }
            let in_place = |id: LocalId| owns(&f.locals[id.0 as usize].ty) && !owned.contains(id);
            for x in for_vars {
                lent[x.0 as usize] = in_place(x);
            }
            // In program order: a binding's scrutinee is declared, so decided, before it.
            for (id, s, mentioned) in bindings {
                lent[id.0 as usize] = in_place(id) && (lent[s.0 as usize] || !mentioned);
            }
        }
        if !changed {
            return b;
        }
    }
}

/// The locals of a function that are read where a reference is taken, or assigned or changed
/// in place.
struct Uses<'b> {
    borrowed: &'b [Vec<bool>],
    owned: Live,
    /// The `for` variables declared, in program order.
    for_vars: Vec<LocalId>,
    /// The `match` bindings whose scrutinee is a read of a local, in program order: the
    /// binding, the scrutinee's local, and whether an arm's body mentions that local.
    bindings: Vec<(LocalId, LocalId, bool)>,
}

impl Uses<'_> {
    /// `e` where its value is taken: a read of a local there takes a reference.
    fn owned(&mut self, e: &Expr) {
        match e.kind {
            ExprKind::Local(id) => self.owned.insert(id),
            _ => self.inner(e),
        }
    }

    /// `e` where a read of a local is used in place.
    fn borrowed(&mut self, e: &Expr) {
        if !matches!(e.kind, ExprKind::Local(_)) {
            self.inner(e);
        }
    }

    fn inner(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Local(id) => self.owned.insert(*id),
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Unit
            | ExprKind::FnRef(_)
            | ExprKind::MapNew => {}
            ExprKind::Unary(_, x) => self.owned(x),
            ExprKind::Field(x, _) | ExprKind::Try(x) => self.borrowed(x),
            // On pointers only `==`, `!=` and `+`, which borrow; the rest are on scalars.
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Assert(a, b) | ExprKind::AssertEq(a, b) => {
                self.borrowed(a);
                self.borrowed(b);
            }
            ExprKind::If(cond, then, els) => {
                self.owned(cond);
                self.block(then);
                if let Some(els) = els {
                    self.owned(els);
                }
            }
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(f, args) => {
                for (x, &lent) in args.iter().zip(&self.borrowed[f.0 as usize]) {
                    match lent {
                        true => self.borrowed(x),
                        false => self.owned(x),
                    }
                }
            }
            ExprKind::List(args) | ExprKind::Variant(_, _, args) => args.iter().for_each(|x| self.owned(x)),
            ExprKind::StructLit(_, inits) => inits.iter().for_each(|(_, x)| self.owned(x)),
            ExprKind::CallValue(callee, args) => {
                self.borrowed(callee);
                args.iter().for_each(|x| self.owned(x));
            }
            ExprKind::Closure(_, captured) => captured.iter().for_each(|&id| self.owned.insert(id)),
            ExprKind::CapCall(_, _, args) => args.iter().for_each(|x| self.borrowed(x)),
            ExprKind::Builtin(_, recv, args) => {
                self.borrowed(recv);
                args.iter().for_each(|x| self.borrowed(x));
            }
            ExprKind::Mutate(place, op, args) => {
                self.place(place);
                let key = matches!(
                    op,
                    ir::Mut::MapInsert | ir::Mut::MapRemove | ir::Mut::ListSortBy | ir::Mut::ListSortByKey
                );
                for (i, x) in args.iter().enumerate() {
                    match key && i == 0 {
                        true => self.borrowed(x),
                        false => self.owned(x),
                    }
                }
            }
            ExprKind::Match(scrut, arms) => {
                self.borrowed(scrut);
                if let ExprKind::Local(s) = scrut.kind {
                    let mentioned = arms.iter().any(|arm| mentions(&arm.body, s));
                    for arm in arms {
                        binds(&arm.pat, &mut |id| self.bindings.push((id, s, mentioned)));
                    }
                }
                arms.iter().for_each(|arm| self.owned(&arm.body));
            }
        }
    }

    /// A place written or changed: its root is replaced or made unique.
    fn place(&mut self, place: &ir::Place) {
        self.owned.insert(place.root);
        for step in &place.steps {
            if let PlaceStep::Index(i, _) = step {
                self.owned(i);
            }
        }
    }

    fn block(&mut self, b: &ir::Block) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let(_, init) => self.owned(init),
                // `x += y` borrows `y`; a place's `+=` evaluates it as an operand.
                StmtKind::Assign(place, op, value) => {
                    self.place(place);
                    match *op == Some(ir::BinOp::Concat) && place.steps.is_empty() {
                        true => self.borrowed(value),
                        false => self.owned(value),
                    }
                }
                StmtKind::Return(value) => value.iter().for_each(|e| self.owned(e)),
                StmtKind::While(cond, body) => {
                    self.owned(cond);
                    self.block(body);
                }
                StmtKind::Loop(body) => self.block(body),
                StmtKind::ForList(x, xs, body) => {
                    self.borrowed(xs);
                    self.for_vars.push(*x);
                    self.block(body);
                }
                StmtKind::ForRange(_, a, b, body) => {
                    self.owned(a);
                    self.owned(b);
                    self.block(body);
                }
                StmtKind::Break | StmtKind::Continue => {}
                StmtKind::Expr(e) => self.owned(e),
            }
        }
        if let Some(t) = &b.tail {
            self.owned(t);
        }
    }
}
