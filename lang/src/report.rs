use crate::ast::*;
use crate::check::{key, module_of, Program, Ty};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Methods reachable on each capability leaf, keyed by (function, leaf path). A leaf is a
/// capability parameter (`d`) or a field of a capability struct parameter (`env.db`).
type Uses = BTreeMap<(String, String), BTreeSet<String>>;

/// The call graph read for authority, one set of maps every derivation shares (L32): each
/// capability method a function calls on one of its own leaves, and each call edge from a
/// function to another with its binding map, each leaf of the callee that receives a leaf of the
/// caller. A leaf argument binds a capability parameter; a capability struct passed whole binds
/// each field to the same field; a literal binds each field to the leaf written for it.
struct Edges {
    direct: Vec<(String, String, String)>,
    calls: Vec<(String, String, Map)>,
}

/// A call edge's binding map: each `(callee leaf, caller leaf)`.
type Map = Vec<(String, String)>;

fn edges(p: &Program) -> Edges {
    let mut e = Edges {
        direct: Vec::new(),
        calls: Vec::new(),
    };
    for (name, f) in &p.funcs {
        walk_block(&f.body, &mut |n| {
            let Node::Expr(x) = n else { return };
            match &x.kind {
                ExprKind::Method(recv, m, args) => {
                    if let Some(d) = leaf_of(p, recv) {
                        let http = matches!(p.types.get(&recv.span), Some(Ty::Cap(t)) if t == "Http");
                        match (http, m.as_str(), args.first()) {
                            (true, "send", Some(v)) => e.direct.push((name.clone(), d, format!("send {}", verb(p, v)))),
                            (true, "send_all", Some(l)) => {
                                for v in send_all_verbs(p, f, l) {
                                    e.direct.push((name.clone(), d.clone(), format!("send_all {v}")));
                                }
                            }
                            _ => e.direct.push((name.clone(), d, m.clone())),
                        }
                    }
                }
                ExprKind::Call(callee, args) => {
                    // `Some(x)` and the like are calls on names that are not functions. A
                    // generic function's capability parameters are concrete, so every instance
                    // is charged the same.
                    let Some(g) = callee_key(name, callee) else { return };
                    let (Some(g_fn), Some(sig)) = (p.funcs.get(&g), p.sigs.get(&g)) else {
                        return;
                    };
                    let mut map = Vec::new();
                    for ((arg, param), ty) in args.iter().zip(&g_fn.params).zip(&sig.params) {
                        match ty {
                            Ty::Cap(_) => map.extend(leaf_of(p, arg).map(|d| (param.name.clone(), d))),
                            Ty::Group(k) => map.extend(group_arg(p, k, &param.name, arg)),
                            _ => {}
                        }
                    }
                    if !map.is_empty() {
                        e.calls.push((name.clone(), g, map));
                    }
                }
                _ => {}
            }
        });
    }
    e
}

/// The binding map of a capability struct `k` argument `arg` to parameter `param` of the callee:
/// a parameter passed whole binds field to field, a literal each field to its written leaf.
fn group_arg(p: &Program, k: &str, param: &str, arg: &Expr) -> Map {
    let inits = match (&arg.kind, p.types.get(&arg.span)) {
        (ExprKind::Name(g), Some(Ty::Group(_))) => {
            return (p.fields[k].iter())
                .map(|(f, _)| (format!("{param}.{f}"), format!("{g}.{f}")))
                .collect()
        }
        (ExprKind::StructLit(_, inits, _), Some(Ty::Group(_))) => inits,
        (ExprKind::Module(_, inner), Some(Ty::Group(_))) => match &inner.kind {
            ExprKind::StructLit(_, inits, _) => inits,
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    (inits.iter())
        .filter_map(|(f, x)| leaf_of(p, x).map(|d| (format!("{param}.{f}"), d)))
        .collect()
}

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
    print(&authority(p))
}

/// `entry_authority` printed as `report` prints: one line, `main : ..`.
pub fn entry_report(p: &Program, entry: &str) -> String {
    print(&entry_authority(p, entry))
}

fn print(a: &Authority) -> String {
    let width = a.funcs.keys().map(|n| n.len()).max().unwrap_or(0) + 1;
    lines(a.funcs.iter(), width).join("\n")
}

type Line<'a> = (&'a String, &'a Vec<(String, BTreeSet<String>)>);

/// `name : uses`, the name padded to `width`.
fn lines<'a>(funcs: impl Iterator<Item = Line<'a>>, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for (name, params) in funcs {
        let used = if params.is_empty() {
            "none".to_string()
        } else {
            uses_str(params)
        };
        lines.push(format!("{name:<width$}: {used}"));
    }
    lines
}

