use crate::ast::*;
use crate::check::{key, module_of, Program, Ty};
use std::collections::{BTreeMap, BTreeSet};

/// Methods reachable on each capability parameter, keyed by (function, parameter).
type Uses = BTreeMap<(String, String), BTreeSet<String>>;

/// What a program may do: per function, each capability parameter that calls something with the
/// methods it may call (in parameter order); and `main`'s parameters as `(name, type)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authority {
    pub funcs: BTreeMap<String, Vec<(String, BTreeSet<String>)>>,
    pub main: Vec<(String, String)>,
}

/// One line per function: its capability parameters and the methods they may call, including
/// through helpers. Reads the checked AST only.
pub fn report(p: &Program) -> String {
    let a = authority(p);
    let width = a.funcs.keys().map(|n| n.len()).max().unwrap_or(0) + 1;
    let mut lines = Vec::new();
    for (name, params) in &a.funcs {
        let used = if params.is_empty() {
            "none".to_string()
        } else {
            uses_str(params)
        };
        lines.push(format!("{name:<width$}: {used}"));
    }
    lines.join("\n")
}

/// `hook send, log write`.
fn uses_str(params: &[(String, BTreeSet<String>)]) -> String {
    let parts: Vec<String> = params
        .iter()
        .map(|(d, ms)| format!("{d} {}", ms.iter().map(String::as_str).collect::<Vec<_>>().join(" ")))
        .collect();
    parts.join(", ")
}

/// What `new` changes in `old`'s authority; every list is empty when nothing changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorityDiff {
    /// One line per grant (`main` parameter) whose authority changed, in `new`'s order, then the
    /// removed ones: `+ stripe: Hmac (verify)` for a new grant, with the methods `main` reaches on
    /// it through any function; `~ db: Db (+query -get)` for methods gained and lost on a grant
    /// of the same name and type; `- x: Dir (read)`. A grant whose type changed is one removed
    /// and one added.
    pub grants: Vec<String>,
    /// Per function by name, the capability methods added (`+ handle: stripe verify`) and dropped
    /// (`- ..`). A function on one side only counts in full; a parameter is matched by name.
    pub funcs: Vec<String>,
    /// Some line adds authority.
    pub added: bool,
}

/// The authority `new` has that `old` lacks, and the reverse.
pub fn report_diff(old: &Program, new: &Program) -> AuthorityDiff {
    let (old, new) = (authority(old), authority(new));
    let mut diff = AuthorityDiff::default();
    for g @ (d, ty) in &new.main {
        let ms = reached(&new, d);
        if !old.main.contains(g) {
            diff.grants.push(format!("+ {d}: {ty}{}", in_parens(ms)));
            diff.added = true;
            continue;
        }
        let was = reached(&old, d);
        let gained: Vec<String> = ms.difference(&was).map(|m| format!("+{m}")).collect();
        let lost: Vec<String> = was.difference(&ms).map(|m| format!("-{m}")).collect();
        if !gained.is_empty() || !lost.is_empty() {
            diff.added |= !gained.is_empty();
            diff.grants
                .push(format!("~ {d}: {ty}{}", in_parens([gained, lost].concat())));
        }
    }
    for (d, ty) in old.main.iter().filter(|g| !new.main.contains(g)) {
        diff.grants.push(format!("- {d}: {ty}{}", in_parens(reached(&old, d))));
    }
    let none = Vec::new();
    let names: BTreeSet<&String> = old.funcs.keys().chain(new.funcs.keys()).collect();
    for name in names {
        let (o, n) = (
            old.funcs.get(name).unwrap_or(&none),
            new.funcs.get(name).unwrap_or(&none),
        );
        for (sign, a, b) in [('+', n, o), ('-', o, n)] {
            let mut only = Vec::new();
            for (d, ms) in a {
                let theirs = b.iter().find(|(e, _)| e == d).map(|(_, t)| t);
                let ms: BTreeSet<String> = ms
                    .iter()
                    .filter(|m| !theirs.is_some_and(|t| t.contains(*m)))
                    .cloned()
                    .collect();
                if !ms.is_empty() {
                    only.push((d.clone(), ms));
                }
            }
            if !only.is_empty() {
                diff.added |= sign == '+';
                diff.funcs.push(format!("{sign} {name}: {}", uses_str(&only)));
            }
        }
    }
    diff
}

/// The methods `main` reaches on its parameter `d`, through any function.
fn reached(a: &Authority, d: &str) -> BTreeSet<String> {
    let main = a.funcs.get("main").map_or(&[][..], Vec::as_slice);
    main.iter()
        .find(|(e, _)| e == d)
        .map(|(_, ms)| ms.clone())
        .unwrap_or_default()
}

/// ` (a b)`, or nothing for no items.
fn in_parens(items: impl IntoIterator<Item = String>) -> String {
    let items: Vec<String> = items.into_iter().collect();
    if items.is_empty() {
        String::new()
    } else {
        format!(" ({})", items.join(" "))
    }
}

/// The structure `report` prints.
pub fn authority(p: &Program) -> Authority {
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
            walk_block(&f.body, &mut |e| match &e.kind {
                ExprKind::Method(recv, m, _) => {
                    if let Some(d) = cap_param(p, recv) {
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
                        if let (Some(d), Some(ms)) = (cap_param(p, arg), uses.get(&(g.clone(), param.name.clone()))) {
                            found.extend(ms.iter().map(|m| (d.to_string(), m.clone())));
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

    let mut a = Authority::default();
    for (name, f) in &p.funcs {
        let mut params = Vec::new();
        for d in &f.params {
            if let Some(ms) = uses.remove(&(name.clone(), d.name.clone())).filter(|ms| !ms.is_empty()) {
                params.push((d.name.clone(), ms));
            }
        }
        a.funcs.insert(name.clone(), params);
    }
    if let Some(f) = p.funcs.get("main") {
        a.main = f.params.iter().map(|d| (d.name.clone(), type_str(&d.ty))).collect();
    }
    a
}

/// `Http`, `List<Int>`, `web::Point`, as the source writes it.
fn type_str(t: &Type) -> String {
    let list = |ts: &[Type]| ts.iter().map(type_str).collect::<Vec<_>>().join(", ");
    match &t.kind {
        TypeKind::Unit => "()".into(),
        TypeKind::Named(n, a) if a.is_empty() => n.clone(),
        TypeKind::Named(n, a) => format!("{n}<{}>", list(a)),
        TypeKind::Module(m, t) => format!("{m}::{}", type_str(t)),
        TypeKind::Fn(a, r) => format!("fn({}) -> {}", list(a), type_str(r)),
    }
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
        ExprKind::StructLit(_, fields, base) => {
            fields.iter().for_each(|(_, v)| walk(v, visit));
            base.iter().for_each(|b| walk(b, visit));
        }
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

/// The capability parameter `e` names, as the checker resolved it. The checker records a
/// capability type only for a name bound to a parameter of the function it is in (rule 1), so
/// a local, closure parameter, pattern binding or loop variable that shadows the parameter's
/// name is not charged to it.
fn cap_param<'a>(p: &Program, e: &'a Expr) -> Option<&'a str> {
    match (&e.kind, p.types.get(&e.span)?) {
        (ExprKind::Name(d), Ty::Cap(_)) => Some(d),
        _ => None,
    }
}
