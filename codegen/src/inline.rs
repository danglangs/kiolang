//! Which functions codegen inlines at their direct call sites (C8c3).
//!
//! A function is inlined when it has no loop, creates no closure, is not recursive (reaches no
//! call back to itself) and is small: its IR nodes (expressions and statements), with those of
//! the functions inlined into it, number under `MAX_NODES`. So an inlined body holds no call
//! that is inlined again without bound, and every expansion stays under the threshold. One
//! exception to "not recursive": a small function whose only recursion is a direct call to
//! itself is inlined into its own body once (`fib`), the copy's calls staying calls.
//! `FuncGen::call` binds the arguments to fresh locals and generates the callee's IR in place,
//! so its error sites keep the callee's spans.

use lang::ir::{self, Expr, ExprKind, FuncId, Module, PlaceStep, StmtKind};

pub(crate) const MAX_NODES: usize = 40;

/// The nodes inlined into one function at most. Cranelift's optimizer is superlinear in a
/// function's length (17,000 checked additions in one function: 0.8 s inlined as calls, 100 s
/// inlined), so a function with very many calls inlines the first ones only.
pub(crate) const BUDGET: usize = 1000;

/// How direct calls to a function are generated, and its size with its inlined callees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inline {
    No,
    /// Inlined at every direct call.
    Yes(usize),
    /// A small function whose only recursion is calling itself: inlined into its own body once
    /// (the calls in the inlined copy are calls).
    IntoItself(usize),
}

/// Per function, how direct calls to it are generated.
pub(crate) fn inlined(m: &Module) -> Vec<Inline> {
    let facts: Vec<Facts> = m.funcs.iter().map(facts).collect();
    let n = m.funcs.len();
    // Sizes with the inlined callees counted, memoized.
    let mut size: Vec<Option<usize>> = vec![None; n];
    let plain = |f: usize| !facts[f].loops && !facts[f].closure;
    let leaf: Vec<bool> = (0..n).map(|f| plain(f) && !reaches(&facts, f, f)).collect();
    (0..n)
        .map(|f| {
            let only_itself = || {
                let back = |g: &FuncId| g.0 as usize != f && reaches(&facts, g.0 as usize, f);
                !facts[f].calls.iter().any(back)
            };
            let n = sized(&facts, &leaf, &mut size, f);
            if leaf[f] && f != m.main.0 as usize && n < MAX_NODES {
                Inline::Yes(n)
            } else if !leaf[f] && plain(f) && only_itself() && n < MAX_NODES {
                Inline::IntoItself(n)
            } else {
                Inline::No
            }
        })
        .collect()
}

/// `f`'s size with every callee it inlines counted in: a leaf that is itself small enough. A
/// leaf calls neither `f` nor itself back, so it ends.
fn sized(facts: &[Facts], leaf: &[bool], size: &mut [Option<usize>], f: usize) -> usize {
    if let Some(s) = size[f] {
        return s;
    }
    let mut s = facts[f].nodes;
    for &g in &facts[f].calls {
        let g = g.0 as usize;
        if leaf[g] {
            let gs = sized(facts, leaf, size, g);
            if gs < MAX_NODES {
                s += gs;
            }
        }
    }
    size[f] = Some(s);
    s
}

/// Whether a chain of direct calls leads from `from` to `to`.
fn reaches(facts: &[Facts], from: usize, to: usize) -> bool {
    let mut seen = vec![false; facts.len()];
    let mut work = vec![from];
    while let Some(f) = work.pop() {
        for g in &facts[f].calls {
            let g = g.0 as usize;
            if g == to {
                return true;
            }
            if !seen[g] {
                seen[g] = true;
                work.push(g);
            }
        }
    }
    false
}

#[derive(Default)]
struct Facts {
    nodes: usize,
    loops: bool,
    closure: bool,
    /// Direct calls, one per call site.
    calls: Vec<FuncId>,
}

fn facts(f: &ir::Func) -> Facts {
    let mut a = Facts::default();
    a.block(&f.body);
    a
}

impl Facts {
    fn block(&mut self, b: &ir::Block) {
        for s in &b.stmts {
            self.nodes += 1;
            match &s.kind {
                StmtKind::Let(_, e) | StmtKind::Expr(e) => self.expr(e),
                StmtKind::Assign(place, _, e) => {
                    self.place(place);
                    self.expr(e);
                }
                StmtKind::Return(e) => e.iter().for_each(|e| self.expr(e)),
                StmtKind::While(..) | StmtKind::Loop(_) | StmtKind::ForList(..) | StmtKind::ForRange(..) => {
                    self.loops = true
                }
                StmtKind::Break | StmtKind::Continue => {}
            }
        }
        if let Some(t) = &b.tail {
            self.expr(t);
        }
    }

    fn place(&mut self, place: &ir::Place) {
        for step in &place.steps {
            if let PlaceStep::Index(i, _) = step {
                self.expr(i);
            }
        }
    }

    fn exprs(&mut self, xs: &[Expr]) {
        xs.iter().for_each(|x| self.expr(x));
    }

    fn expr(&mut self, e: &Expr) {
        self.nodes += 1;
        match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Unit
            | ExprKind::Local(_)
            | ExprKind::FnRef(_)
            | ExprKind::MapNew => {}
            ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) | ExprKind::Assume(x) => self.expr(x),
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Assert(a, b) | ExprKind::AssertEq(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::If(cond, then, els) => {
                self.expr(cond);
                self.block(then);
                if let Some(els) = els {
                    self.expr(els);
                }
            }
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(f, args) => {
                self.calls.push(*f);
                self.exprs(args);
            }
            ExprKind::List(args) | ExprKind::Variant(_, _, args) | ExprKind::CapCall(_, _, args) => self.exprs(args),
            ExprKind::CallValue(callee, args) | ExprKind::Builtin(_, callee, args) => {
                self.expr(callee);
                self.exprs(args);
            }
            ExprKind::Closure(..) => self.closure = true,
            ExprKind::Mutate(place, _, args) => {
                self.place(place);
                self.exprs(args);
            }
            ExprKind::StructLit(_, inits) => inits.iter().for_each(|(_, x)| self.expr(x)),
            ExprKind::Match(scrut, arms) => {
                self.expr(scrut);
                arms.iter().for_each(|arm| self.expr(&arm.body));
            }
        }
    }
}