/// The report `kio check` prints (P8), as parts. `report` prints every function and is what the
/// host checks a run against; this view is never checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    /// `main`'s line, then each function `main` reaches that uses a capability.
    pub main: Vec<String>,
    /// Each function only a test reaches that uses a capability (test functions are left out).
    pub tests: Vec<String>,
    /// Functions reached, from `main` or a test, that use no capability.
    pub no_capability: usize,
    /// Functions nothing reaches, and how many of them use a capability.
    pub unreached: usize,
    pub unreached_with_capability: usize,
}

impl Shown {
    /// The lines; then, after a blank line, `reached only from tests:` and its lines; then, after
    /// a blank line, a count of the rest.
    pub fn text(&self) -> String {
        let mut out = self.main.clone();
        if !self.tests.is_empty() {
            out.extend(["".into(), "reached only from tests:".into()]);
            out.extend(self.tests.iter().cloned());
        }
        let mut rest = Vec::new();
        let (quiet, unreached, caps) = (self.no_capability, self.unreached, self.unreached_with_capability);
        if quiet > 0 {
            rest.push(format!(
                "{quiet} more use{} no capability",
                if quiet == 1 { "s" } else { "" }
            ));
        }
        if unreached > 0 {
            let with = match caps {
                0 => String::new(),
                n => format!(" ({n} use{} a capability)", if n == 1 { "s" } else { "" }),
            };
            rest.push(format!(
                "{unreached} {} unreached{with}",
                if unreached == 1 { "is" } else { "are" }
            ));
        }
        if !rest.is_empty() {
            out.extend(["".into(), format!("{}; --all lists every function", rest.join(", "))]);
        }
        out.join("\n")
    }
}

/// `shown(p).text()`: what `kio check` prints.
pub fn shown_report(p: &Program) -> String {
    shown(p).text()
}

/// `main`, each function it reaches that uses a capability, each only a test reaches that uses
/// one, and counts of the rest.
pub fn shown(p: &Program) -> Shown {
    let a = authority(p);
    let from_main = reachable(p, ["main"]);
    let from_tests = reachable(p, p.funcs.iter().filter(|(_, f)| f.test).map(|(k, _)| k.as_str()));
    let (mut main, mut tests, mut s) = (Vec::new(), Vec::new(), Shown::default());
    for line @ (k, params) in &a.funcs {
        match (from_main.contains(k), from_tests.contains(k)) {
            _ if k == "main" => main.insert(0, line),
            (false, false) => {
                s.unreached += 1;
                s.unreached_with_capability += usize::from(!params.is_empty());
            }
            _ if params.is_empty() => s.no_capability += 1,
            (true, _) => main.push(line),
            (false, true) => tests.push(line),
        }
    }
    let width = main.iter().chain(&tests).map(|(n, _)| n.len()).max().unwrap_or(0) + 1;
    s.main = lines(main.into_iter(), width);
    s.tests = lines(tests.into_iter(), width);
    s
}

/// Every function a call of `roots` may run, `roots` included. A function counts once a reached
/// body names it, called or not: as a value passed, returned or stored in a struct, list or map,
/// in a closure (called or not) or in dead code, under any type arguments. A name is taken as the
/// checker resolved it; one it did not record (a local holding a function value, say) is taken as
/// the function of that spelling in the body's own module, as the checker would resolve it. A
/// module path `web::f` names only `web::f`, never the caller's own `f`.
pub fn reachable<'a>(p: &Program, roots: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut todo: Vec<String> = roots.into_iter().map(String::from).collect();
    while let Some(k) = todo.pop() {
        let Some(f) = p.funcs.get(&k) else { continue };
        if !seen.insert(k.clone()) {
            continue;
        }
        let module = module_of(&k);
        // The inner name of each `web::f` seen: resolved by its `Module` expression, not here.
        let mut pathed = HashSet::new();
        walk_block(&f.body, &mut |n| {
            let Node::Expr(e) = n else { return };
            todo.extend(p.instances.get(&e.span).map(|(g, _)| g.clone()));
            let resolved = p.item_uses.get(&e.span);
            todo.extend(resolved.cloned());
            match &e.kind {
                ExprKind::Name(g) | ExprKind::Turbofish(g, _) if resolved.is_none() && !pathed.contains(&e.span) => {
                    todo.push(key(module, g).into_owned())
                }
                ExprKind::Module(m, inner) => {
                    if let ExprKind::Name(g) | ExprKind::Turbofish(g, _) = &inner.kind {
                        pathed.insert(inner.span);
                        todo.push(format!("{m}::{g}"));
                    }
                }
                _ => {}
            }
        });
    }
    seen
}

