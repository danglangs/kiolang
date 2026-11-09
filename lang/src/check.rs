use crate::ast::*;
use crate::eval::Value;
use crate::ir::{self, EnumRef};
use crate::parse::parse_file;
use crate::{Diag, Span};
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;

/// The capability types a program may use, and the error enums their methods return.
#[derive(Debug, Default)]
pub struct CapTable {
    pub caps: Vec<CapType>,
    /// Declared as ordinary enums before the user's types, such as `IoErr`.
    pub enums: Vec<Enum>,
}

impl CapTable {
    /// `Dir` and `Http` and their error enums, generated from caps.wit by tools/capgen.
    pub fn v0() -> CapTable {
        crate::caps_gen::v0()
    }
}

#[derive(Debug)]
pub struct CapType {
    pub name: String,
    /// Method name, parameter types and return type.
    pub methods: Vec<(String, Vec<Ty>, Ty)>,
}

/// A program that passed `check`. Only `check` builds one.
#[derive(Debug)]
pub struct Program {
    /// Items are keyed by `key`: `f` in the root, `web::f` in module `web`.
    pub(crate) funcs: HashMap<String, Fn>,
    pub(crate) structs: HashMap<String, Struct>,
    pub(crate) enums: HashMap<String, Enum>,
    /// Return type of each capability method, keyed by (capability, method), such as (`Dir`, `read`).
    pub(crate) cap_methods: HashMap<(String, String), Ty>,
    /// Resolved payload types of every enum, for checking host values.
    pub(crate) variants: Variants,
    pub(crate) fields: Fields,
    /// Resolved signature of each function, by key.
    pub(crate) sigs: HashMap<String, Sig>,
    /// The type of every expression and assignment target, by its span, which no other
    /// expression has; and the type of each `let`'s local, by the span of the `let`.
    pub(crate) types: HashMap<Span, Ty>,
    /// The outer locals each closure uses, by the closure's span, in order of first use: what
    /// it copies when it is created.
    pub(crate) captures: HashMap<Span, Vec<String>>,
    /// The program lowered, which `run` evaluates.
    pub(crate) ir: ir::Module,
}

impl Program {
    /// Name and capability type of each `main` parameter, in order. The host uses it to
    /// validate grants before running.
    pub fn main_params(&self) -> Vec<(String, String)> {
        let params = &self.funcs["main"].params;
        params
            .iter()
            .filter_map(|p| match &p.ty.kind {
                TypeKind::Named(ty, _) => Some((p.name.clone(), ty.clone())),
                _ => None,
            })
            .collect()
    }

    /// `v` as `kio run` prints it: a top-level `Str` bare, everything else like its source form
    /// (`["a", 1]`, `Point { x: 1 }`, `Shape::Circle(2)`, `Some(3)`), with declared field and variant names.
    pub fn display(&self, v: &Value) -> String {
        match v {
            Value::Str(s) => s.to_string(),
            _ => self.nested(v),
        }
    }

    fn nested(&self, v: &Value) -> String {
        let list = |vs: &[Value]| vs.iter().map(|v| self.nested(v)).collect::<Vec<_>>().join(", ");
        match v {
            Value::Int(n) => n.to_string(),
            Value::Float(x) => x.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => format!("{s:?}"),
            Value::Bytes(b) => format!("<{} bytes>", b.len()),
            Value::Unit => "()".to_string(),
            Value::List(vs) => format!("[{}]", list(vs)),
            Value::Map(m) => {
                let entries: Vec<_> = m
                    .iter()
                    .map(|(k, v)| format!("{}: {}", self.nested(&k.to_value()), self.nested(v)))
                    .collect();
                format!("{{{}}}", entries.join(", "))
            }
            Value::Struct(s, vs) => {
                let s = &self.ir.structs[s.0 as usize];
                let fields: Vec<_> = (s.fields.iter().zip(vs))
                    .map(|((f, _), v)| format!("{f}: {}", self.nested(v)))
                    .collect();
                if fields.is_empty() {
                    s.name.clone()
                } else {
                    format!("{} {{ {} }}", s.name, fields.join(", "))
                }
            }
            Value::Enum(en, i, vs) => {
                let variant = match en {
                    EnumRef::User(e) => {
                        let e = &self.ir.enums[e.0 as usize];
                        format!("{}::{}", e.name, e.variants[*i].0)
                    }
                    EnumRef::Option => ["None", "Some"][*i].to_string(),
                    EnumRef::Result => ["Ok", "Err"][*i].to_string(),
                };
                if vs.is_empty() {
                    variant
                } else {
                    format!("{variant}({})", list(vs))
                }
            }
            Value::Cap(name) => name.clone(),
            Value::Fn(f) => format!("<fn {}>", self.ir.funcs[f.0 as usize].name),
            Value::Closure(..) => "<closure>".to_string(),
        }
    }

    pub(crate) fn is_cap(&self, ty: &str) -> bool {
        self.cap_methods.keys().any(|(c, _)| c == ty)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Bytes,
    Unit,
    Struct(String),
    Enum(String),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    /// A capability, which only a parameter can hold.
    Cap(String),
    /// A function value: parameter types and return type. Never mentions a capability.
    Fn(Vec<Ty>, Box<Ty>),
    /// A block that always returns; it matches any type.
    Never,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Ty::Int => f.write_str("Int"),
            Ty::Float => f.write_str("Float"),
            Ty::Bool => f.write_str("Bool"),
            Ty::Str => f.write_str("Str"),
            Ty::Bytes => f.write_str("Bytes"),
            Ty::Unit => f.write_str("()"),
            Ty::Struct(name) | Ty::Enum(name) | Ty::Cap(name) => f.write_str(name),
            Ty::Option(t) => write!(f, "Option<{t}>"),
            Ty::Result(t, e) => write!(f, "Result<{t}, {e}>"),
            Ty::List(t) => write!(f, "List<{t}>"),
            Ty::Map(k, v) => write!(f, "Map<{k}, {v}>"),
            Ty::Fn(ps, r) => {
                let ps: Vec<_> = ps.iter().map(Ty::to_string).collect();
                write!(f, "fn({})", ps.join(", "))?;
                match **r {
                    Ty::Unit => Ok(()),
                    _ => write!(f, " -> {r}"),
                }
            }
            Ty::Never => f.write_str("!"),
        }
    }
}

/// The key of item `name` in `module`: the name itself in the root (module ""), else `web::name`.
pub(crate) fn key<'a>(module: &str, name: &'a str) -> Cow<'a, str> {
    if module.is_empty() {
        Cow::Borrowed(name)
    } else {
        Cow::Owned(format!("{module}::{name}"))
    }
}

/// The module of a `key`; "" for the root.
pub(crate) fn module_of(key: &str) -> &str {
    key.rsplit_once("::").map_or("", |(m, _)| m)
}

/// The name of a `key` without its module.
fn bare(key: &str) -> &str {
    key.rsplit_once("::").map_or(key, |(_, n)| n)
}

/// Names that user types may not take: built-in types, capabilities and their error enums.
fn reserved(caps: &CapTable, name: &str) -> bool {
    matches!(name, "Option" | "Result" | "Map")
        || caps.enums.iter().any(|e| e.name == name)
        || caps.caps.iter().any(|c| c.name == name)
}

/// The module being checked and the modules the root declares.
#[derive(Clone, Copy)]
struct Scope<'a> {
    module: &'a str,
    mods: &'a HashSet<String>,
}

impl Scope<'_> {
    /// The key of a type `name` as written in this module; built-in types are visible everywhere.
    fn own(&self, caps: &CapTable, name: &str) -> String {
        if reserved(caps, name) {
            name.to_string()
        } else {
            key(self.module, name).into_owned()
        }
    }

    /// The key of `name` in module `m`, as written `m::name`; `m` is a declared module, possibly this one.
    fn member(&self, m: &str, name: &str, span: Span) -> Result<String, Diag> {
        if !self.mods.contains(m) {
            let hint = format!("declare it with `mod {m};` in the root file");
            return Err(err(span, format!("unknown module `{m}`"), Some(&hint)));
        }
        Ok(format!("{m}::{name}"))
    }

    /// `check_public` for a use of `key`; a module may name its own private items as `m::item`.
    fn public(&self, key: &str, public: bool, kw: &str, span: Span) -> Result<(), Diag> {
        if module_of(key) == self.module {
            return Ok(());
        }
        check_public(key, public, kw, span)
    }
}

/// Hint for a name that exists only in the root, used from a module.
const ROOT_ITEM: &str = "a module cannot use the root's items; define it in this module or pass values as arguments";

/// Rejects use of private item `key` through `module::`. `kw` is `fn`, `struct` or `enum`.
fn check_public(key: &str, public: bool, kw: &str, span: Span) -> Result<(), Diag> {
    if public {
        return Ok(());
    }
    let (module, name) = (module_of(key), bare(key));
    let hint = format!("mark it `pub {kw} {name}` in {module}.kio");
    Err(err(span, format!("`{name}` is private to `{module}`"), Some(&hint)))
}

/// `web::web::f` and the like: a path names one module.
fn nested_path(span: Span) -> Diag {
    err(
        span,
        "a path has one module".to_string(),
        Some("write `web::f`, not `web::web::f`"),
    )
}

/// Hint for rule 2: no expression produces a capability.
const NO_CAP_SYNTAX: &str = "capabilities come only as parameters of `main`";

/// Resolved field names and types of each struct, in declaration order.
pub(crate) type Fields = HashMap<String, Vec<(String, Ty)>>;

/// Resolved variant names and payload types of each enum, in declaration order.
pub(crate) type Variants = HashMap<String, Vec<(String, Vec<Ty>)>>;

/// An enum's name and its variants with payload types, as `shape` returns.
pub(crate) type Shape = (String, Vec<(String, Vec<Ty>)>);

/// A constructor a pattern tests for. `Int` and `Str` have too many to list.
#[derive(Clone, PartialEq)]
enum Ctor {
    Variant(usize),
    Bool(bool),
    Int(i64),
    Str(String),
}

/// What a pattern matches, for exhaustiveness: every value, or the values a constructor builds
/// from parts that the inner covers match.
#[derive(Clone)]
enum Cover {
    All,
    Ctor(Ctor, Vec<Cover>),
}

/// Exhaustiveness work allowed per `match`, in pattern cells visited, and its recursion depth,
/// in pattern columns; and the work allowed for all of a program's matches together, so many
/// large matches cannot cost minutes either. The check is exponential in the worst case; real
/// matches use a tiny fraction of any.
const MATCH_BUDGET: usize = 4_000_000;
const MATCH_DEPTH: usize = 256;
const PROGRAM_MATCH_BUDGET: usize = 20_000_000;

/// Which exhaustiveness limit a `match` ran into.
enum Spent {
    Match,
    Program,
}

/// Values that some patterns leave unmatched, as rows of covers; `None` when there are none.
type Witnesses = Option<Vec<Vec<Cover>>>;

/// The usefulness algorithm of Maranget, "Warnings for pattern matching" (2007), with witnesses.
struct Usefulness<'a> {
    variants: &'a Variants,
    budget: usize,
    /// What is left of `PROGRAM_MATCH_BUDGET`, shared by every match being checked.
    program: &'a Cell<usize>,
    depth: usize,
}

