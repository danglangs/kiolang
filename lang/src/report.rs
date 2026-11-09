use crate::ast::*;
use crate::check::{key, module_of, Program};
use std::collections::{BTreeMap, BTreeSet};

/// Methods reachable on each capability parameter, keyed by (function, parameter).
type Uses = BTreeMap<(String, String), BTreeSet<String>>;

/// One line per function: its capability parameters and the methods they may call, including
/// through helpers. Reads the checked AST only.
pub fn report(p: &Program) -> String {
    let is_cap = |d: &Param| matches!(&d.ty.kind, TypeKind::Named(n, a) if a.is_empty() && p.is_cap(n));
    let mut uses = Uses::new();
    for (name, f) in &p.funcs {
        for d in f.params.iter().filter(|d| is_cap(d)) {
            uses.insert((name.clone(), d.name.clone()), BTreeSet::new());
        }
    }
    // Sets only grow and are bounded by the method names, so this stops.
    let mut grown = true;
    while grown {
        grown = false;
        for (name, f) in &p.funcs {
            let mut found = Vec::new();
            // Rule 1: a capability is only a parameter name, as a receiver or a whole argument.
            // A name that is not a capability parameter finds no entry in `uses` and is skipped.
            walk_block(&f.body, &mut |e| match &e.kind {
                ExprKind::Method(recv, m, _) => {
                    // Only a method the capability has: a local shadowing the parameter
                    // (`let d = 5; d.to_str()`) names no capability method.
                    let Some(d) = recv_name(recv) else { return };
                    let Some(ty) = cap_type(f, d) else { return };
                    if p.cap_methods.contains_key(&(ty.to_string(), m.clone())) {
                        found.push((d.to_string(), m.clone()));
                    }
                }
                ExprKind::Call(callee, args) => {
                    // `Some(x)` and the like are calls on names that are not functions. A
                    // generic function's capability parameters are concrete, so every instance
                    // is charged the same.
                    let g = match &callee.kind {
                        ExprKind::Name(g) | ExprKind::Turbofish(g, _) => key(module_of(name), g).into_owned(),
                        ExprKind::Module(m, inner) => match &inner.kind {
                            ExprKind::Name(g) | ExprKind::Turbofish(g, _) => format!("{m}::{g}"),
                            _ => return,
                        },
                        _ => return,
                    };
                    let Some(g_fn) = p.funcs.get(&g) else { return };
                    for (arg, param) in args.iter().zip(&g_fn.params) {
                        if let (ExprKind::Name(d), Some(ms)) = (&arg.kind, uses.get(&(g.clone(), param.name.clone()))) {
                            found.extend(ms.iter().map(|m| (d.clone(), m.clone())));
                        }
                    }
                }
                _ => {}
            });
            for (d, m) in found {
                if let Some(set) = uses.get_mut(&(name.clone(), d)) {
                    grown |= set.insert(m);
                }
            }
        }
    }

    let mut names: Vec<&String> = p.funcs.keys().collect();
    names.sort();
    let width = names.iter().map(|n| n.len()).max().unwrap_or(0) + 1;
    let mut lines = Vec::new();
    for name in names {
        let mut parts = Vec::new();
        for d in &p.funcs[name].params {
            if let Some(ms) = uses.get(&(name.clone(), d.name.clone())).filter(|ms| !ms.is_empty()) {
                let ms: Vec<&str> = ms.iter().map(String::as_str).collect();
                parts.push(format!("{} {}", d.name, ms.join(" ")));
            }
        }
        let used = if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(", ")
        };
        lines.push(format!("{name:<width$}: {used}"));
    }
    lines.join("\n")
}

/// Calls `visit` on every expression in the block, outer before inner.
fn walk_block(b: &Block, visit: &mut dyn FnMut(&Expr)) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { init: e, .. } | StmtKind::Return(Some(e)) | StmtKind::Expr(e) => walk(e, visit),
            StmtKind::Assign { target, value, .. } => {
                walk(target, visit);
                walk(value, visit);
            }
            StmtKind::LetElse { init, els, .. } => {
                walk(init, visit);
                walk_block(els, visit);
            }
            StmtKind::Loop { body } => walk_block(body, visit),
            StmtKind::While { cond, body } => {
                walk(cond, visit);
                walk_block(body, visit);
            }
            StmtKind::For { iter, body, .. } => {
                match iter {
                    Iter::List(e) => walk(e, visit),
                    Iter::Range(a, b) => {
                        walk(a, visit);
                        walk(b, visit);
                    }
                }
                walk_block(body, visit);
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        }
    }
    if let Some(t) = &b.tail {
        walk(t, visit);
    }
}

fn walk(e: &Expr, visit: &mut dyn FnMut(&Expr)) {
    visit(e);
    match &e.kind {
        // A closure's body is charged to the function it is written in. It can reach no
        // capability (the checker refuses a capture of one), so this only keeps the report an
        // upper bound should that rule ever fail.
        // `web::Row { b: d.read("a") }` is a module path around a struct literal: walk into it.
        ExprKind::Unary(_, a)
        | ExprKind::Try(a)
        | ExprKind::Field(a, _)
        | ExprKind::Closure(_, a)
        | ExprKind::Module(_, a) => walk(a, visit),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
            walk(a, visit);
            walk(b, visit);
        }
        ExprKind::If(c, then, els) => {
            walk(c, visit);
            walk_block(then, visit);
            if let Some(els) = els {
                walk(els, visit);
            }
        }
        ExprKind::IfLet(_, s, then, els) => {
            walk(s, visit);
            walk_block(then, visit);
            if let Some(els) = els {
                walk(els, visit);
            }
        }
        ExprKind::Block(b) => walk_block(b, visit),
        ExprKind::Call(f, args) => {
            walk(f, visit);
            args.iter().for_each(|a| walk(a, visit));
        }
        ExprKind::Method(r, _, args) => {
            walk(r, visit);
            args.iter().for_each(|a| walk(a, visit));
        }
        ExprKind::List(items) => items.iter().for_each(|a| walk(a, visit)),
        ExprKind::StructLit(_, fields) => fields.iter().for_each(|(_, v)| walk(v, visit)),
        ExprKind::Match(s, arms) => {
            walk(s, visit);
            arms.iter().for_each(|a| walk(&a.body, visit));
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Name(_)
        | ExprKind::Turbofish(..)
        | ExprKind::Path(..) => {}
    }
}

fn recv_name(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Name(d) => Some(d),
        _ => None,
    }
}

/// The capability type of `f`'s parameter `d`, if it is one.
fn cap_type<'a>(f: &'a Fn, d: &str) -> Option<&'a str> {
    let param = f.params.iter().find(|x| x.name == d)?;
    match &param.ty.kind {
        TypeKind::Named(n, a) if a.is_empty() => Some(n.as_str()),
        _ => None,
    }
}