/// The constants, structs and enums the functions `funcs` name, by key: each constant one reads,
/// each struct or enum in its signature or the type of an expression in its body, and those a
/// struct's fields or an enum's payloads hold, in turn.
pub fn items_used<'a>(p: &Program, funcs: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut tys: Vec<Ty> = Vec::new();
    for k in funcs {
        let Some(f) = p.funcs.get(k) else { continue };
        let s = f.span;
        let inside = |t: &crate::Span| t.file == s.file && t.start >= s.start && t.end <= s.end;
        let consts = (p.item_uses.iter()).filter(|(t, item)| inside(t) && p.consts.contains_key(*item));
        out.extend(consts.map(|(_, item)| item.clone()));
        tys.extend(p.types.iter().filter(|(t, _)| inside(t)).map(|(_, ty)| ty.clone()));
        if let Some(sig) = p.sigs.get(k) {
            tys.extend(sig.params.iter().cloned());
            tys.push(sig.ret.clone());
        }
    }
    while let Some(t) = tys.pop() {
        match t {
            Ty::Group(k) => _ = out.insert(k),
            Ty::Struct(k, args) | Ty::Enum(k, args) => {
                tys.extend(args);
                if out.insert(k.clone()) {
                    tys.extend(p.fields.get(&k).into_iter().flatten().map(|(_, t)| t.clone()));
                    tys.extend(p.variants.get(&k).into_iter().flatten().flat_map(|(_, ts)| ts.clone()));
                }
            }
            Ty::Option(a) | Ty::List(a) => tys.push(*a),
            Ty::Result(a, b) | Ty::Map(a, b) | Ty::HashMap(a, b) => tys.extend([*a, *b]),
            Ty::Fn(ps, r) => {
                tys.extend(ps);
                tys.push(*r);
            }
            _ => {}
        }
    }
    out
}

/// Each comparison and logical operator in function `key`'s body (`<`, `<=`, `>`, `>=`, `==`,
/// `!=`, `&&`, `||`): the span of its left operand, of its right, and the operator.
pub fn operators(p: &Program, key: &str) -> Vec<(crate::Span, crate::Span, &'static str)> {
    let mut out = Vec::new();
    let Some(f) = p.funcs.get(key) else { return out };
    walk_block(&f.body, &mut |n| {
        if let Node::Expr(Expr {
            kind: ExprKind::Binary(op @ ("<" | "<=" | ">" | ">=" | "==" | "!=" | "&&" | "||"), l, r),
            ..
        }) = n
        {
            out.push((l.span, r.span, *op));
        }
    });
    out
}

/// `hook send POST PUT, log write`: an `Http` method's verbs follow it once.
fn uses_str(params: &[(String, BTreeSet<String>)]) -> String {
    let parts: Vec<String> = params
        .iter()
        .map(|(d, ms)| {
            let mut words = vec![d.as_str()];
            let mut last = "";
            for m in ms {
                let (name, verb) = split_verb(m);
                if name != last {
                    words.push(name);
                    last = name;
                }
                words.extend(verb);
            }
            words.join(" ")
        })
        .collect();
    parts.join(", ")
}

/// `send PUT` is (`send`, `PUT`); a method without a verb is itself.
fn split_verb(m: &str) -> (&str, Option<&str>) {
    m.split_once(' ').map_or((m, None), |(n, v)| (n, Some(v)))
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
    /// Some grant line adds authority.
    pub grant_added: bool,
}

impl AuthorityDiff {
    /// The lines `kio check --base` prints: one per changed grant; then, with `detail` or when
    /// those lines do not show every addition, a blank line and one per changed function. And
    /// whether the function lines are included.
    pub fn lines(&self, detail: bool) -> (Vec<String>, bool) {
        let funcs = detail || (self.added && !self.grant_added) || self.grants.is_empty();
        let mut out = self.grants.clone();
        if funcs {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.extend(self.funcs.iter().cloned());
        }
        (out, funcs)
    }

    /// What `kio check --base` prints on stdout: `lines`, then whether authority was added
    /// (exit 3), only removed, or covered.
    pub fn text(&self, detail: bool) -> String {
        if self.grants.is_empty() && self.funcs.is_empty() {
            return "no change in authority".into();
        }
        let (mut out, funcs) = self.lines(detail);
        let last = if !self.added {
            // A gain a base method covers (P6) is shown but adds nothing; judged by the lines printed.
            match out.iter().any(|l| l.contains(" is covered by ")) {
                true => "no authority added",
                false => "authority removed only",
            }
        } else if funcs || self.funcs.is_empty() {
            "authority added (exit 3)"
        } else {
            "authority added (exit 3); --detail lists each function"
        };
        out.push(last.into());
        out.join("\n")
    }
}