impl Usefulness<'_> {
    /// Every constructor of `ty` and the types of its parts; `None` for `Int` and `Str`.
    fn ctors(&self, ty: &Ty) -> Option<Vec<(Ctor, Vec<Ty>)>> {
        match ty {
            Ty::Bool => Some(vec![(Ctor::Bool(true), vec![]), (Ctor::Bool(false), vec![])]),
            Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) => {
                let vs = shape(self.variants, ty).1.into_iter().enumerate();
                Some(vs.map(|(i, (_, tys))| (Ctor::Variant(i), tys)).collect())
            }
            _ => None,
        }
    }

    /// Values that `q`, a row of covers of types `tys`, matches and no row of `rows` does.
    /// When `q` is all `Cover::All`, those are the values the rows miss. `Err` when out of budget.
    fn useful(&mut self, rows: &[Vec<Cover>], q: &[Cover], tys: &[Ty]) -> Result<Witnesses, Spent> {
        if self.depth == MATCH_DEPTH {
            return Err(Spent::Match);
        }
        self.depth += 1;
        let ws = self.step(rows, q, tys);
        self.depth -= 1;
        ws
    }

    fn step(&mut self, rows: &[Vec<Cover>], q: &[Cover], tys: &[Ty]) -> Result<Witnesses, Spent> {
        if rows.is_empty() {
            return Ok(Some(vec![q.to_vec()]));
        }
        let cost = (rows.len() + 1) * (q.len() + 1);
        self.budget = self.budget.checked_sub(cost).ok_or(Spent::Match)?;
        self.program
            .set(self.program.get().checked_sub(cost).ok_or(Spent::Program)?);
        let Some((head, rest)) = q.split_first() else {
            return Ok(None);
        };
        if let Cover::Ctor(c, parts) = head {
            return self.specialized(rows, c, parts.clone(), rest, tys);
        }
        let seen: Vec<&Ctor> = (rows.iter())
            .filter_map(|r| match &r[0] {
                Cover::Ctor(c, _) => Some(c),
                Cover::All => None,
            })
            .collect();
        let all = self.ctors(&tys[0]);
        if let Some(all) = &all {
            if all.iter().all(|(c, _)| seen.contains(&c)) {
                for (c, parts) in all {
                    let wild = vec![Cover::All; parts.len()];
                    if let Some(ws) = self.specialized(rows, c, wild, rest, tys)? {
                        return Ok(Some(ws));
                    }
                }
                return Ok(None);
            }
        }
        // A constructor is missing from the first column: only rows starting with `_` match it.
        let default: Vec<_> = (rows.iter())
            .filter(|r| matches!(r[0], Cover::All))
            .map(|r| r[1..].to_vec())
            .collect();
        let Some(ws) = self.useful(&default, rest, &tys[1..])? else {
            return Ok(None);
        };
        let heads = match all {
            Some(all) if !seen.is_empty() => (all.into_iter())
                .filter(|(c, _)| !seen.contains(&c))
                .map(|(c, parts)| Cover::Ctor(c, vec![Cover::All; parts.len()]))
                .collect(),
            _ => vec![Cover::All],
        };
        Ok(Some(
            heads.into_iter().map(|h| [vec![h], ws[0].clone()].concat()).collect(),
        ))
    }

    /// `useful` for `q` = `c(parts) rest`, over the rows that can match `c`, with `c`'s parts as columns.
    fn specialized(
        &mut self,
        rows: &[Vec<Cover>],
        c: &Ctor,
        parts: Vec<Cover>,
        rest: &[Cover],
        tys: &[Ty],
    ) -> Result<Witnesses, Spent> {
        let n = parts.len();
        let rows: Vec<_> = (rows.iter())
            .filter_map(|r| match &r[0] {
                Cover::Ctor(d, ps) if d == c => Some([ps, &r[1..]].concat()),
                Cover::Ctor(..) => None,
                Cover::All => Some([&vec![Cover::All; n], &r[1..]].concat()),
            })
            .collect();
        let part_tys = match c {
            Ctor::Variant(i) => shape(self.variants, &tys[0]).1.swap_remove(*i).1,
            _ => Vec::new(),
        };
        let q = [parts, rest.to_vec()].concat();
        let ws = self.useful(&rows, &q, &[part_tys, tys[1..].to_vec()].concat())?;
        Ok(ws.map(|ws| {
            (ws.into_iter())
                .map(|mut w| {
                    let rest = w.split_off(n);
                    [vec![Cover::Ctor(c.clone(), w)], rest].concat()
                })
                .collect()
        }))
    }

    /// `c`, a cover of type `ty`, written as a pattern; a variant whose parts are all `_` without them.
    fn show(&self, c: &Cover, ty: &Ty) -> String {
        match c {
            Cover::All => "_".to_string(),
            Cover::Ctor(Ctor::Bool(b), _) => b.to_string(),
            Cover::Ctor(Ctor::Int(n), _) => n.to_string(),
            Cover::Ctor(Ctor::Str(s), _) => format!("{s:?}"),
            Cover::Ctor(Ctor::Variant(i), parts) => {
                let (en, vs) = shape(self.variants, ty);
                let (v, tys) = &vs[*i];
                if parts.iter().all(|p| matches!(p, Cover::All)) {
                    return format!("{en}::{v}");
                }
                let parts: Vec<_> = parts.iter().zip(tys).map(|(p, t)| self.show(p, t)).collect();
                format!("{en}::{v}({})", parts.join(", "))
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Sig {
    public: bool,
    /// The declaration as written, like `fn f(a: Int, b: Str)`, for hints.
    decl: String,
    pub(crate) params: Vec<Ty>,
    pub(crate) ret: Ty,
}

struct Local {
    ty: Ty,
    mutable: bool,
}

/// A closure being checked.
struct ClosureFrame {
    /// The index in `Checker::scopes` of its parameters' scope: a local found below it is a
    /// capture.
    base: usize,
    /// Its captures, in order of first use.
    captures: Vec<String>,
    /// Whether a loop encloses it, for the error on `break` inside it.
    in_loop: bool,
}

fn err(span: Span, msg: String, hint: Option<&str>) -> Diag {
    Diag {
        span,
        msg,
        hint: hint.map(str::to_string),
    }
}

fn want(span: Span, found: &Ty, expected: &Ty) -> Result<(), Diag> {
    if *found == Ty::Never || found == expected {
        Ok(())
    } else {
        let msg = format!("expected {expected}, found {found}");
        Err(err(span, msg, None))
    }
}

/// Like `want` at the span of `e`; hints at a stray `;` when `e` is a block that ends in an expression statement.
fn want_expr(e: &Expr, found: &Ty, expected: &Ty) -> Result<(), Diag> {
    want(e.span, found, expected).map_err(|mut d| {
        if let ExprKind::Block(Block { stmts, tail: None, .. }) = &e.kind {
            let ends_in_expr = stmts.last().is_some_and(|s| matches!(s.kind, StmtKind::Expr(_)));
            if ends_in_expr && *found == Ty::Unit {
                d.hint = Some("remove the trailing `;` to return the value".to_string());
            }
        }
        d
    })
}

/// Edits to turn `a` into `b`, counting a swap of neighbours as one (`cuont` -> `count`).
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() {
        for j in 0..=b.len() {
            d[i][j] = if i == 0 || j == 0 {
                i + j
            } else {
                let sub = d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]);
                let swap = if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                    d[i - 2][j - 2] + 1
                } else {
                    usize::MAX
                };
                sub.min(d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(swap)
            };
        }
    }
    d[a.len()][b.len()]
}

/// The candidate nearest to `name`, within a third of its length (at least 1 edit); ties go to the first by name.
pub fn closest<'a>(name: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let limit = 1.max(name.chars().count() / 3);
    let near = candidates.iter().map(|c| (edit_distance(name, c), *c));
    near.filter(|(d, _)| *d <= limit).min().map(|(_, c)| c)
}

/// "did you mean `x`?", or a list of the `kind` that exist.
fn suggest(name: &str, candidates: &[&str], kind: &str) -> String {
    match closest(name, candidates) {
        Some(c) => format!("did you mean `{c}`?"),
        None if candidates.is_empty() => format!("no {kind} are declared"),
        None => format!("{kind}: {}", candidates.join(", ")),
    }
}

/// "`E::V` holds (Int, Str)", for the hint on a wrong payload count.
fn holds(en: &str, v: &str, payload: &[Ty]) -> String {
    let tys: Vec<_> = payload.iter().map(Ty::to_string).collect();
    format!("`{en}::{v}` holds ({})", tys.join(", "))
}

/// The names of the items in `module`, sorted.
fn sorted_names<'a, V>(m: &'a HashMap<String, V>, module: &str) -> Vec<&'a str> {
    let mut names: Vec<_> = m.keys().filter(|k| module_of(k) == module).map(|k| bare(k)).collect();
    names.sort();
    names
}

/// What an operator accepts, for the hint on `cannot be applied to`.
fn op_hint(op: &str) -> String {
    let accepts = match op {
        "+" => "two Int, two Float or two Str",
        "-" | "*" | "/" | "%" | "<" | "<=" | ">" | ">=" => "two Int or two Float",
        "&&" | "||" => "two Bool",
        _ => "two values of one type that holds no capability",
    };
    format!("`{op}` works on {accepts}")
}

/// The error for `l op r` that `binary_ty` refused, at `span`; `written` is `op` or `op=`.
fn binary_err(op: &str, written: &str, l: &Ty, r: &Ty, span: Span) -> Diag {
    if matches!((l, r), (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int)) && op != "&&" && op != "||" {
        let verb = match op {
            "+" => "add",
            "-" => "subtract",
            "*" => "multiply",
            "/" => "divide",
            "%" => "take the remainder of",
            _ => "compare",
        };
        let hint = Some("convert with `.to_float()` or `.to_int()`");
        return err(span, format!("cannot {verb} {l} and {r}"), hint);
    }
    err(
        span,
        format!("`{written}` cannot be applied to {l} and {r}"),
        Some(&op_hint(op)),
    )
}

/// The result type of `l op r`, or `None` if the operator does not apply.
fn binary_ty(op: &str, l: &Ty, r: &Ty) -> Option<Ty> {
    use Ty::*;
    match (op, l, r) {
        ("+", Str, Str) => Some(Str),
        ("+" | "-" | "*" | "/" | "%", Int, Int) => Some(Int),
        ("<" | "<=" | ">" | ">=", Int, Int) => Some(Bool),
        ("+" | "-" | "*" | "/" | "%", Float, Float) => Some(Float),
        ("<" | "<=" | ">" | ">=", Float, Float) => Some(Bool),
        ("&&" | "||", Bool, Bool) => Some(Bool),
        ("==" | "!=", a, b) if a == b && *a != Never && cap_in(a).is_none() => Some(Bool),
        _ => None,
    }
}

fn resolve(
    t: &Type,
    caps: &CapTable,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
    scope: Scope,
) -> Result<Ty, Diag> {
    match &t.kind {
        TypeKind::Unit => Ok(Ty::Unit),
        TypeKind::Fn(ps, r) => {
            let ps = ps.iter().map(|p| resolve(p, caps, structs, enums, scope));
            let ps = ps.collect::<Result<_, _>>()?;
            Ok(Ty::Fn(ps, Box::new(resolve(r, caps, structs, enums, scope)?)))
        }
        TypeKind::Module(m, inner) => {
            let TypeKind::Named(n, _) = &inner.kind else {
                return Err(nested_path(t.span));
            };
            let k = scope.member(m, n, t.span)?;
            if let Some(s) = structs.get(&k) {
                scope.public(&k, s.public, "struct", t.span)?;
                return Ok(Ty::Struct(k));
            }
            if let Some(e) = enums.get(&k) {
                scope.public(&k, e.public, "enum", t.span)?;
                return Ok(Ty::Enum(k));
            }
            let names = [sorted_names(structs, m), sorted_names(enums, m)].concat();
            Err(err(
                t.span,
                format!("unknown type `{k}`"),
                Some(&suggest(n, &names, "types")),
            ))
        }
        TypeKind::Named(n, args) => match (n.as_str(), args.as_slice()) {
            ("Option", [a]) => Ok(Ty::Option(Box::new(resolve(a, caps, structs, enums, scope)?))),
            ("Result", [a, b]) => {
                let (a, b) = (
                    resolve(a, caps, structs, enums, scope)?,
                    resolve(b, caps, structs, enums, scope)?,
                );
                Ok(Ty::Result(Box::new(a), Box::new(b)))
            }
            ("List", [a]) => Ok(Ty::List(Box::new(resolve(a, caps, structs, enums, scope)?))),
            ("Map", [k, v]) => {
                let key = resolve(k, caps, structs, enums, scope)?;
                if !matches!(key, Ty::Int | Ty::Str) {
                    let msg = format!("`{key}` cannot be a Map key");
                    return Err(err(k.span, msg, Some("Map keys must be Int or Str")));
                }
                let v = resolve(v, caps, structs, enums, scope)?;
                Ok(Ty::Map(Box::new(key), Box::new(v)))
            }
            ("Option" | "Result" | "List" | "Map", _) => {
                let (count, hint) = match n.as_str() {
                    "Option" => (1, "write `Option<T>`"),
                    "Result" => (2, "write `Result<T, E>`"),
                    "Map" => (2, "write `Map<K, V>`"),
                    _ => (1, "write `List<T>`"),
                };
                let msg = format!("`{n}` takes {count} type arguments, found {}", args.len());
                Err(err(t.span, msg, Some(hint)))
            }
            ("Int", []) => Ok(Ty::Int),
            ("Float", []) => Ok(Ty::Float),
            ("Bool", []) => Ok(Ty::Bool),
            ("Str", []) => Ok(Ty::Str),
            ("Bytes", []) => Ok(Ty::Bytes),
            (_, []) if caps.caps.iter().any(|c| c.name == *n) => Ok(Ty::Cap(n.clone())),
            (_, []) => {
                let k = scope.own(caps, n);
                if structs.contains_key(&k) {
                    return Ok(Ty::Struct(k));
                }
                if enums.contains_key(&k) {
                    return Ok(Ty::Enum(k));
                }
                let mut d = unknown_type(t);
                if !scope.module.is_empty() && (structs.contains_key(n) || enums.contains_key(n)) {
                    d.hint = Some(ROOT_ITEM.to_string());
                }
                Err(d)
            }
            _ => Err(unknown_type(t)),
        },
    }
}

fn unknown_type(t: &Type) -> Diag {
    let TypeKind::Named(name, _) = &t.kind else {
        unreachable!("only named types can be unknown");
    };
    // Rust names agents reach for, mapped to the kio type.
    let hint = match name.as_str() {
        "String" | "str" | "char" => "use `Str`".to_string(),
        "Vec" => "use `List<T>`".to_string(),
        "HashMap" | "BTreeMap" => "use `Map<K, V>`".to_string(),
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => "use `Int`".to_string(),
        "f32" | "f64" => "use `Float`".to_string(),
        "bool" => "use `Bool`".to_string(),
        _ => "types are Int, Float, Bool, Str, Bytes, (), Option, Result, List, Map and your structs and enums"
            .to_string(),
    };
    err(t.span, format!("unknown type `{name}`"), Some(&hint))
}

