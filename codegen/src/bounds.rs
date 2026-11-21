//! Byte reads that cannot be out of bounds (C25): `b[i]` on `Bytes` skips its bounds check
//! where `0 <= i < b.len()` is known to hold.
//!
//! `i < b.len()` holds in the body of `while i < b.len()` and of `if i < b.len()`, on the
//! right of `i < b.len() &&` (also as a conjunct of the condition), and in the body of
//! `for i in a..b.len()` that does not write `b`; in each, `i` and `b` are locals. It holds
//! until a statement that writes `i` or `b` (codegen drops it before generating that statement,
//! wherever in it the write is), and is not one at all if the condition or the `&&`'s right side
//! may write either, since only the function's own code can write a local, and a
//! `Bytes` object changes in place only through a write to its own local (a shared object is
//! copied first; a borrowed local's source does not change while it is borrowed, moves.rs).
//! `i >= 0` holds when every value written to `i` is not negative (`nonneg`).

use lang::ir::{self, BinOp, Block, Builtin, Expr, ExprKind, LocalId, Pat, PlaceStep, Stmt, StmtKind, Ty};

/// Per local of `f`, whether it never holds a negative `Int`: every value written to it is a
/// literal `>= 0`, a length, such a local, or `+` of two of these (an overflow traps), and
/// `+=` adds one. A parameter is one if `seed` says so (an inlined call's argument is).
/// The greatest such set: every local starts in it and leaves when a write does not qualify.
pub(crate) fn nonneg(f: &ir::Func, seed: &[bool]) -> Vec<bool> {
    let mut ok: Vec<bool> = f
        .locals
        .iter()
        .enumerate()
        .map(|(i, l)| l.ty == Ty::Int && (i >= f.params as usize || seed.get(i) == Some(&true)))
        .collect();
    let mut writes = Vec::new();
    block_writes(&f.body, &mut |id, value| writes.push((id, value)));
    let mut changed = true;
    while changed {
        changed = false;
        for &(id, value) in &writes {
            if ok[id.0 as usize] && !value.is_some_and(|e| nonneg_expr(e, &ok)) {
                ok[id.0 as usize] = false;
                changed = true;
            }
        }
    }
    ok
}

/// Whether `e` is never negative, given the locals `ok` holds for.
pub(crate) fn nonneg_expr(e: &Expr, ok: &[bool]) -> bool {
    match &e.kind {
        ExprKind::Int(n) => *n >= 0,
        ExprKind::Local(id) => ok[id.0 as usize],
        ExprKind::Binary(BinOp::Add, a, b) => e.ty == Ty::Int && nonneg_expr(a, ok) && nonneg_expr(b, ok),
        ExprKind::Builtin(Builtin::BytesLen | Builtin::StrLen | Builtin::ListLen, _, _) => true,
        _ => false,
    }
}

/// The pairs `(i, b)` for which `cond` being true means `0 <= i < b.len()` when `cond` has
/// run and then `rest` runs up to the read: `i < b.len()` or `b.len() > i` with `i` in `ok`,
/// as `cond` or an operand of its `&&`s, unless `cond` or `rest` may write `i` or `b`
/// anywhere (a later operand of the same `&&` chain could).
pub(crate) fn facts(cond: &Expr, rest: &[&Expr], ok: &[bool], out: &mut Vec<(LocalId, LocalId)>) {
    let mut found = Vec::new();
    conjuncts(cond, ok, &mut found);
    let kept = |&(i, b): &(LocalId, LocalId)| !expr_writes(cond, i, b) && !rest.iter().any(|e| expr_writes(e, i, b));
    out.extend(found.into_iter().filter(kept));
}

fn conjuncts(cond: &Expr, ok: &[bool], out: &mut Vec<(LocalId, LocalId)>) {
    match &cond.kind {
        ExprKind::Binary(BinOp::And, a, b) => {
            conjuncts(a, ok, out);
            conjuncts(b, ok, out);
        }
        ExprKind::Binary(BinOp::Lt, i, len) | ExprKind::Binary(BinOp::Gt, len, i) => {
            if let (ExprKind::Local(i), Some(b)) = (&i.kind, len_of(len)) {
                if ok[i.0 as usize] {
                    out.push((*i, b));
                }
            }
        }
        _ => {}
    }
}

/// `b` when `len` is `b.len()` on a `Bytes` local `b`.
pub(crate) fn len_of(len: &Expr) -> Option<LocalId> {
    match &len.kind {
        ExprKind::Builtin(Builtin::BytesLen, b, _) => match b.kind {
            ExprKind::Local(b) => Some(b),
            _ => None,
        },
        _ => None,
    }
}

/// Whether running `s` may write local `a` or `b`.
pub(crate) fn stmt_writes(s: &Stmt, a: LocalId, b: LocalId) -> bool {
    let mut hit = false;
    stmt_writes_each(s, &mut |id, _| hit |= id == a || id == b);
    hit
}