/// Every capability method with the methods that cover it: a gained method that a covering
/// one already reached on the same grant (or function parameter) in the base is not new
/// authority (P6, open_questions #208). Every caps.wit method is listed; `[]` is standalone.
/// An `Http` method carries its verb (P7, #219): a verb is covered by the same verb or `*` on
/// a covering method, and by `*` on its own; `*` only by `*`. `get` is `GET`.
pub const COVERS: &[(&str, &str, &[&str])] = &[
    ("Dir", "read", &[]),
    ("Dir", "create", &["write"]),
    ("Dir", "list", &[]),
    ("Dir", "write", &[]),
    ("Dir", "remove", &[]),
    ("Dir", "exists", &["read"]),
    ("Dir", "append", &["write"]),
    ("Http", "get", &["send", "send_all"]),
    ("Http", "send", &["send_all"]),
    ("Http", "send_all", &["send"]),
    ("Incoming", "method", &[]),
    ("Incoming", "target", &[]),
    ("Incoming", "header_names", &[]),
    ("Incoming", "header_values", &[]),
    ("Incoming", "body", &[]),
    ("Outgoing", "send", &[]),
    ("Kv", "get", &["scan"]),
    ("Kv", "set", &[]),
    ("Kv", "delete", &[]),
    ("Kv", "keys", &["scan"]),
    ("Kv", "swap", &[]),
    ("Kv", "scan", &[]),
    ("Kv", "add", &[]),
    ("WsMessage", "kind", &[]),
    ("WsMessage", "conn", &[]),
    ("WsMessage", "data", &[]),
    ("WsMessage", "target", &[]),
    ("WsMessage", "header", &[]),
    ("WsSend", "text", &["binary"]),
    ("WsSend", "binary", &["text"]),
    ("WsSend", "close", &["text", "binary"]),
    ("Rng", "bytes", &[]),
    ("Rng", "int", &[]),
    ("Clock", "now_ms", &[]),
    ("Clock", "sleep_ms", &[]),
    ("Log", "write", &[]),
    ("Db", "query", &["exec_rows", "transaction_rows"]),
    ("Db", "exec", &["transaction", "exec_rows", "transaction_rows"]),
    ("Db", "exec_rows", &["transaction_rows"]),
    ("Db", "transaction", &["exec", "exec_rows", "transaction_rows"]),
    ("Db", "transaction_rows", &["exec_rows"]),
    ("Hmac", "sign", &[]),
    ("Hmac", "verify", &["sign"]),
    ("Secret", "matches", &[]),
    ("Value", "get", &[]),
];

/// Of `gains` on a capability of type `ty` (`None`: nothing is covered), whether any is
/// uncovered by `base`, and a note for each covered one: `create is covered by write`.
fn split_gains(ty: Option<&str>, gains: &BTreeSet<String>, base: &BTreeSet<String>) -> (bool, Vec<String>) {
    let mut uncovered = false;
    let mut notes = Vec::new();
    for m in gains {
        let (name, verb) = split_verb(m);
        let verb = verb.or((ty == Some("Http") && name == "get").then_some("GET"));
        let mut by: Vec<String> = Vec::new();
        if verb.is_some_and(|v| v != "*") && name != "get" {
            by.push(format!("{name} *"));
        }
        let row = COVERS.iter().find(|(t, n, _)| Some(*t) == ty && *n == name);
        for c in row.map_or(&[][..], |(_, _, by)| by) {
            match verb {
                Some(v) => by.extend([format!("{c} {v}"), format!("{c} *")]),
                None => by.push(c.to_string()),
            }
        }
        by.dedup();
        match by.iter().find(|c| base.contains(*c)) {
            Some(c) => notes.push(format!("{m} is covered by {c}")),
            None => uncovered = true,
        }
    }
    (uncovered, notes)
}

/// `; a is covered by b, ..`, or nothing.
fn notes_str(notes: &[String]) -> String {
    if notes.is_empty() {
        String::new()
    } else {
        format!("; {}", notes.join(", "))
    }
}

/// The capability type of leaf `d` of function `f`.
fn param_ty(p: &Program, f: &str, d: &str) -> Option<String> {
    p.funcs
        .contains_key(f)
        .then(|| p.leaves(f))?
        .into_iter()
        .find(|(e, _)| e == d)
        .map(|(_, t)| t)
}

/// The authority `new` has that `old` lacks, and the reverse. A gained method covered by one
/// the base reached on the same grant (or function parameter) carries a note and adds nothing.
/// Only functions `main` reaches count (P8): code no run can call adds no authority.
pub fn report_diff(old_p: &Program, new_p: &Program) -> AuthorityDiff {
    let (old, new) = (main_authority(old_p), main_authority(new_p));
    let mut diff = AuthorityDiff::default();
    for g @ (d, ty) in &new.main {
        let ms = reached(&new, d);
        if !old.main.contains(g) {
            diff.grants.push(format!("+ {d}: {ty}{}", in_parens(ms)));
            diff.grant_added = true;
            continue;
        }
        let was = reached(&old, d);
        let gains: BTreeSet<String> = ms.difference(&was).cloned().collect();
        let (uncovered, notes) = split_gains(Some(ty), &gains, &was);
        let gained: Vec<String> = gains.iter().map(|m| format!("+{m}")).collect();
        let lost: Vec<String> = was.difference(&ms).map(|m| format!("-{m}")).collect();
        if !gained.is_empty() || !lost.is_empty() {
            diff.grant_added |= uncovered;
            let items = in_parens([gained, lost].concat());
            diff.grants.push(format!("~ {d}: {ty}{items}{}", notes_str(&notes)));
        }
    }
    diff.added = diff.grant_added;
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
            let (mut only, mut notes, mut uncovered) = (Vec::new(), Vec::new(), false);
            for (d, ms) in a {
                let theirs = b.iter().find(|(e, _)| e == d).map(|(_, t)| t);
                let ms: BTreeSet<String> = ms
                    .iter()
                    .filter(|m| !theirs.is_some_and(|t| t.contains(*m)))
                    .cloned()
                    .collect();
                if ms.is_empty() {
                    continue;
                }
                if sign == '+' {
                    // Covered only by the base's methods on a parameter of the same type.
                    let ty = param_ty(new_p, name, d).filter(|t| param_ty(old_p, name, d).as_ref() == Some(t));
                    let (u, n) = split_gains(ty.as_deref(), &ms, theirs.unwrap_or(&BTreeSet::new()));
                    uncovered |= u;
                    notes.extend(n);
                }
                only.push((d.clone(), ms));
            }
            if !only.is_empty() {
                diff.added |= uncovered;
                diff.funcs
                    .push(format!("{sign} {name}: {}{}", uses_str(&only), notes_str(&notes)));
            }
        }
    }
    diff
}