/// Where a type is written, for the type-level half of rule 1.
enum Place<'a> {
    Param,
    Field,
    Payload,
    Local,
    /// The function whose return type it is.
    Return(&'a str),
}

/// The first capability named inside `ty`.
fn cap_in(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Cap(c) => Some(c),
        Ty::Option(t) | Ty::List(t) => cap_in(t),
        Ty::Result(t, e) | Ty::Map(t, e) => cap_in(t).or_else(|| cap_in(e)),
        Ty::Fn(ps, r) => ps.iter().find_map(cap_in).or_else(|| cap_in(r)),
        _ => None,
    }
}

/// Whether a `fn` type inside `ty` mentions a capability.
fn fn_takes_cap(ty: &Ty) -> bool {
    match ty {
        Ty::Fn(..) => cap_in(ty).is_some(),
        Ty::Option(t) | Ty::List(t) => fn_takes_cap(t),
        Ty::Result(t, e) | Ty::Map(t, e) => fn_takes_cap(t) || fn_takes_cap(e),
        _ => false,
    }
}

/// The error for a `fn` type, or a function used as a value, that would carry a capability.
fn fn_cap_err(span: Span) -> Diag {
    err(
        span,
        "function values cannot take or return capabilities".to_string(),
        Some("pass the capability to a named function instead"),
    )
}

/// Rule 1 for types: a capability type is allowed only as the whole type of a parameter.
fn cap_type_ok(ty: &Ty, span: Span, place: Place) -> Result<(), Diag> {
    let Some(cap) = cap_in(ty) else { return Ok(()) };
    if fn_takes_cap(ty) {
        return Err(fn_cap_err(span));
    }
    let hint = "capabilities are parameters only".to_string();
    let nested = (
        format!("capability nested in `{ty}`"),
        format!("take `{cap}` as a parameter"),
    );
    let (msg, hint) = match place {
        Place::Param if matches!(ty, Ty::Cap(_)) => return Ok(()),
        Place::Param => nested,
        Place::Local if !matches!(ty, Ty::Cap(_)) => nested,
        Place::Field => ("capability type in a field".to_string(), hint),
        Place::Payload => ("capability type in a payload".to_string(), hint),
        Place::Local => ("capability type in a local".to_string(), hint),
        Place::Return(f) => (format!("cannot return `{ty}` from `{f}`"), hint),
    };
    Err(err(span, msg, Some(&hint)))
}

fn signature(
    f: &Fn,
    sigs: &HashMap<String, Sig>,
    caps: &CapTable,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
    scope: Scope,
) -> Result<Sig, Diag> {
    if sigs.contains_key(&*key(scope.module, &f.name)) {
        let msg = format!("duplicate function `{}`", f.name);
        return Err(err(f.span, msg, Some("rename one of them")));
    }
    let mut params = Vec::new();
    for (i, p) in f.params.iter().enumerate() {
        not_builtin(&p.name, p.span)?;
        if f.params[..i].iter().any(|q| q.name == p.name) {
            let msg = format!("duplicate parameter `{}`", p.name);
            return Err(err(p.span, msg, Some("rename one of them")));
        }
        let ty = resolve(&p.ty, caps, structs, enums, scope)?;
        cap_type_ok(&ty, p.ty.span, Place::Param)?;
        if scope.module.is_empty() && f.name == "main" && !matches!(ty, Ty::Cap(_)) {
            let msg = format!("`main` parameter `{}` is not a capability", p.name);
            let hint = "`main` parameters must be capabilities, like `input: Dir`";
            return Err(err(p.span, msg, Some(hint)));
        }
        params.push(ty);
    }
    let typed: Vec<_> = f
        .params
        .iter()
        .zip(&params)
        .map(|(p, ty)| format!("{}: {ty}", p.name))
        .collect();
    let ret = resolve(&f.ret, caps, structs, enums, scope)?;
    cap_type_ok(&ret, f.ret.span, Place::Return(&f.name))?;
    Ok(Sig {
        public: f.public,
        decl: format!("fn {}({})", f.name, typed.join(", ")),
        params,
        ret,
    })
}

fn struct_fields(
    s: &Struct,
    caps: &CapTable,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
    scope: Scope,
) -> Result<Vec<(String, Ty)>, Diag> {
    let mut fields: Vec<(String, Ty)> = Vec::new();
    for p in &s.fields {
        if fields.iter().any(|(name, _)| *name == p.name) {
            let msg = format!("duplicate field `{}`", p.name);
            return Err(err(p.span, msg, Some("rename one of them")));
        }
        let ty = resolve(&p.ty, caps, structs, enums, scope)?;
        cap_type_ok(&ty, p.ty.span, Place::Field)?;
        fields.push((p.name.clone(), ty));
    }
    Ok(fields)
}

fn enum_variants(
    e: &Enum,
    caps: &CapTable,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
    scope: Scope,
) -> Result<Vec<(String, Vec<Ty>)>, Diag> {
    let mut variants: Vec<(String, Vec<Ty>)> = Vec::new();
    for v in &e.variants {
        if variants.iter().any(|(name, _)| *name == v.name) {
            let msg = format!("duplicate variant `{}`", v.name);
            return Err(err(v.span, msg, Some("rename one of them")));
        }
        let mut payload = Vec::new();
        for t in &v.fields {
            let ty = resolve(t, caps, structs, enums, scope)?;
            cap_type_ok(&ty, t.span, Place::Payload)?;
            payload.push(ty);
        }
        variants.push((v.name.clone(), payload));
    }
    Ok(variants)
}

/// The index and payload types of variant `v` among `variants`.
pub(crate) fn variant<'a>(variants: &'a [(String, Vec<Ty>)], v: &str) -> Option<(usize, &'a [Ty])> {
    let i = variants.iter().position(|(name, _)| name == v)?;
    Some((i, &variants[i].1))
}

/// The enum's name and variants that `ty` matches on; empty for other types.
pub(crate) fn shape(variants: &Variants, ty: &Ty) -> Shape {
    let some = |t: &Ty| vec![("None".to_string(), vec![]), ("Some".to_string(), vec![t.clone()])];
    match ty {
        Ty::Enum(en) => (en.clone(), variants[en].clone()),
        Ty::Option(t) => ("Option".to_string(), some(t)),
        Ty::Result(t, e) => {
            let ok = vec![
                ("Ok".to_string(), vec![(**t).clone()]),
                ("Err".to_string(), vec![(**e).clone()]),
            ];
            ("Result".to_string(), ok)
        }
        _ => (String::new(), Vec::new()),
    }
}

/// `None`, `Some`, `Ok` and `Err` always mean the constructors, so no binding may take those names.
fn not_builtin(name: &str, span: Span) -> Result<(), Diag> {
    match builtin(name) {
        Some(_) => Err(err(
            span,
            format!("`{name}` is a built-in constructor"),
            Some("choose another name"),
        )),
        None => Ok(()),
    }
}

/// The enum and variant index of the built-in constructors `None`, `Some`, `Ok` and `Err`.
pub(crate) fn builtin(name: &str) -> Option<(&'static str, usize)> {
    match name {
        "None" => Some(("Option", 0)),
        "Some" => Some(("Option", 1)),
        "Ok" => Some(("Result", 0)),
        "Err" => Some(("Result", 1)),
        _ => None,
    }
}

/// The parameter and return types of method `name` on `recv`: a capability's from `caps`, else built in.
fn method(caps: &CapTable, recv: &Ty, name: &str) -> Option<(Vec<Ty>, Ty)> {
    match (recv, name) {
        (Ty::Cap(c), _) => {
            let (_, params, ret) = cap_methods(caps, c).iter().find(|m| m.0 == name)?;
            Some((params.clone(), ret.clone()))
        }
        (Ty::Int, "to_str") | (Ty::Bytes, "to_str") => Some((vec![], Ty::Str)),
        (Ty::Str, "len") | (Ty::Bytes, "len") | (Ty::List(_), "len") => Some((vec![], Ty::Int)),
        (Ty::List(t), "push") => Some((vec![(**t).clone()], Ty::Unit)),
        (Ty::List(t), "contains") => Some((vec![(**t).clone()], Ty::Bool)),
        (Ty::List(_), "sort") => Some((vec![], Ty::Unit)),
        (Ty::List(t), "pop") => Some((vec![], Ty::Option(t.clone()))),
        (Ty::Bytes, "push") => Some((vec![Ty::Int], Ty::Unit)),
        (Ty::Option(t), "unwrap_or") | (Ty::Result(t, _), "unwrap_or") => Some((vec![(**t).clone()], (**t).clone())),
        (Ty::Option(_), "is_some" | "is_none") | (Ty::Result(..), "is_ok" | "is_err") => Some((vec![], Ty::Bool)),
        (Ty::Result(t, _), "ok") => Some((vec![], Ty::Option(t.clone()))),
        (Ty::Result(_, e), "err") => Some((vec![], Ty::Option(e.clone()))),
        (Ty::Map(..), "len") => Some((vec![], Ty::Int)),
        (Ty::Map(k, _), "keys") => Some((vec![], Ty::List(k.clone()))),
        (Ty::Map(k, v), "get") => Some((vec![(**k).clone()], Ty::Option(v.clone()))),
        (Ty::Map(k, v), "insert") => Some((vec![(**k).clone(), (**v).clone()], Ty::Unit)),
        (Ty::Map(k, _), "remove") => Some((vec![(**k).clone()], Ty::Unit)),
        (Ty::Str, "to_bytes") => Some((vec![], Ty::Bytes)),
        (Ty::Str, "split") => Some((vec![Ty::Str], Ty::List(Box::new(Ty::Str)))),
        (Ty::Str, "lines") => Some((vec![], Ty::List(Box::new(Ty::Str)))),
        (Ty::Str, "trim") => Some((vec![], Ty::Str)),
        (Ty::Str, "contains" | "starts_with" | "ends_with") => Some((vec![Ty::Str], Ty::Bool)),
        (Ty::Str, "strip_prefix" | "strip_suffix") => Some((vec![Ty::Str], Ty::Option(Box::new(Ty::Str)))),
        (Ty::Str, "find") => Some((vec![Ty::Str], Ty::Option(Box::new(Ty::Int)))),
        (Ty::Str, "replace") => Some((vec![Ty::Str, Ty::Str], Ty::Str)),
        (Ty::Str, "parse_int") => Some((vec![], Ty::Option(Box::new(Ty::Int)))),
        (Ty::Str, "parse_float") => Some((vec![], Ty::Option(Box::new(Ty::Float)))),
        (Ty::Int, "to_float") => Some((vec![], Ty::Float)),
        (Ty::Float, "to_str") => Some((vec![], Ty::Str)),
        (Ty::Float, "to_int") => Some((vec![], Ty::Option(Box::new(Ty::Int)))),
        (Ty::Float, "abs" | "floor" | "ceil" | "round" | "sqrt") => Some((vec![], Ty::Float)),
        (Ty::Float, "is_nan" | "is_finite" | "is_infinite") => Some((vec![], Ty::Bool)),
        (Ty::Str, "slice") => Some((vec![Ty::Int, Ty::Int], Ty::Str)),
        (Ty::Bytes, "slice") => Some((vec![Ty::Int, Ty::Int], Ty::Bytes)),
        _ => None,
    }
}

/// `Option.ok_or`, whose parameter type is the error type `E` of the result, so it is not in `method`.
fn is_ok_or(recv: &Ty, name: &str) -> bool {
    matches!((recv, name), (Ty::Option(_), "ok_or"))
}

/// The built-in methods that take a function, typed from it by `Checker::fn_method`, since
/// `method` cannot express their result.
fn takes_fn(recv: &Ty, name: &str) -> bool {
    matches!(
        (recv, name),
        (Ty::Option(_), "map" | "and_then")
            | (Ty::Result(..), "map" | "map_err" | "and_then")
            | (Ty::List(_), "map" | "filter" | "any" | "all")
    )
}

/// How a method that takes a function is declared, as a hint; `U` and `F` are the function's result.
fn fn_declared(recv: &Ty, name: &str) -> String {
    let decl = match (recv, name) {
        (Ty::Option(t), "map") => format!("map(fn({t}) -> U) -> Option<U>"),
        (Ty::Option(t), _) => format!("and_then(fn({t}) -> Option<U>) -> Option<U>"),
        (Ty::Result(t, e), "map") => format!("map(fn({t}) -> U) -> Result<U, {e}>"),
        (Ty::Result(t, e), "map_err") => format!("map_err(fn({e}) -> F) -> Result<{t}, F>"),
        (Ty::Result(t, e), _) => format!("and_then(fn({t}) -> Result<U, {e}>) -> Result<U, {e}>"),
        (Ty::List(t), "map") => format!("map(fn({t}) -> U) -> List<U>"),
        (Ty::List(t), "filter") => format!("filter(fn({t}) -> Bool) -> {recv}"),
        (Ty::List(t), _) => format!("{name}(fn({t}) -> Bool) -> Bool"),
        _ => unreachable!("takes_fn"),
    };
    format!("`{recv}.{name}` is declared as `{decl}`")
}

/// How a method is declared, as a hint for a call that does not fit.
fn declared(recv: &Ty, name: &str, params: &[Ty], ret: &Ty) -> String {
    let tys: Vec<_> = params.iter().map(Ty::to_string).collect();
    format!("`{recv}.{name}` is declared as `{name}({}) -> {ret}`", tys.join(", "))
}