/// Whether evaluating `e` may write local `a` or `b`.
pub(crate) fn expr_writes(e: &Expr, a: LocalId, b: LocalId) -> bool {
    let mut hit = false;
    expr_writes_each(e, &mut |id, _| hit |= id == a || id == b);
    hit
}

/// Whether running `body` may write local `a` or `b`.
pub(crate) fn body_writes(body: &Block, a: LocalId, b: LocalId) -> bool {
    let mut hit = false;
    block_writes(body, &mut |id, _| hit |= id == a || id == b);
    hit
}

/// Calls `f` with every local a write in `b` sets, and the value written when it is `e` itself
/// (`let`, `=`) or adds `e` to the local (`+=`); `None` for any other write.
fn block_writes<'e>(b: &'e Block, f: &mut impl FnMut(LocalId, Option<&'e Expr>)) {
    b.stmts.iter().for_each(|s| stmt_writes_each(s, f));
    if let Some(t) = &b.tail {
        expr_writes_each(t, f);
    }
}

fn stmt_writes_each<'e>(s: &'e Stmt, f: &mut impl FnMut(LocalId, Option<&'e Expr>)) {
    match &s.kind {
        StmtKind::Let(id, init) => {
            expr_writes_each(init, f);
            f(*id, Some(init));
        }
        StmtKind::Assign(place, op, value) => {
            place_writes(place, f);
            expr_writes_each(value, f);
            let known = place.steps.is_empty() && matches!(op, None | Some(BinOp::Add));
            f(place.root, known.then_some(value));
        }
        StmtKind::Return(value) => value.iter().for_each(|e| expr_writes_each(e, f)),
        StmtKind::While(cond, body) => {
            expr_writes_each(cond, f);
            block_writes(body, f);
        }
        StmtKind::Loop(body) => block_writes(body, f),
        StmtKind::ForList(x, xs, body) => {
            expr_writes_each(xs, f);
            f(*x, None);
            block_writes(body, f);
        }
        StmtKind::ForRange(i, a, b, body) => {
            expr_writes_each(a, f);
            expr_writes_each(b, f);
            // Counts up from `a`, below the end: no overflow.
            f(*i, Some(a));
            block_writes(body, f);
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Expr(e) => expr_writes_each(e, f),
    }
}

fn place_writes<'e>(place: &'e ir::Place, f: &mut impl FnMut(LocalId, Option<&'e Expr>)) {
    for step in &place.steps {
        if let PlaceStep::Index(i, _) = step {
            expr_writes_each(i, f);
        }
    }
}

fn expr_writes_each<'e>(e: &'e Expr, f: &mut impl FnMut(LocalId, Option<&'e Expr>)) {
    match &e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::FnRef(_)
        | ExprKind::MapNew
        // A closure copies its captures: its body writes its own locals.
        | ExprKind::Closure(..) => {}
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) | ExprKind::Assume(x) => expr_writes_each(x, f),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Assert(a, b) | ExprKind::AssertEq(a, b) => {
            expr_writes_each(a, f);
            expr_writes_each(b, f);
        }
        ExprKind::If(cond, then, els) => {
            expr_writes_each(cond, f);
            block_writes(then, f);
            if let Some(els) = els {
                expr_writes_each(els, f);
            }
        }
        ExprKind::Block(b) => block_writes(b, f),
        ExprKind::Call(_, args) | ExprKind::List(args) | ExprKind::Variant(_, _, args) | ExprKind::CapCall(_, _, args) => {
            args.iter().for_each(|x| expr_writes_each(x, f))
        }
        ExprKind::CallValue(callee, args) | ExprKind::Builtin(_, callee, args) => {
            expr_writes_each(callee, f);
            args.iter().for_each(|x| expr_writes_each(x, f));
        }
        ExprKind::Mutate(place, _, args) => {
            place_writes(place, f);
            args.iter().for_each(|x| expr_writes_each(x, f));
            f(place.root, None);
        }
        ExprKind::StructLit(_, inits) => inits.iter().for_each(|(_, x)| expr_writes_each(x, f)),
        ExprKind::Match(scrut, arms) => {
            expr_writes_each(scrut, f);
            for arm in arms {
                pat_binds(&arm.pat, f);
                expr_writes_each(&arm.body, f);
            }
        }
    }
}

fn pat_binds<'e>(p: &Pat, f: &mut impl FnMut(LocalId, Option<&'e Expr>)) {
    match p {
        Pat::Bind(id) => f(*id, None),
        Pat::Variant(_, ps) => ps.iter().for_each(|p| pat_binds(p, f)),
        Pat::Or(alts) => alts.iter().for_each(|p| pat_binds(p, f)),
        Pat::Wild | Pat::Int(_) | Pat::Str(_) | Pat::Bool(_) => {}
    }
}