/// `authority` of the functions `main` reaches (P8).
fn main_authority(p: &Program) -> Authority {
    let mut a = authority(p);
    let from_main = reachable(p, ["main"]);
    a.funcs.retain(|k, _| from_main.contains(k));
    a
}

/// The functions `main` reaches whose report line has one of `methods` on a parameter `main`'s
/// parameter `grant` is passed to (`passed`), `main`'s own `grant` included, in key order.
pub fn users(p: &Program, grant: &str, methods: &[&str]) -> Vec<String> {
    let a = main_authority(p);
    let to = passed(p, grant);
    let uses = |f: &String, params: &[(String, BTreeSet<String>)]| {
        (params.iter()).any(|(d, ms)| to.contains(&(f.clone(), d.clone())) && methods.iter().any(|m| ms.contains(*m)))
    };
    a.funcs
        .iter()
        .filter(|(f, ps)| uses(f, ps))
        .map(|(f, _)| f.clone())
        .collect()
}

/// Each `(function, leaf)` `main`'s parameter `grant` reaches, `("main", grant)` included: the
/// closure of the edge maps from it, as `uses` charges methods.
fn passed(p: &Program, grant: &str) -> BTreeSet<(String, String)> {
    passed_in(&edges(p), grant)
}

fn passed_in(e: &Edges, grant: &str) -> BTreeSet<(String, String)> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![("main".to_string(), grant.to_string())];
    while let Some(fd) = todo.pop() {
        if !seen.insert(fd.clone()) {
            continue;
        }
        for (caller, callee, map) in &e.calls {
            if *caller != fd.0 {
                continue;
            }
            todo.extend(
                map.iter()
                    .filter(|(_, from)| *from == fd.1)
                    .map(|(to, _)| (callee.clone(), to.clone())),
            );
        }
    }
    seen
}

/// The key of the function a call in function `name` names as `callee`, if it names one.
fn callee_key(name: &str, callee: &Expr) -> Option<String> {
    match &callee.kind {
        ExprKind::Name(g) | ExprKind::Turbofish(g, _) => Some(key(module_of(name), g).into_owned()),
        ExprKind::Module(m, inner) => match &inner.kind {
            ExprKind::Name(g) | ExprKind::Turbofish(g, _) => Some(format!("{m}::{g}")),
            _ => None,
        },
        _ => None,
    }
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
    let mut uses = uses(p, &edges(p));
    let mut a = Authority::default();
    // A test is not `main`'s authority: no function calls one.
    for name in p.funcs.iter().filter(|(_, f)| !f.test).map(|(k, _)| k) {
        a.funcs.insert(name.clone(), own_uses(&mut uses, p, name));
    }
    if let Some(f) = p.funcs.get("main") {
        a.main = f.params.iter().map(|d| (d.name.clone(), type_str(&d.ty))).collect();
    }
    a
}

/// The authority of function `entry` (a test's key, or `main`) as `authority` gives `main`'s:
/// its line alone, named `main`, and its leaves as `main`'s parameters, each by its short name
/// (a capability struct's field is a grant of the field's name; rule 8 keeps them distinct). The
/// host checks a test's grants and calls against it as it checks `main`'s.
pub fn entry_authority(p: &Program, entry: &str) -> Authority {
    let mut a = Authority::default();
    let short = |path: &str| path.rsplit('.').next().unwrap_or_default().to_string();
    let line = (own_uses(&mut uses(p, &edges(p)), p, entry).into_iter())
        .map(|(d, ms)| (short(&d), ms))
        .collect();
    a.funcs.insert("main".into(), line);
    // A property's data parameters are generated inputs, not grants.
    a.main = p.test_leaves(entry);
    a
}