/// Every built-in method name, so hints can list the ones `method` accepts on a type.
const METHODS: &[&str] = &[
    "abs",
    "all",
    "and_then",
    "any",
    "ceil",
    "contains",
    "ends_with",
    "err",
    "filter",
    "find",
    "floor",
    "get",
    "insert",
    "is_err",
    "is_finite",
    "is_infinite",
    "is_nan",
    "is_none",
    "is_ok",
    "is_some",
    "keys",
    "len",
    "lines",
    "map",
    "map_err",
    "ok",
    "ok_or",
    "parse_float",
    "parse_int",
    "pop",
    "push",
    "remove",
    "replace",
    "round",
    "slice",
    "sort",
    "split",
    "sqrt",
    "starts_with",
    "strip_prefix",
    "strip_suffix",
    "to_bytes",
    "to_float",
    "to_int",
    "to_str",
    "trim",
    "unwrap_or",
];

fn cap_methods<'a>(caps: &'a CapTable, name: &str) -> &'a [(String, Vec<Ty>, Ty)] {
    caps.caps.iter().find(|c| c.name == name).map_or(&[], |c| &c.methods)
}

fn method_names<'a>(caps: &'a CapTable, recv: &Ty) -> Vec<&'a str> {
    if let Ty::Cap(c) = recv {
        return cap_methods(caps, c).iter().map(|m| m.0.as_str()).collect();
    }
    METHODS
        .iter()
        .copied()
        .filter(|n| method(caps, recv, n).is_some() || is_ok_or(recv, n) || takes_fn(recv, n))
        .collect()
}

/// The user types stored inline in `ty`. `List` and `Map` hold items on the heap, so they end the walk.
fn named(ty: &Ty) -> Vec<&str> {
    match ty {
        Ty::Struct(n) | Ty::Enum(n) => vec![n],
        Ty::Option(t) => named(t),
        Ty::Result(t, e) => [named(t), named(e)].concat(),
        _ => Vec::new(),
    }
}

/// Whether `name` reaches itself through its field or payload types.
fn contains_itself(name: &str, fields: &Fields, variants: &Variants) -> bool {
    let mut todo = vec![name];
    let mut seen = HashSet::new();
    while let Some(n) = todo.pop() {
        let of_struct = fields.get(n).into_iter().flatten().map(|(_, ty)| ty);
        let of_enum = variants.get(n).into_iter().flatten().flat_map(|(_, tys)| tys);
        for m in of_struct.chain(of_enum).flat_map(named) {
            if m == name {
                return true;
            }
            if seen.insert(m) {
                todo.push(m);
            }
        }
    }
    false
}

fn no_field(span: Span, field: &str, ty: &str, decl: &[(String, Ty)]) -> Diag {
    let names: Vec<_> = decl.iter().map(|(n, _)| n.as_str()).collect();
    let hint = suggest(field, &names, "fields");
    err(span, format!("no field `{field}` on {ty}"), Some(&hint))
}

/// Whether a value of `ty` can hold a function. `seen` holds the user types already walked.
fn has_fn<'a>(ty: &'a Ty, fields: &'a Fields, variants: &'a Variants, seen: &mut HashSet<&'a str>) -> bool {
    let mut walk = |t| has_fn(t, fields, variants, seen);
    match ty {
        Ty::Fn(..) => true,
        Ty::Option(t) | Ty::List(t) => walk(t),
        Ty::Result(t, e) | Ty::Map(t, e) => walk(t) || walk(e),
        Ty::Struct(n) => seen.insert(n) && fields[n].iter().any(|(_, t)| has_fn(t, fields, variants, seen)),
        Ty::Enum(n) => {
            seen.insert(n)
                && variants[n]
                    .iter()
                    .flat_map(|(_, ts)| ts)
                    .any(|t| has_fn(t, fields, variants, seen))
        }
        _ => false,
    }
}

fn no_fn_equality(span: Span, ty: &Ty) -> Diag {
    err(
        span,
        format!("cannot compare `{ty}` values"),
        Some("functions have no equality"),
    )
}

/// At most this many diagnostics are reported, so a hostile source cannot make output unbounded.
pub const MAX_DIAGS: usize = 100;

/// Checks `src` as a one-file program.
pub fn check(src: &str, caps: &CapTable) -> Result<Program, Vec<Diag>> {
    check_files(&[("main", src)], caps)
}

/// Checks a program of `(name, source)` files. `files[0]` is the root; every other file is the
/// module named by its name without `.kio`, which the root must declare once with `mod name;`.
/// Reports at most one `Diag` per function, in source order.
pub fn check_files(files: &[(&str, &str)], caps: &CapTable) -> Result<Program, Vec<Diag>> {
    check_all(files, caps).map_err(|mut diags| {
        if diags.len() > MAX_DIAGS {
            let more = diags.len() - MAX_DIAGS;
            diags.truncate(MAX_DIAGS);
            let msg = format!("{more} more errors not shown");
            diags.push(err(Span::default(), msg, Some("fix the errors above first")));
        }
        diags
    })
}

/// Checks that `mod` appears only in the root and that modules and files match one to one.
fn declared_modules(names: &[&str], parsed: &[Vec<Item>]) -> Result<HashSet<String>, Vec<Diag>> {
    let root = names[0];
    let mut mods = HashSet::new();
    let mut diags = Vec::new();
    for (i, items) in parsed.iter().enumerate() {
        for item in items {
            let Item::Mod(m, span) = item else { continue };
            if i != 0 {
                let msg = "`mod` is only allowed in the root file".to_string();
                diags.push(err(*span, msg, Some(&format!("declare `mod {m};` in {root}.kio"))));
            } else if !names[1..].contains(&m.as_str()) {
                let hint = format!("add a file named {m}.kio");
                diags.push(err(*span, format!("unknown module `{m}`"), Some(&hint)));
            } else if !mods.insert(m.clone()) {
                let msg = format!("duplicate `mod {m};`");
                diags.push(err(*span, msg, Some("declare each module once")));
            }
        }
    }
    for (i, n) in names.iter().enumerate().skip(1) {
        let at = Span {
            file: i as u32,
            ..Span::default()
        };
        if names[1..i].contains(n) {
            let msg = format!("duplicate file for module `{n}`");
            diags.push(err(at, msg, Some("give each file its own name")));
        } else if !mods.contains(*n) {
            let msg = format!("module `{n}` is never declared");
            diags.push(err(at, msg, Some(&format!("add `mod {n};` to {root}.kio"))));
        }
    }
    if diags.is_empty() {
        Ok(mods)
    } else {
        Err(diags)
    }
}