/// The capability leaves of function `name` that call something, with the methods, in
/// parameter then field order.
fn own_uses(uses: &mut Uses, p: &Program, name: &str) -> Vec<(String, BTreeSet<String>)> {
    let mut params = Vec::new();
    for (d, _) in p.leaves(name) {
        if let Some(ms) = uses.remove(&(name.to_string(), d.clone())).filter(|ms| !ms.is_empty()) {
            params.push((d, ms));
        }
    }
    params
}

/// The methods each capability leaf of each function may call, through helpers too: the
/// fixpoint of `effects(f) ⊇ map_s(effects(g))` over the edge maps.
fn uses(p: &Program, e: &Edges) -> Uses {
    let mut uses = Uses::new();
    for name in p.funcs.keys() {
        for (d, _) in p.leaves(name) {
            uses.insert((name.clone(), d), BTreeSet::new());
        }
    }
    for (f, d, m) in &e.direct {
        if let Some(set) = uses.get_mut(&(f.clone(), d.clone())) {
            set.insert(m.clone());
        }
    }
    // Sets only grow and are bounded by the method names, so this stops.
    let mut grown = true;
    while grown {
        grown = false;
        for (f, g, map) in &e.calls {
            for (to, from) in map {
                let Some(ms) = uses.get(&(g.clone(), to.clone())).cloned() else {
                    continue;
                };
                if let Some(set) = uses.get_mut(&(f.clone(), from.clone())) {
                    for m in ms {
                        grown |= set.insert(m);
                    }
                }
            }
        }
    }
    uses
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

/// The verb `e` names when the checker can see it, a string literal or a constant, else `*`.
/// A verb is capital letters (the host refuses any other as `Invalid`), so another literal is
/// `*` too and a report's words stay unambiguous.
fn verb(p: &Program, e: &Expr) -> String {
    let lit = match &e.kind {
        ExprKind::Name(_) | ExprKind::Module(..) => p
            .item_uses
            .get(&e.span)
            .and_then(|k| p.consts.get(k))
            .map(|c| &c.value.kind),
        k => Some(k),
    };
    match lit {
        Some(ExprKind::Str(s)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase()) => s.clone(),
        _ => "*".into(),
    }
}

/// The verbs a `send_all` of `arg`, in function `home`, may send (#220): what `list_verbs`
/// finds, or `*` if any is unseen or there is none.
fn send_all_verbs(p: &Program, home: &Fn, arg: &Expr) -> BTreeSet<String> {
    let mut cx = Cx {
        p,
        home,
        verbs: BTreeSet::new(),
        fns: BTreeSet::new(),
        locals: BTreeSet::new(),
    };
    cx.list_verbs(arg);
    if cx.verbs.is_empty() || cx.verbs.contains("*") {
        return BTreeSet::from(["*".to_string()]);
    }
    cx.verbs
}

/// The verbs found for one `send_all`, and the functions and locals already followed.
struct Cx<'a> {
    p: &'a Program,
    home: &'a Fn,
    verbs: BTreeSet<String>,
    fns: BTreeSet<&'a str>,
    locals: BTreeSet<(&'a str, bool)>,
}

impl<'a> Cx<'a> {
    fn star(&mut self) {
        self.verbs.insert("*".into());
    }

    /// A `List<HttpRequest>`: a list literal of requests, `xs.map(f)` with `f` a named function,
    /// or a local of `home`; `*` for anything else.
    fn list_verbs(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::List(items) => items.iter().for_each(|e| self.request_verbs(e)),
            ExprKind::Method(_, m, args) if m == "map" && args.len() == 1 => match named_fn(self.p, &args[0]) {
                Some(f) => fn_verbs(self.p, f, &mut self.verbs, &mut self.fns),
                None => self.star(),
            },
            ExprKind::Name(n) if !self.p.item_uses.contains_key(&e.span) => self.local_verbs(n, true),
            _ => self.star(),
        }
    }

    /// An `HttpRequest`: a literal's `method` (or its base's), what a named function returns, or
    /// a local of `home`; `*` for anything else.
    fn request_verbs(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::StructLit(_, fields, base) => match (fields.iter().find(|(f, _)| f == "method"), base) {
                (Some((_, v)), _) => _ = self.verbs.insert(verb(self.p, v)),
                (None, Some(b)) => self.request_verbs(b),
                (None, None) => self.star(),
            },
            ExprKind::Call(callee, _) => match named_fn(self.p, callee) {
                Some(f) => fn_verbs(self.p, f, &mut self.verbs, &mut self.fns),
                None => self.star(),
            },
            ExprKind::Name(n) if !self.p.item_uses.contains_key(&e.span) => self.local_verbs(n, false),
            _ => self.star(),
        }
    }

    /// Local `name` of `home` (a `List<HttpRequest>` if `list`, else an `HttpRequest`): every
    /// `let name = ..`, assignment to it or into it, and `name.push(..)` in `home`, whichever
    /// variable of that name each is. `*` if a parameter or a pattern, `for` or closure binds the
    /// name, or another method that could add a request is called on it. Values are copied, so
    /// nothing else can change it.
    fn local_verbs(&mut self, name: &'a str, list: bool) {
        if !self.locals.insert((name, list)) {
            return;
        }
        if self.home.params.iter().any(|d| d.name == name) {
            return self.star();
        }
        enum Src<'a> {
            Whole(&'a Expr),
            Elem(&'a Expr),
            Verb(&'a Expr),
            Star,
        }
        let p = self.p;
        let mut found = Vec::new();
        walk_block(&self.home.body, &mut |n| match n {
            Node::Let(n, init) if n == name => found.push(Src::Whole(init)),
            Node::Bind(n) if n == name => found.push(Src::Star),
            Node::Assign(target, op, value) if root(target) == Some(name) => {
                let src = match &target.kind {
                    _ if op != "=" => Src::Star,
                    ExprKind::Name(_) => Src::Whole(value),
                    ExprKind::Index(b, _) if list && matches!(b.kind, ExprKind::Name(_)) => Src::Elem(value),
                    ExprKind::Field(_, f) if f == "method" => Src::Verb(value),
                    _ => return,
                };
                found.push(src);
            }
            Node::Expr(e) => {
                let ExprKind::Method(recv, m, args) = &e.kind else {
                    return;
                };
                if !matches!(&recv.kind, ExprKind::Name(n) if n == name) {
                    return;
                }
                let carry = |a: &Expr| p.types.get(&a.span).is_none_or(|t| carries(p, t, &mut BTreeSet::new()));
                match (m.as_str(), args.as_slice()) {
                    ("push", [v]) if list => found.push(Src::Elem(v)),
                    _ if args.iter().any(carry) => found.push(Src::Star),
                    _ => {}
                }
            }
            _ => {}
        });
        for src in found {
            match src {
                Src::Whole(e) if list => self.list_verbs(e),
                Src::Whole(e) | Src::Elem(e) => self.request_verbs(e),
                Src::Verb(v) => _ = self.verbs.insert(verb(p, v)),
                Src::Star => self.star(),
            }
        }
    }
}

/// The local a place (`x`, `x.f`, `x[i].f`) is rooted at.
fn root(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Name(n) => Some(n),
        ExprKind::Field(b, _) | ExprKind::Index(b, _) => root(b),
        _ => None,
    }
}