fn check_all(files: &[(&str, &str)], caps: &CapTable) -> Result<Program, Vec<Diag>> {
    let names: Vec<&str> = files.iter().map(|(n, _)| n.strip_suffix(".kio").unwrap_or(n)).collect();
    let mut parsed = Vec::new();
    let mut diags = Vec::new();
    for (i, (_, src)) in files.iter().enumerate() {
        match parse_file(src, i as u32) {
            Ok(items) => parsed.push(items),
            Err(d) => diags.push(d),
        }
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    let mods = declared_modules(&names, &parsed)?;

    let mut fns = Vec::new();
    let mut structs = HashMap::new();
    let mut enums = HashMap::new();
    let mut order = Vec::new();
    for e in &caps.enums {
        order.push(e.name.clone());
        enums.insert(e.name.clone(), e.clone());
    }
    for (i, items) in parsed.into_iter().enumerate() {
        let module = if i == 0 { "" } else { names[i] };
        for item in items {
            let declared = match &item {
                Item::Struct(s) => Some((s.name.as_str(), s.span)),
                Item::Enum(e) => Some((e.name.as_str(), e.span)),
                Item::Fn(_) | Item::Mod(..) => None,
            };
            if let Some((name, span)) = declared.filter(|(n, _)| reserved(caps, n)) {
                let msg = format!("`{name}` is a built-in type");
                diags.push(err(span, msg, Some("rename your type")));
                continue;
            }
            // `web::x` means module `web`, so a lowercase type name would be ambiguous in a path.
            if let Some((name, span)) = declared.filter(|(n, _)| !n.starts_with(|c: char| c.is_ascii_uppercase())) {
                let mut fixed = name.to_string();
                fixed[..1].make_ascii_uppercase();
                let msg = format!("type names start with an uppercase letter: `{name}`");
                diags.push(err(span, msg, Some(&format!("rename it `{fixed}`"))));
                continue;
            }
            match item {
                Item::Mod(..) => {}
                Item::Fn(f) => fns.push((module, f)),
                Item::Enum(e) => {
                    let k = key(module, &e.name).into_owned();
                    if structs.contains_key(&k) || enums.contains_key(&k) {
                        let msg = format!("duplicate type `{}`", e.name);
                        diags.push(err(e.span, msg, Some("rename one of them")));
                    } else {
                        order.push(k.clone());
                        enums.insert(k, e);
                    }
                }
                Item::Struct(s) => {
                    let k = key(module, &s.name).into_owned();
                    if structs.contains_key(&k) || enums.contains_key(&k) {
                        let msg = format!("duplicate type `{}`", s.name);
                        diags.push(err(s.span, msg, Some("rename one of them")));
                    } else {
                        order.push(k.clone());
                        structs.insert(k, s);
                    }
                }
            }
        }
    }

    // Types before signatures, which may name them.
    let mut fields = Fields::new();
    let mut variants = Variants::new();
    for name in &order {
        let scope = Scope {
            module: module_of(name),
            mods: &mods,
        };
        let resolved = match structs.get(name) {
            Some(s) => struct_fields(s, caps, &structs, &enums, scope).map(|f| drop(fields.insert(name.clone(), f))),
            None => enum_variants(&enums[name], caps, &structs, &enums, scope)
                .map(|v| drop(variants.insert(name.clone(), v))),
        };
        diags.extend(resolved.err());
    }
    for name in &order {
        if contains_itself(name, &fields, &variants) {
            let (kind, span) = match structs.get(name) {
                Some(s) => ("struct", s.span),
                None => ("enum", enums[name].span),
            };
            let msg = format!("{kind} `{name}` contains itself");
            diags.push(err(span, msg, Some("use indexes into a `List` instead")));
        }
    }
    if !diags.is_empty() {
        return Err(diags);
    }

    // Signatures first, so call order in the file does not matter.
    let mut sigs = HashMap::new();
    let mut sig_errs = Vec::new();
    for (module, f) in &fns {
        let scope = Scope { module, mods: &mods };
        match signature(f, &sigs, caps, &structs, &enums, scope) {
            Ok(sig) => {
                sigs.insert(key(module, &f.name).into_owned(), sig);
                sig_errs.push(None);
            }
            Err(d) => sig_errs.push(Some(d)),
        }
    }

    // `main`'s capability parameters, so a helper that uses one it was not given gets a hint.
    let main_caps: Vec<(String, String)> = (fns.iter())
        .filter(|(m, f)| m.is_empty() && f.name == "main")
        .flat_map(|(_, f)| f.params.iter())
        .filter_map(|p| match &p.ty.kind {
            TypeKind::Named(t, _) if caps.caps.iter().any(|c| c.name == *t) => Some((p.name.clone(), t.clone())),
            _ => None,
        })
        .collect();

    let match_budget = Cell::new(PROGRAM_MATCH_BUDGET);
    let mut types = HashMap::new();
    let mut captures = HashMap::new();
    for ((module, f), sig_err) in fns.iter().zip(sig_errs) {
        let mut c = Checker {
            caps,
            sigs: &sigs,
            fields: &fields,
            variants: &variants,
            structs: &structs,
            enums: &enums,
            home: Scope { module, mods: &mods },
            scopes: Vec::new(),
            ret: Ty::Unit,
            ret_open: false,
            try_shape: None,
            loops: Vec::new(),
            closures: Vec::new(),
            main_caps: &main_caps,
            match_budget: &match_budget,
            types: HashMap::new(),
            captures: HashMap::new(),
        };
        diags.extend(sig_err.or_else(|| c.function(f).err()));
        types.extend(c.types);
        captures.extend(c.captures);
    }
    if !fns.iter().any(|(m, f)| m.is_empty() && f.name == "main") {
        let hint = "add `fn main() { .. }`";
        diags.push(err(Span::default(), "no `main` function".to_string(), Some(hint)));
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    let mut p = Program {
        funcs: fns
            .into_iter()
            .map(|(m, f)| (key(m, &f.name).into_owned(), f))
            .collect(),
        structs,
        enums,
        variants,
        fields,
        sigs,
        types,
        captures,
        cap_methods: (caps.caps.iter())
            .flat_map(|c| {
                c.methods
                    .iter()
                    .map(|(m, _, ret)| ((c.name.clone(), m.clone()), ret.clone()))
            })
            .collect(),
        // Replaced below: lowering reads the rest of the program.
        ir: ir::Module {
            funcs: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            caps: Vec::new(),
            main: ir::FuncId(0),
        },
    };
    p.ir = crate::lower::lower(&p, caps);
    Ok(p)
}

struct Checker<'a> {
    caps: &'a CapTable,
    sigs: &'a HashMap<String, Sig>,
    fields: &'a Fields,
    variants: &'a Variants,
    structs: &'a HashMap<String, Struct>,
    enums: &'a HashMap<String, Enum>,
    /// The module being checked.
    home: Scope<'a>,
    scopes: Vec<HashMap<String, Local>>,
    /// Return type of the function or closure being checked. For a closure with no expected
    /// type (`ret_open`), `Never` until a `return` sets it.
    ret: Ty,
    ret_open: bool,
    /// While `ret_open`, what the closure must return as an `and_then` argument: `Option<!>`
    /// or `Result<!, E>`, so `?` inside it can be checked before the return type is known.
    try_shape: Option<Ty>,
    /// Loops enclosing the statement being checked, inside the innermost function or closure.
    /// One entry per enclosing loop: whether a `break` has been seen in it.
    loops: Vec<bool>,
    /// Closures enclosing the expression being checked, innermost last.
    closures: Vec<ClosureFrame>,
    /// Name and type of each capability parameter of `main`.
    main_caps: &'a [(String, String)],
    /// What is left of `PROGRAM_MATCH_BUDGET` for this program.
    match_budget: &'a Cell<usize>,
    /// As `Program::types`.
    types: HashMap<Span, Ty>,
    /// As `Program::captures`.
    captures: HashMap<Span, Vec<String>>,
}

impl<'a> Checker<'a> {
    fn function(&mut self, f: &Fn) -> Result<(), Diag> {
        let sig = &self.sigs[&*key(self.home.module, &f.name)];
        let params = f.params.iter().zip(&sig.params);
        let params = params.map(|(p, ty)| {
            (
                p.name.clone(),
                Local {
                    ty: ty.clone(),
                    mutable: false,
                },
            )
        });
        self.scopes = vec![params.collect()];
        self.ret = sig.ret.clone();
        let found = self.block(&f.body, Some(&sig.ret))?;
        if f.body.tail.is_none() && found == Ty::Unit && sig.ret != Ty::Unit {
            let msg = format!("missing return: `{}` must return {}", f.name, sig.ret);
            let ends_in_expr = f.body.stmts.last().is_some_and(|s| matches!(s.kind, StmtKind::Expr(_)));
            let hint = if ends_in_expr {
                "remove the trailing `;` to return the value"
            } else {
                "end the body with a value or `return`"
            };
            return Err(err(f.body.span, msg, Some(hint)));
        }
        match &f.body.tail {
            Some(t) => want_expr(t, &found, &sig.ret),
            None => want(f.body.span, &found, &sig.ret),
        }
    }

    fn unknown_name(&self, span: Span, name: &str) -> Diag {
        let mut names: Vec<_> = self.scopes.iter().flat_map(|s| s.keys().map(String::as_str)).collect();
        names.sort();
        let main_cap = self.main_caps.iter().find(|(n, _)| n == name);
        let hint = match closest(name, &names) {
            _ if self.is_cap(name) => NO_CAP_SYNTAX.to_string(),
            _ if main_cap.is_some() => {
                let (n, t) = main_cap.unwrap();
                format!("`{n}` is a capability of `main`; take it as a parameter, `{n}: {t}`, and pass it in")
            }
            Some(c) => format!("did you mean `{c}`?"),
            None => "declare it with `let`".to_string(),
        };
        err(span, format!("unknown name `{name}`"), Some(&hint))
    }

    fn is_cap(&self, name: &str) -> bool {
        self.caps.caps.iter().any(|c| c.name == name)
    }

    fn unknown_variant(&self, span: Span, en: &str, v: &str) -> Diag {
        // `Http::get(..)` or `Dir::open(..)`: the ambient access other languages give, which kio has none of.
        if self.is_cap(en) {
            let msg = format!("no ambient {en} access: `{en}::{v}` does not exist");
            return err(span, msg, Some(NO_CAP_SYNTAX));
        }
        let hint = match (en, self.variants.get(en)) {
            ("Option", _) => suggest(v, &["None", "Some"], "variants"),
            ("Result", _) => suggest(v, &["Ok", "Err"], "variants"),
            ("Map", _) => "the only Map function is `Map::new()`".to_string(),
            (_, Some(vs)) => suggest(v, &vs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), "variants"),
            _ if !self.home.module.is_empty() && self.enums.contains_key(bare(en)) => ROOT_ITEM.to_string(),
            _ => suggest(bare(en), &sorted_names(self.enums, module_of(en)), "enums"),
        };
        err(span, format!("unknown variant `{en}::{v}`"), Some(&hint))
    }

    /// The local `name`, used at `span`. A local declared outside a closure being checked is
    /// recorded as a capture of that closure; a capability cannot be one. Every use of a local
    /// goes through here, so no path gives a closure a capability.
    fn lookup(&mut self, name: &str, span: Span) -> Result<Option<&mut Local>, Diag> {
        let Some(i) = self.scopes.iter().rposition(|s| s.contains_key(name)) else {
            return Ok(None);
        };
        let mut captured = false;
        for c in self.closures.iter_mut().filter(|c| i < c.base) {
            captured = true;
            if !c.captures.iter().any(|n| n == name) {
                c.captures.push(name.to_string());
            }
        }
        if let (true, Ty::Cap(ty)) = (captured, &self.scopes[i][name].ty) {
            let hint = format!("take it as a parameter: `|{name}: {ty}, ..|`");
            return Err(err(span, format!("closure captures capability `{name}`"), Some(&hint)));
        }
        Ok(self.scopes[i].get_mut(name))
    }

    /// Whether `name` is a capture of the innermost closure being checked.
    fn is_capture(&self, name: &str) -> bool {
        let found = self.scopes.iter().rposition(|s| s.contains_key(name));
        matches!((found, self.closures.last()), (Some(i), Some(c)) if i < c.base)
    }

    /// The error for changing capture `name` at `span`; `verb` is `assigned` or `changed`.
    fn capture_changed(span: Span, name: &str, verb: &str) -> Diag {
        let msg = format!("`{name}` is captured by copy and cannot be {verb}");
        err(span, msg, Some("return the new value instead"))
    }

    fn block(&mut self, b: &Block, expected: Option<&Ty>) -> Result<Ty, Diag> {
        self.scopes.push(HashMap::new());
        let mut last = Ty::Unit;
        for s in &b.stmts {
            last = self.stmt(s)?;
        }
        let ty = match &b.tail {
            Some(t) => self.expr(t, expected)?,
            None => last,
        };
        self.scopes.pop();
        Ok(ty)
    }

    /// `Never` for a statement that always returns, otherwise `()`.
    fn stmt(&mut self, s: &Stmt) -> Result<Ty, Diag> {
        match &s.kind {
            StmtKind::Let {
                name,
                mutable,
                ty,
                init,
            } => {
                not_builtin(name, s.span)?;
                let declared = ty
                    .as_ref()
                    .map(|t| resolve(t, self.caps, self.structs, self.enums, self.home));
                let declared = declared.transpose()?;
                if let ExprKind::Name(from) = &init.kind {
                    if let Some(Local { ty: Ty::Cap(_), .. }) = self.lookup(from, init.span)? {
                        let msg = format!("capability `{from}` bound to local `{name}`");
                        return Err(err(init.span, msg, Some(&format!("pass `{from}` directly instead"))));
                    }
                }
                if let (Some(d), Some(t)) = (&declared, ty) {
                    cap_type_ok(d, t.span, Place::Local)?;
                }
                let mut found = self.expr(init, declared.as_ref())?;
                if let Some(declared) = declared {
                    want_expr(init, &found, &declared)?;
                    found = declared;
                } else if found == Ty::Never {
                    found = Ty::Unit;
                }
                self.types.insert(s.span, found.clone());
                let local = Local {
                    ty: found,
                    mutable: *mutable,
                };
                self.scopes.last_mut().unwrap().insert(name.clone(), local);
                Ok(Ty::Unit)
            }
            StmtKind::Assign { target, op, value } => {
                let ty = self.place(target)?;
                let found = self.expr(value, (*op == "=").then_some(&ty))?;
                if *op == "=" {
                    want_expr(value, &found, &ty)?;
                } else if binary_ty(&op[..1], &ty, &found) != Some(ty.clone()) {
                    return Err(binary_err(&op[..1], op, &ty, &found, s.span));
                }
                Ok(Ty::Unit)
            }
            StmtKind::Return(value) if self.ret_open => {
                // A closure with no expected type: the first `return` sets its return type.
                let found = match value {
                    Some(e) => self.expr(e, None)?,
                    None => Ty::Unit,
                };
                if self.ret == Ty::Never {
                    self.ret = found;
                } else {
                    let ret = self.ret.clone();
                    match value {
                        Some(e) => want_expr(e, &found, &ret)?,
                        None => want(s.span, &found, &ret)?,
                    }
                }
                Ok(Ty::Never)
            }
            StmtKind::Return(value) => {
                let ret = self.ret.clone();
                match value {
                    Some(e) => want_expr(e, &self.expr(e, Some(&ret))?, &ret)?,
                    None => want(s.span, &Ty::Unit, &ret)?,
                }
                Ok(Ty::Never)
            }
            StmtKind::While { cond, body } => {
                want_expr(cond, &self.expr(cond, None)?, &Ty::Bool)?;
                self.loops.push(false);
                let found = self.block(body, None);
                self.loops.pop();
                want(body.span, &found?, &Ty::Unit)?;
                Ok(Ty::Unit)
            }
            StmtKind::Loop { body } => {
                self.loops.push(false);
                let found = self.block(body, None);
                let broke = self.loops.pop() == Some(true);
                want(body.span, &found?, &Ty::Unit)?;
                Ok(if broke { Ty::Unit } else { Ty::Never })
            }
            StmtKind::For { name, iter, body } => {
                not_builtin(name, s.span)?;
                let ty = match iter {
                    Iter::List(e) => match self.expr(e, None)? {
                        Ty::List(t) => *t,
                        found => {
                            let hint = "`for` runs over a List or a range `a..b`";
                            return Err(err(e.span, format!("cannot iterate over {found}"), Some(hint)));
                        }
                    },
                    Iter::Range(a, b) => {
                        want_expr(a, &self.expr(a, None)?, &Ty::Int)?;
                        want_expr(b, &self.expr(b, None)?, &Ty::Int)?;
                        Ty::Int
                    }
                };
                self.scopes
                    .push(HashMap::from([(name.clone(), Local { ty, mutable: false })]));
                self.loops.push(false);
                let found = self.block(body, None);
                self.loops.pop();
                self.scopes.pop();
                want(body.span, &found?, &Ty::Unit)?;
                Ok(Ty::Unit)
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.loops.is_empty() {
                    let word = if matches!(s.kind, StmtKind::Break) {
                        "break"
                    } else {
                        "continue"
                    };
                    if self.closures.last().is_some_and(|c| c.in_loop) {
                        let msg = format!("`{word}` cannot leave a closure");
                        return Err(err(s.span, msg, Some("return from the closure instead")));
                    }
                    let hint = "use `return` to leave a function";
                    return Err(err(s.span, format!("`{word}` outside a loop"), Some(hint)));
                }
                if matches!(s.kind, StmtKind::Break) {
                    *self.loops.last_mut().unwrap() = true;
                }
                Ok(Ty::Never)
            }
            StmtKind::Expr(e) => Ok(match self.expr(e, None)? {
                Ty::Never => Ty::Never,
                _ => Ty::Unit,
            }),
        }
    }

    fn expr(&mut self, e: &Expr, expected: Option<&Ty>) -> Result<Ty, Diag> {
        let ty = self.expr_kind(e, expected)?;
        self.record(e, &ty);
        Ok(ty)
    }

    fn record(&mut self, e: &Expr, ty: &Ty) {
        // Lowering reads types back by span, so two expressions sharing one would give one of
        // them the wrong type: refuse in every build, not only debug ones.
        if self.types.insert(e.span, ty.clone()).is_some() {
            panic!("two expressions at {:?}", e.span);
        }
    }

    fn expr_kind(&mut self, e: &Expr, expected: Option<&Ty>) -> Result<Ty, Diag> {
        match &e.kind {
            ExprKind::Int(_) => Ok(Ty::Int),
            ExprKind::Float(_) => Ok(Ty::Float),
            ExprKind::Str(_) => Ok(Ty::Str),
            ExprKind::Bool(_) => Ok(Ty::Bool),
            ExprKind::Unit => Ok(Ty::Unit),
            ExprKind::Name(name) if builtin(name).is_some() => self.constructor(e, name, &[], expected),
            ExprKind::Name(name) => match self.lookup(name, e.span)? {
                Some(Local { ty: Ty::Cap(_), .. }) => {
                    let hint = "call methods on the parameter itself";
                    Err(err(e.span, "capability used as a value".to_string(), Some(hint)))
                }
                Some(local) => Ok(local.ty.clone()),
                None => {
                    let k = key(self.home.module, name);
                    if self.sigs.contains_key(&*k) {
                        return self.fn_value(e.span, &k);
                    }
                    let mut d = self.unknown_name(e.span, name);
                    if !self.home.module.is_empty() && self.sigs.contains_key(name.as_str()) {
                        d.hint = Some(ROOT_ITEM.to_string());
                    }
                    Err(d)
                }
            },
            ExprKind::Unary(op, x) => {
                let found = self.expr(x, None)?;
                if *op == "-" && found == Ty::Float {
                    return Ok(Ty::Float);
                }
                let want_ty = if *op == "-" { Ty::Int } else { Ty::Bool };
                want_expr(x, &found, &want_ty)?;
                Ok(want_ty)
            }
            ExprKind::Binary(op, a, b) => {
                if matches!(*op, "==" | "!=") {
                    // A capability name is not a value, so catch it before `expr` says so.
                    for x in [a, b] {
                        if let ExprKind::Name(n) = &x.kind {
                            if let Some(Local {
                                ty: ty @ Ty::Cap(_), ..
                            }) = self.lookup(n, x.span)?
                            {
                                let hint = Some("capabilities have no equality");
                                return Err(err(e.span, format!("cannot compare `{ty}` values"), hint));
                            }
                        }
                    }
                }
                let (l, r) = (self.expr(a, None)?, self.expr(b, None)?);
                if matches!(*op, "==" | "!=") && l == r && self.has_fn(&l) {
                    return Err(no_fn_equality(e.span, &l));
                }
                binary_ty(op, &l, &r).ok_or_else(|| binary_err(op, op, &l, &r, e.span))
            }
            ExprKind::If(cond, then, els) => {
                want_expr(cond, &self.expr(cond, None)?, &Ty::Bool)?;
                let t = self.block(then, expected)?;
                self.else_branch(e, t, then, els.as_deref(), expected)
            }
            ExprKind::IfLet(pat, scrut, then, els) => {
                let st = self.scrutinee(scrut)?;
                self.bind_pattern(pat, &st)?;
                let t = self.block(then, expected)?;
                self.scopes.pop();
                self.else_branch(e, t, then, els.as_deref(), expected)
            }
            ExprKind::Block(b) => self.block(b, expected),
            ExprKind::Call(callee, args) => self.call(e, callee, args, expected),
            ExprKind::Method(recv, name, args) => self.method_call(e, recv, name, args),
            ExprKind::Field(base, name) => {
                let ty = self.expr(base, None)?;
                self.field(&ty, name, e.span)
            }
            ExprKind::Index(base, index) => {
                let ty = self.expr(base, None)?;
                self.element(&ty, index, e.span)
            }
            ExprKind::List(items) => self.list_lit(e, items, expected),
            ExprKind::StructLit(name, inits) => self.struct_lit(e, &key(self.home.module, name), inits),
            ExprKind::Path(en, v) => self.variant_value(e, &self.enum_key(en), v, &[]),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::StructLit(name, inits) => {
                    let k = self.home.member(m, name, e.span)?;
                    if let Some(s) = self.structs.get(&k) {
                        self.home.public(&k, s.public, "struct", e.span)?;
                    }
                    self.struct_lit(e, &k, inits)
                }
                ExprKind::Path(en, v) => self.module_variant(e, m, en, v, &[]),
                ExprKind::Name(name) => {
                    let (k, _) = self.named_fn(e.span, name, Some(m))?;
                    self.fn_value(e.span, &k)
                }
                _ => Err(nested_path(e.span)),
            },
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms, expected),
            ExprKind::Try(x) => self.try_expr(e, x),
            ExprKind::Closure(params, body) => self.closure(e, params, body, expected),
        }
    }

    /// Like `expr`, but a capability parameter is allowed: the one place rule 1 lets it appear.
    fn operand(&mut self, e: &Expr, expected: Option<&Ty>) -> Result<Ty, Diag> {
        if let ExprKind::Name(name) = &e.kind {
            if builtin(name).is_none() {
                if let Some(Local {
                    ty: ty @ Ty::Cap(_), ..
                }) = self.lookup(name, e.span)?
                {
                    let ty = ty.clone();
                    self.types.insert(e.span, ty.clone());
                    return Ok(ty);
                }
            }
        }
        self.expr(e, expected)
    }

    /// The element type of list or bytes type `ty`, with `index` checked as an `Int`.
    fn element(&mut self, ty: &Ty, index: &Expr, span: Span) -> Result<Ty, Diag> {
        let elem = match ty {
            Ty::List(t) => (**t).clone(),
            Ty::Bytes => Ty::Int,
            _ => {
                let hint = "only a List or Bytes can be indexed";
                return Err(err(span, format!("cannot index {ty}"), Some(hint)));
            }
        };
        want_expr(index, &self.expr(index, None)?, &Ty::Int)?;
        Ok(elem)
    }

    /// `[a, b]`: the elements share the expected element type, else the first one's.
    fn list_lit(&mut self, e: &Expr, items: &[Expr], expected: Option<&Ty>) -> Result<Ty, Diag> {
        let mut elem = match expected {
            Some(Ty::List(t)) => Some((**t).clone()),
            _ => None,
        };
        for item in items {
            let found = self.expr(item, elem.as_ref())?;
            match &elem {
                Some(t) => want_expr(item, &found, t)?,
                None => elem = Some(found),
            }
        }
        match elem {
            Some(t) => Ok(Ty::List(Box::new(t))),
            None => {
                let hint = "add a type: `let xs: List<Int> = [];`";
                Err(err(e.span, "cannot tell the type of `[]`".to_string(), Some(hint)))
            }
        }
    }

    fn method_call(&mut self, e: &Expr, recv: &Expr, name: &str, args: &[Expr]) -> Result<Ty, Diag> {
        let recv_ty = self.operand(recv, None)?;
        if matches!(
            (&recv_ty, name),
            (Ty::List(_), "push" | "sort" | "pop") | (Ty::Bytes, "push") | (Ty::Map(..), "insert" | "remove")
        ) {
            self.mutation_target(recv, name, &recv_ty)?;
        }
        if let (Ty::Option(t), true) = (&recv_ty, is_ok_or(&recv_ty, name)) {
            if args.len() != 1 {
                let msg = format!("`ok_or` takes 1 arguments, found {}", args.len());
                return Err(err(
                    e.span,
                    msg,
                    Some("`Option<T>.ok_or` is declared as `ok_or(E) -> Result<T, E>`"),
                ));
            }
            // `E` is the argument's own type, as `Some(e)` takes its type from `e`.
            let found = self.expr(&args[0], None)?;
            return Ok(Ty::Result(t.clone(), Box::new(found)));
        }
        if takes_fn(&recv_ty, name) {
            return self.fn_method(e, &recv_ty, name, args);
        }
        let Some((params, ret)) = method(self.caps, &recv_ty, name) else {
            let names = method_names(self.caps, &recv_ty);
            let fn_field = match &recv_ty {
                Ty::Struct(s) => self.fields[s].iter().any(|(f, t)| f == name && matches!(t, Ty::Fn(..))),
                _ => false,
            };
            let hint = if fn_field {
                let r = match &recv.kind {
                    ExprKind::Name(r) => r.as_str(),
                    _ => "..",
                };
                format!("`{name}` is a field; call it as `({r}.{name})(..)`")
            } else if matches!((&recv_ty, name), (Ty::List(_), "iter" | "into_iter")) {
                "a List has no iterators; loop over it with `for x in xs { .. }`".to_string()
            } else if names.is_empty() {
                format!("{recv_ty} has no methods")
            } else {
                suggest(name, &names, &format!("{recv_ty} has"))
            };
            return Err(err(e.span, format!("no method `{name}` on {recv_ty}"), Some(&hint)));
        };
        if args.len() != params.len() {
            let msg = format!("`{name}` takes {} arguments, found {}", params.len(), args.len());
            return Err(err(e.span, msg, Some(&declared(&recv_ty, name, &params, &ret))));
        }
        if let (Ty::List(t), "contains") = (&recv_ty, name) {
            if self.has_fn(t) {
                return Err(no_fn_equality(e.span, t));
            }
        }
        if let (Ty::List(t), "sort") = (&recv_ty, name) {
            if !matches!(**t, Ty::Int | Ty::Str) {
                let msg = format!("cannot sort {recv_ty}");
                return Err(err(e.span, msg, Some("sort works on List<Int> and List<Str>")));
            }
        }
        for (a, param) in args.iter().zip(&params) {
            let found = self.expr(a, Some(param))?;
            want_expr(a, &found, param).map_err(|mut d| {
                if !matches!(recv_ty, Ty::Cap(_)) {
                    d.hint.get_or_insert_with(|| declared(&recv_ty, name, &params, &ret));
                }
                d
            })?;
        }
        Ok(ret)
    }

    /// A built-in method that takes a function (`takes_fn`), typed from the function's type. A
    /// closure written as the argument takes its parameter type from the receiver and its
    /// return type from its body.
    fn fn_method(&mut self, e: &Expr, recv_ty: &Ty, name: &str, args: &[Expr]) -> Result<Ty, Diag> {
        let decl = fn_declared(recv_ty, name);
        if args.len() != 1 {
            let msg = format!("`{name}` takes 1 arguments, found {}", args.len());
            return Err(err(e.span, msg, Some(&decl)));
        }
        let param = match (recv_ty, name) {
            (Ty::Result(_, err_ty), "map_err") => (**err_ty).clone(),
            (Ty::Option(t) | Ty::Result(t, _) | Ty::List(t), _) => (**t).clone(),
            _ => unreachable!("takes_fn"),
        };
        let shape = match (recv_ty, name) {
            (Ty::Option(_), "and_then") => Some(Ty::Option(Box::new(Ty::Never))),
            (Ty::Result(_, err_ty), "and_then") => Some(Ty::Result(Box::new(Ty::Never), err_ty.clone())),
            _ => None,
        };
        let f = &args[0];
        let found = match &f.kind {
            ExprKind::Closure(ps, body) => {
                if ps.len() != 1 {
                    let msg = format!(
                        "`{name}` takes a function of 1 parameter, found a closure of {} parameters",
                        ps.len()
                    );
                    return Err(err(f.span, msg, Some(&decl)));
                }
                let want = (std::slice::from_ref(&param), None);
                let ty = self.closure_with(f, ps, body, Some(want), shape, None)?;
                self.record(f, &ty);
                ty
            }
            _ => self.expr(f, None)?,
        };
        let Ty::Fn(ps, ret) = &found else {
            let msg = format!("`{name}` takes a function, found {found}");
            return Err(err(f.span, msg, Some(&decl)));
        };
        if ps.len() != 1 {
            let msg = format!("`{name}` takes a function of 1 parameter, found {found}");
            return Err(err(f.span, msg, Some(&decl)));
        }
        if ps[0] != param {
            let msg = format!("`{name}` needs a function taking {param}, found {found}");
            return Err(err(f.span, msg, Some(&decl)));
        }
        let ret = ret.clone();
        let wrong_ret = |what: String| {
            let msg = format!("`{name}` needs a function returning {what}, found {found}");
            Err(err(f.span, msg, Some(&decl)))
        };
        match (recv_ty, name) {
            (_, "filter" | "any" | "all") if *ret != Ty::Bool => wrong_ret("Bool".to_string()),
            (Ty::List(_), "filter") => Ok(recv_ty.clone()),
            (_, "any" | "all") => Ok(Ty::Bool),
            (Ty::List(_), "map") => Ok(Ty::List(ret)),
            (Ty::Option(_), "map") => Ok(Ty::Option(ret)),
            (Ty::Result(_, err_ty), "map") => Ok(Ty::Result(ret, err_ty.clone())),
            (Ty::Result(t, _), "map_err") => Ok(Ty::Result(t.clone(), ret)),
            (Ty::Option(_), _) => match *ret {
                Ty::Option(_) => Ok(*ret),
                _ => wrong_ret("Option<..>".to_string()),
            },
            (Ty::Result(_, err_ty), _) => match &*ret {
                Ty::Result(_, e2) if e2 == err_ty => Ok(*ret),
                _ => wrong_ret(format!("Result<.., {err_ty}>")),
            },
            _ => unreachable!("takes_fn"),
        }
    }

    /// `None`, `Some(x)`, `Ok(x)` or `Err(x)`; all but `Some` take their type from `expected`.
    fn constructor(&mut self, e: &Expr, name: &str, args: &[Expr], expected: Option<&Ty>) -> Result<Ty, Diag> {
        let arity = usize::from(name != "None");
        if args.len() != arity {
            let msg = format!("`{name}` takes {arity} arguments, found {}", args.len());
            let hint = if arity == 1 {
                format!("write `{name}(value)`")
            } else {
                "write `None` without arguments".to_string()
            };
            return Err(err(e.span, msg, Some(&hint)));
        }
        // In a closure given to `Result.and_then`, `Ok(x)` takes its error type from the receiver.
        if let ("Ok", None, true, Some(Ty::Result(_, err_ty))) = (name, expected, self.ret_open, &self.try_shape) {
            let err_ty = err_ty.clone();
            let t = self.expr(&args[0], None)?;
            return Ok(Ty::Result(Box::new(t), err_ty));
        }
        let payload = match (name, expected) {
            ("None", Some(Ty::Option(_))) => None,
            ("Some", Some(Ty::Option(t))) | ("Ok", Some(Ty::Result(t, _))) | ("Err", Some(Ty::Result(_, t))) => Some(t),
            ("Some", _) => {
                let t = self.expr(&args[0], None)?;
                return Ok(Ty::Option(Box::new(t)));
            }
            (_, Some(t)) => {
                let what = if name == "None" || name == "Some" {
                    "an Option"
                } else {
                    "a Result"
                };
                return Err(err(e.span, format!("expected {t}, found {what}"), None));
            }
            _ => {
                let example = match name {
                    "None" => "let x: Option<Int> = None;",
                    "Ok" => "let x: Result<Int, Str> = Ok(1);",
                    _ => "let x: Result<Int, Str> = Err(\"no\");",
                };
                let msg = format!("cannot tell the type of `{name}`");
                if self.ret_open && self.try_shape.is_some() {
                    let hint =
                        "give the closure a type: `let f: fn(Str) -> Option<Int> = |s| ..;`, then `.and_then(f)`";
                    return Err(err(e.span, msg, Some(hint)));
                }
                return Err(err(e.span, msg, Some(&format!("add a type: `{example}`"))));
            }
        };
        if let Some(t) = payload {
            want_expr(&args[0], &self.expr(&args[0], Some(t))?, t)?;
        }
        Ok(expected.expect("matched above").clone())
    }

    /// `Map::new()`, which takes its type from `expected`.
    fn map_new(&self, e: &Expr, args: &[Expr], expected: Option<&Ty>) -> Result<Ty, Diag> {
        if !args.is_empty() {
            let msg = format!("`Map::new` takes 0 arguments, found {}", args.len());
            return Err(err(e.span, msg, Some("write `Map::new()`")));
        }
        match expected {
            Some(t @ Ty::Map(..)) => Ok(t.clone()),
            Some(t) => {
                let msg = format!("expected {t}, found a Map");
                Err(err(e.span, msg, Some("`Map::new()` makes a `Map<K, V>`")))
            }
            None => {
                let hint = "add a type: `let m: Map<Str, Int> = Map::new();`";
                Err(err(
                    e.span,
                    "cannot tell the type of `Map::new()`".to_string(),
                    Some(hint),
                ))
            }
        }
    }

    /// `x?`: the value of an `Option` or `Result`, or an early return of the `None` or `Err`.
    fn try_expr(&mut self, e: &Expr, x: &Expr) -> Result<Ty, Diag> {
        let found = self.expr(x, None)?;
        if self.ret_open && self.ret == Ty::Never && matches!(found, Ty::Option(_) | Ty::Result(..)) {
            // A closure passed to `and_then`: its return type is not known yet, but its kind is.
            match (&found, &self.try_shape) {
                (Ty::Option(t), Some(Ty::Option(_))) => return Ok((**t).clone()),
                (Ty::Result(t, e_ty), Some(Ty::Result(_, want))) if e_ty == want => return Ok((**t).clone()),
                (_, Some(shape)) => {
                    let shape = match shape {
                        Ty::Result(_, e_ty) => format!("Result<.., {e_ty}>"),
                        _ => "Option<..>".to_string(),
                    };
                    let msg = format!("`?` on {found} in a closure returning {shape}");
                    let hint = "`match` on the value, or convert it: `.ok_or(e)` makes an Option a Result";
                    return Err(err(e.span, msg, Some(hint)));
                }
                (_, None) => {}
            }
            let hint = "give the closure a type: `let f: fn(Int) -> Option<Int> = |x| ..`";
            let msg = "cannot tell what `?` returns from this closure".to_string();
            return Err(err(e.span, msg, Some(hint)));
        }
        match (&found, &self.ret) {
            (Ty::Option(t), Ty::Option(_)) => Ok((**t).clone()),
            (Ty::Result(t, e_ty), Ty::Result(_, ret_err)) if e_ty == ret_err => Ok((**t).clone()),
            (Ty::Option(_) | Ty::Result(..), ret) => {
                let msg = format!("`?` on {found} in a function returning {ret}");
                Err(err(
                    e.span,
                    msg,
                    Some("change the return type or `match` on the result"),
                ))
            }
            _ => {
                let msg = format!("`?` needs an Option or Result, found {found}");
                Err(err(x.span, msg, Some("`?` works on Option and Result values")))
            }
        }
    }

    /// The key of enum `en` as written in this module; built-in enums keep their name.
    fn enum_key(&self, en: &str) -> String {
        self.home.own(self.caps, en)
    }

    /// `m::E::V(args)`, which must name a public enum of module `m`.
    fn module_variant(&mut self, e: &Expr, m: &str, en: &str, v: &str, args: &[Expr]) -> Result<Ty, Diag> {
        let k = self.home.member(m, en, e.span)?;
        if let Some(d) = self.enums.get(&k) {
            self.home.public(&k, d.public, "enum", e.span)?;
        }
        self.variant_value(e, &k, v, args)
    }

    /// `E::V` or `E::V(args)`, with `en` the enum's key; arity and payload types must match the declaration.
    fn variant_value(&mut self, e: &Expr, en: &str, v: &str, args: &[Expr]) -> Result<Ty, Diag> {
        if builtin(v).is_some_and(|(b, _)| b == en) {
            let msg = format!("`{en}::{v}` cannot be written as a path");
            return Err(err(e.span, msg, Some(&format!("write `{v}` without `{en}::`"))));
        }
        let Some((_, payload)) = self.variants.get(en).and_then(|vs| variant(vs, v)) else {
            return Err(self.unknown_variant(e.span, en, v));
        };
        if args.len() != payload.len() {
            let msg = format!("`{en}::{v}` takes {} arguments, found {}", payload.len(), args.len());
            return Err(err(e.span, msg, Some(&holds(en, v, payload))));
        }
        for (a, ty) in args.iter().zip(payload) {
            want_expr(a, &self.expr(a, Some(ty))?, ty)?;
        }
        Ok(Ty::Enum(en.to_string()))
    }

    /// The type of an `if` (`e`) whose then-block has type `t`, given its else branch.
    fn else_branch(
        &mut self,
        e: &Expr,
        t: Ty,
        then: &Block,
        els: Option<&Expr>,
        expected: Option<&Ty>,
    ) -> Result<Ty, Diag> {
        let Some(els) = els else {
            want(then.span, &t, &Ty::Unit)?;
            return Ok(Ty::Unit);
        };
        let f = self.expr(els, expected)?;
        if t == Ty::Never {
            Ok(f)
        } else if f == Ty::Never || t == f {
            Ok(t)
        } else {
            let msg = format!("`if` branches have different types: {t} and {f}");
            Err(err(e.span, msg, Some("every branch must have the same type")))
        }
    }

    /// The type of a `match` or `if let` scrutinee, which must be one patterns can match.
    fn scrutinee(&mut self, scrut: &Expr) -> Result<Ty, Diag> {
        let st = self.expr(scrut, None)?;
        if st == Ty::Float {
            let hint = Some("compare with `<` or `==`");
            return Err(err(scrut.span, "cannot match on Float".to_string(), hint));
        }
        if !matches!(
            st,
            Ty::Int | Ty::Bool | Ty::Str | Ty::Enum(_) | Ty::Option(_) | Ty::Result(..)
        ) {
            let hint = "match works on Int, Bool, Str and enums";
            return Err(err(scrut.span, format!("cannot match on {st}"), Some(hint)));
        }
        Ok(st)
    }

    /// Checks pattern `p` against `st` and pushes a scope with the names it binds.
    fn bind_pattern(&mut self, p: &Pat, st: &Ty) -> Result<Cover, Diag> {
        let mut binds = Vec::new();
        let cover = self.pattern(p, st, &mut binds)?;
        let binds = binds.into_iter().map(|(name, ty)| (name, Local { ty, mutable: false }));
        self.scopes.push(binds.collect());
        Ok(cover)
    }

    fn match_expr(&mut self, scrut: &Expr, arms: &[Arm], expected: Option<&Ty>) -> Result<Ty, Diag> {
        let st = self.scrutinee(scrut)?;
        let too_complex = |spent| match spent {
            Spent::Match => err(
                scrut.span,
                "this `match` is too complex to check".to_string(),
                Some("split it into smaller matches"),
            ),
            Spent::Program => err(
                scrut.span,
                "this program's matches are too complex to check".to_string(),
                Some("simplify the largest matches; together they exceed what the checker will do"),
            ),
        };
        let mut u = Usefulness {
            variants: self.variants,
            budget: MATCH_BUDGET,
            program: self.match_budget,
            depth: 0,
        };
        let tys = [st.clone()];
        let mut ty = Ty::Never;
        let mut rows = Vec::new();
        for arm in arms {
            let row = vec![self.bind_pattern(&arm.pat, &st)?];
            if u.useful(&rows, &row, &tys).map_err(too_complex)?.is_none() {
                let hint = "earlier arms match every value it matches; remove it";
                return Err(err(arm.pat.span, "unreachable arm".to_string(), Some(hint)));
            }
            rows.push(row);
            let body = self.expr(&arm.body, expected)?;
            self.scopes.pop();
            if ty == Ty::Never {
                ty = body;
            } else if body != Ty::Never && body != ty {
                let msg = format!("`match` arms have different types: {ty} and {body}");
                return Err(err(arm.body.span, msg, Some("every branch must have the same type")));
            }
        }
        let Some(missing) = u.useful(&rows, &[Cover::All], &tys).map_err(too_complex)? else {
            return Ok(ty);
        };
        let missing: Vec<_> = missing.iter().map(|w| u.show(&w[0], &st)).collect();
        let msg = format!("non-exhaustive `match`: missing {}", missing.join(", "));
        let hint = "add arms for the missing cases, or `_ => ..`";
        Err(err(scrut.span, msg, Some(hint)))
    }

    /// What pattern `p` covers, typed against `st`, the type of the value it matches. Adds the
    /// names it binds to `binds`, left to right.
    fn pattern(&self, p: &Pat, st: &Ty, binds: &mut Vec<(String, Ty)>) -> Result<Cover, Diag> {
        let lit = |ty: Ty, c: Ctor| want(p.span, &ty, st).map(|_| Cover::Ctor(c, Vec::new()));
        match &p.kind {
            PatKind::Wild => Ok(Cover::All),
            PatKind::Int(n) => lit(Ty::Int, Ctor::Int(*n)),
            PatKind::Str(s) => lit(Ty::Str, Ctor::Str(s.clone())),
            PatKind::Bool(b) => lit(Ty::Bool, Ctor::Bool(*b)),
            PatKind::Name(n) if variant(&shape(self.variants, st).1, n).is_some() => {
                self.variant_pattern(p, st, n, &[], binds)
            }
            PatKind::Name(n) => {
                if binds.iter().any(|(b, _)| b == n) {
                    let msg = format!("`{n}` is bound twice in this pattern");
                    return Err(err(p.span, msg, Some("rename one of them")));
                }
                binds.push((n.clone(), st.clone()));
                Ok(Cover::All)
            }
            PatKind::Module(m, inner) => {
                let PatKind::Variant(Some(named), v, parts) = &inner.kind else {
                    let hint = "write `web::E::V`, naming the enum";
                    return Err(err(p.span, "expected a module enum variant".to_string(), Some(hint)));
                };
                let k = self.home.member(m, named, p.span)?;
                if let Some(d) = self.enums.get(&k) {
                    self.home.public(&k, d.public, "enum", p.span)?;
                }
                self.enum_pattern(p, st, k, v, parts, binds)
            }
            PatKind::Variant(Some(named), v, parts) => self.enum_pattern(p, st, self.enum_key(named), v, parts, binds),
            PatKind::Variant(None, v, parts) => self.variant_pattern(p, st, v, parts, binds),
        }
    }

    /// `E::V(..)`, where enum key `k` must be the matched value's enum.
    fn enum_pattern(
        &self,
        p: &Pat,
        st: &Ty,
        k: String,
        v: &str,
        parts: &[Pat],
        binds: &mut Vec<(String, Ty)>,
    ) -> Result<Cover, Diag> {
        if k != shape(self.variants, st).0 {
            want(p.span, &Ty::Enum(k), st)?;
        }
        self.variant_pattern(p, st, v, parts, binds)
    }

    fn variant_pattern(
        &self,
        p: &Pat,
        st: &Ty,
        v: &str,
        parts: &[Pat],
        binds: &mut Vec<(String, Ty)>,
    ) -> Result<Cover, Diag> {
        let (en, vs) = shape(self.variants, st);
        if en.is_empty() {
            let hint = format!("match {st} with a name or `_`");
            return Err(err(p.span, format!("expected {st}, found variant `{v}`"), Some(&hint)));
        }
        let Some((i, payload)) = variant(&vs, v) else {
            return Err(self.unknown_variant(p.span, &en, v));
        };
        if parts.len() != payload.len() {
            let msg = format!(
                "`{en}::{v}` has {} values, found {} bindings",
                payload.len(),
                parts.len()
            );
            return Err(err(p.span, msg, Some(&holds(&en, v, payload))));
        }
        let parts = parts.iter().zip(payload).map(|(q, ty)| self.pattern(q, ty, binds));
        Ok(Cover::Ctor(Ctor::Variant(i), parts.collect::<Result<_, _>>()?))
    }

    /// The type of field `name` of a value of type `ty`.
    fn field(&self, ty: &Ty, name: &str, span: Span) -> Result<Ty, Diag> {
        let Ty::Struct(s) = ty else {
            let hint = "only structs have fields";
            return Err(err(span, format!("no field `{name}` on {ty}"), Some(hint)));
        };
        let decl = &self.fields[s.as_str()];
        match decl.iter().find(|(n, _)| n == name) {
            Some((_, t)) => Ok(t.clone()),
            None => Err(no_field(span, name, s, decl)),
        }
    }

    /// `Name { .. }`, with `name` the struct's key.
    fn struct_lit(&mut self, e: &Expr, name: &str, inits: &[(String, Expr)]) -> Result<Ty, Diag> {
        let fields = self.fields;
        let Some(decl) = fields.get(name) else {
            let hint = match self.is_cap(name) {
                true => NO_CAP_SYNTAX.to_string(),
                false if !self.home.module.is_empty() && self.structs.contains_key(bare(name)) => ROOT_ITEM.to_string(),
                false => suggest(bare(name), &sorted_names(self.structs, module_of(name)), "structs"),
            };
            return Err(err(e.span, format!("unknown struct `{name}`"), Some(&hint)));
        };
        let mut seen = Vec::new();
        for (f, x) in inits {
            let Some((_, ty)) = decl.iter().find(|(n, _)| n == f) else {
                return Err(no_field(x.span, f, name, decl));
            };
            if seen.contains(&f) {
                return Err(err(
                    x.span,
                    format!("duplicate field `{f}`"),
                    Some("remove one of them"),
                ));
            }
            seen.push(f);
            want_expr(x, &self.expr(x, Some(ty))?, ty)?;
        }
        if let Some((missing, _)) = decl.iter().find(|(n, _)| !seen.contains(&n)) {
            let msg = format!("missing field `{missing}` in `{name}`");
            return Err(err(e.span, msg, Some(&format!("add `{missing}: ..`"))));
        }
        Ok(Ty::Struct(name.to_string()))
    }

    /// The type of an assignment target: a `let mut` local followed by fields and indexes.
    /// Recorded like an expression's.
    fn place(&mut self, target: &Expr) -> Result<Ty, Diag> {
        let ty = match &target.kind {
            ExprKind::Name(name) => {
                let captured = self.is_capture(name);
                let Some(local) = self.lookup(name, target.span)? else {
                    return Err(self.unknown_name(target.span, name));
                };
                if captured {
                    return Err(Self::capture_changed(target.span, name, "assigned"));
                }
                if !local.mutable {
                    let msg = format!("cannot assign to `{name}`, it is not `mut`");
                    return Err(err(target.span, msg, Some("declare it with `let mut`")));
                }
                Ok(local.ty.clone())
            }
            ExprKind::Field(base, name) => {
                let ty = self.place(base)?;
                self.field(&ty, name, target.span)
            }
            ExprKind::Index(base, index) => {
                let ty = self.place(base)?;
                if ty == Ty::Bytes {
                    let hint = "use `push`, or build new Bytes";
                    return Err(err(target.span, "cannot assign to a byte".to_string(), Some(hint)));
                }
                self.element(&ty, index, target.span)
            }
            _ => Err(err(
                target.span,
                "cannot assign to this expression".to_string(),
                Some("assign to a `let mut` variable, or a field or index of one"),
            )),
        }?;
        self.types.insert(target.span, ty.clone());
        Ok(ty)
    }

    /// `push`, `insert`, `remove`, `pop` and `sort` change their receiver, so it must be a place rooted at a
    /// `let mut` local.
    fn mutation_target(&mut self, recv: &Expr, method: &str, recv_ty: &Ty) -> Result<(), Diag> {
        let (noun, to) = match (recv_ty, method) {
            (Ty::List(_), "sort") => ("list", ""),
            (Ty::List(_), "pop") => ("list", " from"),
            (Ty::List(_), _) => ("list", " to"),
            (Ty::Bytes, _) => ("bytes", " to"),
            (_, "remove") => ("map", " from"),
            _ => ("map", " to"),
        };
        let mut root = recv;
        while let ExprKind::Field(base, _) | ExprKind::Index(base, _) = &root.kind {
            root = base;
        }
        let ExprKind::Name(name) = &root.kind else {
            let hint = format!("`{method}` changes a {noun}; call it on a `let mut` variable");
            return Err(err(
                recv.span,
                format!("cannot `{method}`{to} this expression"),
                Some(&hint),
            ));
        };
        if self.is_capture(name) {
            return Err(Self::capture_changed(root.span, name, "changed"));
        }
        if !self.lookup(name, root.span)?.is_some_and(|l| l.mutable) {
            let msg = format!("cannot `{method}`{to} `{name}`, it is not `mut`");
            return Err(err(root.span, msg, Some("declare it with `let mut`")));
        }
        Ok(())
    }

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Expr], expected: Option<&Ty>) -> Result<Ty, Diag> {
        if let ExprKind::Name(name) = &callee.kind {
            // A local hides a function of the same name, as in Rust.
            if builtin(name).is_none() && self.lookup(name, callee.span)?.is_some() {
                return self.call_value(e, callee, args);
            }
        }
        let (name, module) = match &callee.kind {
            ExprKind::Name(name) if builtin(name).is_some() => return self.constructor(e, name, args, expected),
            ExprKind::Name(name) => (name, None),
            ExprKind::Path(en, v) if en == "Map" && v == "new" => return self.map_new(e, args, expected),
            ExprKind::Path(en, v) => return self.variant_value(e, &self.enum_key(en), v, args),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::Name(name) => (name, Some(m)),
                ExprKind::Path(en, v) => return self.module_variant(e, m, en, v, args),
                _ => return Err(nested_path(callee.span)),
            },
            _ => return self.call_value(e, callee, args),
        };
        let (_, sig) = self.named_fn(callee.span, name, module.map(String::as_str))?;
        if args.len() != sig.params.len() {
            let msg = format!("`{name}` takes {} arguments, found {}", sig.params.len(), args.len());
            let hint = format!("`{name}` is declared as `{}`", sig.decl);
            return Err(err(e.span, msg, Some(&hint)));
        }
        for (a, param) in args.iter().zip(&sig.params) {
            want_expr(a, &self.operand(a, Some(param))?, param)?;
        }
        Ok(sig.ret.clone())
    }

    /// The key and signature of function `name`, written `m::name` when `module` is `Some(m)`.
    fn named_fn(&self, span: Span, name: &str, module: Option<&str>) -> Result<(String, &'a Sig), Diag> {
        let k = match module {
            Some(m) => self.home.member(m, name, span)?,
            None => key(self.home.module, name).into_owned(),
        };
        let sigs = self.sigs;
        let Some(sig) = sigs.get(&k) else {
            let hint = if module.is_none() && !self.home.module.is_empty() && sigs.contains_key(name) {
                ROOT_ITEM.to_string()
            } else {
                suggest(name, &sorted_names(sigs, module_of(&k)), "functions")
            };
            return Err(err(span, format!("unknown function `{k}`"), Some(&hint)));
        };
        if module.is_some() {
            self.home.public(&k, sig.public, "fn", span)?;
        }
        Ok((k, sig))
    }

    /// Function `k` named as a value at `span`. Its parameters hold no capability, so calling
    /// the value never adds authority.
    fn fn_value(&self, span: Span, k: &str) -> Result<Ty, Diag> {
        if k == "main" {
            let hint = "move the code into another function and use that";
            return Err(err(span, "`main` cannot be used as a value".to_string(), Some(hint)));
        }
        let sig = &self.sigs[k];
        if sig.params.iter().any(|p| cap_in(p).is_some()) {
            return Err(fn_cap_err(span));
        }
        Ok(Ty::Fn(sig.params.clone(), Box::new(sig.ret.clone())))
    }

    /// `f(x)` where `f` is a local, or `(e)(x)`: a call through a value of `fn` type.
    fn call_value(&mut self, e: &Expr, callee: &Expr, args: &[Expr]) -> Result<Ty, Diag> {
        let (ty, what) = match &callee.kind {
            ExprKind::Name(name) => {
                // Not `expr`, which would refuse a capability before this says it is not a function.
                let ty = self.lookup(name, callee.span)?.expect("a local").ty.clone();
                self.types.insert(callee.span, ty.clone());
                (ty, format!("`{name}`"))
            }
            _ => (self.expr(callee, None)?, "this expression".to_string()),
        };
        let Ty::Fn(params, ret) = &ty else {
            let hint = match &callee.kind {
                ExprKind::Name(n) if self.sigs.contains_key(&*key(self.home.module, n)) => {
                    format!("the local `{n}` hides the function `{n}`; rename the local")
                }
                _ => "only functions and values of `fn` type can be called".to_string(),
            };
            return Err(err(callee.span, format!("{what} is {ty}, not a function"), Some(&hint)));
        };
        if args.len() != params.len() {
            let msg = format!("{what} takes {} arguments, found {}", params.len(), args.len());
            return Err(err(e.span, msg, Some(&format!("{what} has type `{ty}`"))));
        }
        for (a, param) in args.iter().zip(params) {
            want_expr(a, &self.operand(a, Some(param))?, param)?;
        }
        Ok((**ret).clone())
    }

    /// `|a, b: T| body`. Parameter types come from the annotations, else from an expected `fn`
    /// type; the return type from the expected type, else from the body and its `return`s.
    fn closure(&mut self, e: &Expr, params: &[ClosureParam], body: &Expr, expected: Option<&Ty>) -> Result<Ty, Diag> {
        let want_fn = match expected {
            Some(Ty::Fn(ps, r)) => Some((ps, r)),
            _ => None,
        };
        if let Some((ps, _)) = want_fn.filter(|(ps, _)| ps.len() != params.len()) {
            let ty = expected.expect("a fn type");
            let msg = format!("expected {ty}, found a closure of {} parameters", params.len());
            return Err(err(e.span, msg, Some(&format!("`{ty}` takes {}", ps.len()))));
        }
        let want = want_fn.map(|(ps, r)| (ps.as_slice(), Some((**r).clone())));
        self.closure_with(e, params, body, want, None, expected)
    }

    /// A closure whose parameter types, where not annotated, come from `want` (as many as
    /// `params`), and whose return type is `want`'s, else its body's. `shape` is `try_shape`
    /// for its body; `expected` is only for the error on an untyped parameter.
    fn closure_with(
        &mut self,
        e: &Expr,
        params: &[ClosureParam],
        body: &Expr,
        want: Option<(&[Ty], Option<Ty>)>,
        shape: Option<Ty>,
        expected: Option<&Ty>,
    ) -> Result<Ty, Diag> {
        let mut scope = HashMap::new();
        let mut tys = Vec::new();
        for (i, p) in params.iter().enumerate() {
            not_builtin(&p.name, p.span)?;
            if params[..i].iter().any(|q| q.name == p.name) {
                let msg = format!("duplicate parameter `{}`", p.name);
                return Err(err(p.span, msg, Some("rename one of them")));
            }
            let ty = match (&p.ty, &want) {
                (Some(t), _) => {
                    let ty = resolve(t, self.caps, self.structs, self.enums, self.home)?;
                    if cap_in(&ty).is_some() {
                        return Err(fn_cap_err(t.span));
                    }
                    ty
                }
                (None, Some((ps, _))) => ps[i].clone(),
                (None, None) => {
                    let msg = match expected {
                        Some(t) if *t != Ty::Never => format!("expected {t}, found a closure"),
                        _ => format!("cannot tell the type of `{}`", p.name),
                    };
                    let hint = format!("add a type: `|{}: Int| ..`", p.name);
                    return Err(err(p.span, msg, Some(&hint)));
                }
            };
            tys.push(ty.clone());
            scope.insert(p.name.clone(), Local { ty, mutable: false });
        }
        let ret = want.and_then(|(_, r)| r);
        let outer_shape = std::mem::replace(&mut self.try_shape, shape);
        let outer_ret = std::mem::replace(&mut self.ret, ret.clone().unwrap_or(Ty::Never));
        let outer_open = std::mem::replace(&mut self.ret_open, ret.is_none());
        let outer_loops = std::mem::take(&mut self.loops);
        self.closures.push(ClosureFrame {
            base: self.scopes.len(),
            captures: Vec::new(),
            in_loop: !outer_loops.is_empty() || self.closures.last().is_some_and(|c| c.in_loop),
        });
        self.scopes.push(scope);
        let found = self.expr(body, ret.as_ref());
        self.scopes.pop();
        let frame = self.closures.pop().expect("pushed above");
        self.loops = outer_loops;
        self.ret_open = outer_open;
        self.try_shape = outer_shape;
        let ret = std::mem::replace(&mut self.ret, outer_ret);
        let found = found?;
        let ret = match ret {
            // No expected type and no `return`: the body's type; `()` if it never finishes.
            Ty::Never if found == Ty::Never => Ty::Unit,
            Ty::Never => found,
            ret => {
                let at = match &body.kind {
                    ExprKind::Block(Block { tail: Some(t), .. }) => t,
                    _ => body,
                };
                want_expr(at, &found, &ret)?;
                ret
            }
        };
        let ty = Ty::Fn(tys, Box::new(ret));
        // Unreachable: parameters are checked above, and no expression has a capability type.
        if cap_in(&ty).is_some() {
            return Err(fn_cap_err(e.span));
        }
        self.captures.insert(e.span, frame.captures);
        Ok(ty)
    }

    /// Whether a value of `ty` can hold a function, which has no equality.
    fn has_fn(&self, ty: &Ty) -> bool {
        has_fn(ty, self.fields, self.variants, &mut HashSet::new())
    }
}