/// Adds the verbs of every `HttpRequest` function `k` may return: its body's `HttpRequest`
/// literals and assignments to a `method`, and those of every named function it names. An
/// `HttpRequest` can come only from those or from a parameter: `*` if a parameter could carry
/// one (an `HttpRequest`, a function or a type parameter, or a type holding one).
fn fn_verbs<'a>(p: &'a Program, k: &'a str, verbs: &mut BTreeSet<String>, seen: &mut BTreeSet<&'a str>) {
    if !seen.insert(k) {
        return;
    }
    if p.sigs[k].params.iter().any(|t| carries(p, t, &mut BTreeSet::new())) {
        verbs.insert("*".into());
        return;
    }
    let mut named = Vec::new();
    walk_block(&p.funcs[k].body, &mut |n| match n {
        Node::Expr(e) => {
            if let (ExprKind::StructLit(_, fields, _), true) = (&e.kind, is_request(p, e)) {
                verbs.extend(fields.iter().filter(|(f, _)| f == "method").map(|(_, v)| verb(p, v)));
            }
            named.extend(named_fn(p, e));
        }
        Node::Assign(target, op, value) => {
            if let ExprKind::Field(base, f) = &target.kind {
                if f == "method" && is_request(p, base) {
                    verbs.insert(if op == "=" { verb(p, value) } else { "*".into() });
                }
            }
        }
        Node::Let(..) | Node::Bind(_) => {}
    });
    for g in named {
        fn_verbs(p, g, verbs, seen);
    }
}

fn is_request(p: &Program, e: &Expr) -> bool {
    matches!(p.types.get(&e.span), Some(Ty::Struct(n, _)) if n == "HttpRequest")
}

/// The function `e` names (called or as a value), as the checker resolved it.
fn named_fn<'a>(p: &'a Program, e: &Expr) -> Option<&'a str> {
    p.item_uses
        .get(&e.span)
        .map(String::as_str)
        .filter(|k| p.funcs.contains_key(*k))
}

/// Whether a value of type `t` could hold an `HttpRequest` or make one (a function, or a type
/// parameter that may be either).
fn carries(p: &Program, t: &Ty, seen: &mut BTreeSet<String>) -> bool {
    match t {
        Ty::Fn(..) | Ty::Param(_) => true,
        Ty::Struct(k, args) | Ty::Enum(k, args) => {
            if k == "HttpRequest" || args.iter().any(|a| carries(p, a, seen)) {
                return true;
            }
            if !seen.insert(k.clone()) {
                return false;
            }
            let fields = p.fields.get(k).into_iter().flatten().map(|(_, t)| t);
            let payloads = p.variants.get(k).into_iter().flatten().flat_map(|(_, ts)| ts);
            fields.chain(payloads).any(|t| carries(p, t, seen))
        }
        Ty::Option(a) | Ty::List(a) => carries(p, a, seen),
        Ty::Result(a, b) | Ty::Map(a, b) | Ty::HashMap(a, b) => carries(p, a, seen) || carries(p, b, seen),
        _ => false,
    }
}

/// `main`'s literal HTTP verbs, as `(parameter, verb)` in report order: what the host checks
/// against `--http-methods` at start (P7). `*` is left to the per-request check.
pub fn main_http_verbs(a: &Authority) -> Vec<(String, String)> {
    let main = a.funcs.get("main").map_or(&[][..], Vec::as_slice);
    let mut out = Vec::new();
    for (d, ms) in main {
        for m in ms {
            if let (_, Some(v)) = split_verb(m) {
                if v != "*" {
                    out.push((d.clone(), v.to_string()));
                }
            }
        }
    }
    out
}

/// The HTTP methods a request may use; the host refuses any other as `Invalid`.
pub const HTTP_METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// What `walk_block` visits: each expression; each assignment `target op value`, and each
/// `let name = init`, before the expressions in it; and each other name bound (a pattern, a
/// `for` variable, a closure parameter).
enum Node<'a> {
    Expr(&'a Expr),
    Assign(&'a Expr, &'a str, &'a Expr),
    Let(&'a str, &'a Expr),
    Bind(&'a str),
}

/// Calls `visit` with `Node::Bind` for each name `pat` binds.
fn bind_pat<'a>(pat: &'a Pat, visit: &mut dyn FnMut(Node<'a>)) {
    match &pat.kind {
        PatKind::Name(n) => visit(Node::Bind(n)),
        PatKind::Variant(_, _, ps) | PatKind::Or(ps) => ps.iter().for_each(|q| bind_pat(q, visit)),
        PatKind::Module(_, q) => bind_pat(q, visit),
        PatKind::Wild | PatKind::Int(_) | PatKind::Str(_) | PatKind::Bool(_) => {}
    }
}

/// Calls `visit` on every expression in the block, outer before inner.
fn walk_block<'a>(b: &'a Block, visit: &mut dyn FnMut(Node<'a>)) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { name, init, .. } => {
                visit(Node::Let(name, init));
                walk(init, visit);
            }
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) => walk(e, visit),
            StmtKind::Assign { target, op, value } => {
                visit(Node::Assign(target, op, value));
                walk(target, visit);
                walk(value, visit);
            }
            StmtKind::LetElse { pat, init, els, .. } => {
                bind_pat(pat, visit);
                walk(init, visit);
                walk_block(els, visit);
            }
            StmtKind::Loop { body } => walk_block(body, visit),
            StmtKind::While { cond, body } => {
                walk(cond, visit);
                walk_block(body, visit);
            }
            StmtKind::For { name, iter, body } => {
                visit(Node::Bind(name));
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

fn walk<'a>(e: &'a Expr, visit: &mut dyn FnMut(Node<'a>)) {
    visit(Node::Expr(e));
    match &e.kind {
        // A closure's body is charged to the function it is written in. It can reach no
        // capability (the checker refuses a capture of one), so this only keeps the report an
        // upper bound should that rule ever fail.
        // `web::Row { b: d.read("a") }` is a module path around a struct literal: walk into it.
        ExprKind::Unary(_, a) | ExprKind::Try(a) | ExprKind::Field(a, _) | ExprKind::Module(_, a) => walk(a, visit),
        ExprKind::Closure(ps, a) => {
            ps.iter().for_each(|q| visit(Node::Bind(&q.name)));
            walk(a, visit);
        }
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
        ExprKind::IfLet(pat, s, then, els) => {
            bind_pat(pat, visit);
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
            for a in arms {
                bind_pat(&a.pat, visit);
                walk(&a.body, visit);
            }
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

/// The capability leaf `e` names, by its path, as the checker resolved it: a parameter `d`, or
/// a field `env.db` of a capability struct parameter. The checker records a capability type
/// only for a name bound to a parameter of the function it is in (rule 1), so a local, closure
/// parameter, pattern binding or loop variable that shadows the parameter's name is not charged
/// to it (P5).
fn leaf_of(p: &Program, e: &Expr) -> Option<String> {
    match (&e.kind, p.types.get(&e.span)?) {
        (ExprKind::Name(d), Ty::Cap(_)) => Some(d.clone()),
        (ExprKind::Field(base, f), Ty::Cap(_)) => match (&base.kind, p.types.get(&base.span)?) {
            (ExprKind::Name(g), Ty::Group(_)) => Some(format!("{g}.{f}")),
            _ => None,
        },
        _ => None,
    }
}
