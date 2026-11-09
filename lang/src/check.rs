use crate::ast::*;
use crate::eval::{Key, Value};
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
    /// Constants by key, each checked to be a literal of its type; lowering inlines them.
    pub(crate) consts: HashMap<String, Const>,
    /// Return type of each capability method, keyed by (capability, method), such as (`Dir`, `read`).
    pub(crate) cap_methods: HashMap<(String, String), Ty>,
    /// Resolved payload types of every enum, for checking host values.
    pub(crate) variants: Variants,
    pub(crate) fields: Fields,
    pub(crate) tparams: TypeParams,
    /// Resolved signature of each function, by key.
    pub(crate) sigs: HashMap<String, Sig>,
    /// The type of every expression and assignment target, by its span, which no other
    /// expression has; and the type of each `let`'s local, by the span of the `let`. Inside a
    /// generic function a type may hold its type parameters (`Ty::Param`); lowering
    /// substitutes each instance's type arguments.
    pub(crate) types: HashMap<Span, Ty>,
    /// The outer locals each closure uses, by the closure's span, in order of first use: what
    /// it copies when it is created.
    pub(crate) captures: HashMap<Span, Vec<String>>,
    /// Each use of a generic function, called or as a value, by the span of the expression
    /// naming it: the function's key and its type arguments, which may hold the type
    /// parameters of the function the use is in.
    pub(crate) instances: HashMap<Span, (String, Vec<Ty>)>,
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
            Value::Map(m) => self.entries(m.iter()),
            Value::HashMap(m) => self.entries(m.iter()),
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

    /// A map's entries as `{k: v, ..}`, in the order given.
    fn entries<'v>(&self, entries: impl Iterator<Item = (&'v Key, &'v Value)>) -> String {
        let entries: Vec<_> = entries
            .map(|(k, v)| format!("{}: {}", self.nested(&k.to_value()), self.nested(v)))
            .collect();
        format!("{{{}}}", entries.join(", "))
    }

    pub(crate) fn is_cap(&self, ty: &str) -> bool {
        self.cap_methods.keys().any(|(c, _)| c == ty)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Bytes,
    Unit,
    /// A struct by key, with its type arguments (none if it is not generic).
    Struct(String, Vec<Ty>),
    /// An enum by key, with its type arguments.
    Enum(String, Vec<Ty>),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    HashMap(Box<Ty>, Box<Ty>),
    /// A capability, which only a parameter can hold.
    Cap(String),
    /// A function value: parameter types and return type. Never mentions a capability.
    Fn(Vec<Ty>, Box<Ty>),
    /// A type parameter `T` of the generic function being checked: opaque, so it can be
    /// passed, stored and returned, and nothing else. Never a capability.
    Param(String),
    /// A block that always returns; it matches any type.
    Never,
}

/// `ty` with each type parameter named in `binds` replaced by its type.
pub(crate) fn subst(ty: &Ty, binds: &[(String, Ty)]) -> Ty {
    let s = |t: &Ty| Box::new(subst(t, binds));
    match ty {
        Ty::Param(n) => match binds.iter().find(|(b, _)| b == n) {
            Some((_, t)) => t.clone(),
            None => ty.clone(),
        },
        Ty::Option(t) => Ty::Option(s(t)),
        Ty::List(t) => Ty::List(s(t)),
        Ty::Result(t, e) => Ty::Result(s(t), s(e)),
        Ty::Map(k, v) => Ty::Map(s(k), s(v)),
        Ty::HashMap(k, v) => Ty::HashMap(s(k), s(v)),
        Ty::Fn(ps, r) => Ty::Fn(ps.iter().map(|p| subst(p, binds)).collect(), s(r)),
        Ty::Struct(k, args) => Ty::Struct(k.clone(), args.iter().map(|a| subst(a, binds)).collect()),
        Ty::Enum(k, args) => Ty::Enum(k.clone(), args.iter().map(|a| subst(a, binds)).collect()),
        _ => ty.clone(),
    }
}

/// The bound of type parameter `t` among `own`, the type parameters in scope.
fn bound_of(own: &[TParam], t: &str) -> Option<TBound> {
    own.iter().find(|p| p.name == t).and_then(|p| p.bound)
}

/// The first type parameter in `ty` without a bound among `own`; `==` on `ty` needs none.
fn unbounded<'t>(ty: &'t Ty, own: &[TParam]) -> Option<&'t str> {
    match ty {
        Ty::Param(t) if bound_of(own, t).is_none() => Some(t),
        Ty::Option(t) | Ty::List(t) => unbounded(t, own),
        Ty::Result(t, e) | Ty::Map(t, e) | Ty::HashMap(t, e) => unbounded(t, own).or_else(|| unbounded(e, own)),
        Ty::Fn(ps, r) => ps.iter().find_map(|p| unbounded(p, own)).or_else(|| unbounded(r, own)),
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().find_map(|a| unbounded(a, own)),
        _ => None,
    }
}

/// Why `ty` does not satisfy bound `b`, as a hint naming the fix; `None` if it does. `Eq`
/// holds where `==` is accepted (no function inside, each type parameter bounded); `Ord`
/// only for `Int`, `Str` and a type parameter bounded `Ord`. `own` are the type parameters
/// in scope.
fn unmet(ty: &Ty, b: TBound, own: &[TParam], fields: &Fields, variants: &Variants) -> Option<String> {
    match (b, ty) {
        (TBound::Ord, Ty::Int | Ty::Str) => None,
        (_, Ty::Param(t)) if bound_of(own, t) >= Some(b) => None,
        (_, Ty::Param(t)) => Some(format!("add the bound: `{t}: {b}`")),
        (TBound::Ord, _) => Some("only Int and Str are Ord".to_string()),
        (TBound::Eq, _) if has_fn(ty, fields, variants, &mut HashSet::new()) => {
            Some("functions have no equality".to_string())
        }
        (TBound::Eq, _) => unbounded(ty, own).map(|t| format!("add the bound: `{t}: Eq`")),
    }
}

/// `ty`, given for type parameter `t` of `owner`, does not satisfy `t`'s bound `b`.
fn bound_err(span: Span, ty: &Ty, (owner, t, b): (&str, &str, TBound), why: &str) -> Diag {
    err(
        span,
        format!("`{ty}` is not `{b}`: `{owner}` needs `{t}: {b}`"),
        Some(why),
    )
}

/// Type parameters as declared: `T, U: Ord`.
fn tparams_text(tparams: &[TParam]) -> String {
    let each = tparams.iter().map(|t| match t.bound {
        Some(b) => format!("{}: {b}", t.name),
        None => t.name.clone(),
    });
    each.collect::<Vec<_>>().join(", ")
}

/// Whether `ty` mentions a type parameter.
fn has_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::Option(t) | Ty::List(t) => has_param(t),
        Ty::Result(t, e) | Ty::Map(t, e) | Ty::HashMap(t, e) => has_param(t) || has_param(e),
        Ty::Fn(ps, r) => ps.iter().any(has_param) || has_param(r),
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(has_param),
        _ => false,
    }
}

/// The number of type constructors in `ty`, to bound the type arguments of instances.
fn ty_size(ty: &Ty) -> usize {
    1 + match ty {
        Ty::Option(t) | Ty::List(t) => ty_size(t),
        Ty::Result(t, e) | Ty::Map(t, e) | Ty::HashMap(t, e) => ty_size(t) + ty_size(e),
        Ty::Fn(ps, r) => ps.iter().map(ty_size).sum::<usize>() + ty_size(r),
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().map(ty_size).sum(),
        _ => 0,
    }
}

/// Type parameter names of each generic struct and enum, by key.
pub(crate) type TypeParams = HashMap<String, Vec<String>>;

/// The bindings of a generic type's parameters to the arguments in `ty`, a struct or enum
/// type; empty for any other type.
pub(crate) fn type_binds(tparams: &TypeParams, ty: &Ty) -> Vec<(String, Ty)> {
    match ty {
        Ty::Struct(k, args) | Ty::Enum(k, args) if !args.is_empty() => {
            tparams[k].iter().cloned().zip(args.iter().cloned()).collect()
        }
        _ => Vec::new(),
    }
}

/// The fields of struct type `ty`, with its type arguments in place.
pub(crate) fn fields_of(fields: &Fields, tparams: &TypeParams, ty: &Ty) -> Vec<(String, Ty)> {
    let Ty::Struct(k, _) = ty else {
        unreachable!("a struct type");
    };
    let binds = type_binds(tparams, ty);
    fields[k].iter().map(|(n, t)| (n.clone(), subst(t, &binds))).collect()
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
            Ty::Struct(name, args) | Ty::Enum(name, args) if !args.is_empty() => {
                let args: Vec<_> = args.iter().map(Ty::to_string).collect();
                write!(f, "{name}<{}>", args.join(", "))
            }
            Ty::Struct(name, _) | Ty::Enum(name, _) | Ty::Cap(name) | Ty::Param(name) => f.write_str(name),
            Ty::Option(t) => write!(f, "Option<{t}>"),
            Ty::Result(t, e) => write!(f, "Result<{t}, {e}>"),
            Ty::List(t) => write!(f, "List<{t}>"),
            Ty::Map(k, v) => write!(f, "Map<{k}, {v}>"),
            Ty::HashMap(k, v) => write!(f, "HashMap<{k}, {v}>"),
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
    matches!(name, "Option" | "Result" | "Map" | "HashMap" | "Json")
        || caps.enums.iter().any(|e| e.name == name)
        || caps.caps.iter().any(|c| c.name == name)
}

/// The module being checked, the modules the root declares, and the type parameters of the
/// function or type being checked. `types` holds every type's resolved fields and variants
/// once they are known: only then are the bounds of type arguments written in it checked.
#[derive(Clone, Copy)]
struct Scope<'a> {
    module: &'a str,
    mods: &'a HashSet<String>,
    tparams: &'a [TParam],
    types: Option<(&'a Fields, &'a Variants)>,
}

impl Scope<'_> {
    fn is_tparam(&self, name: &str) -> bool {
        self.tparams.iter().any(|t| t.name == name)
    }

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

/// The error for a `match` or `let`-else whose exhaustiveness check ran out of `spent`.
fn too_complex(span: Span, spent: Spent) -> Diag {
    match spent {
        Spent::Match => err(
            span,
            "this `match` is too complex to check".to_string(),
            Some("split it into smaller matches"),
        ),
        Spent::Program => err(
            span,
            "this program's matches are too complex to check".to_string(),
            Some("simplify the largest matches; together they exceed what the checker will do"),
        ),
    }
}

/// Checks that alternatives at `a` and `b` of one pattern bind the same names, `a_binds` and
/// `b_binds`, with the same types.
fn same_binds(a_binds: &[(String, Ty)], b_binds: &[(String, Ty)], a: Span, b: Span) -> Result<(), Diag> {
    for (n, t) in a_binds {
        match b_binds.iter().find(|(m, _)| m == n) {
            None => return Err(not_bound(b, n)),
            Some((_, u)) if u != t => {
                let msg = format!("`{n}` is {t} in one alternative and {u} in another");
                return Err(err(b, msg, Some("bind values of the same type in every alternative")));
            }
            Some(_) => {}
        }
    }
    match b_binds.iter().find(|(m, _)| !a_binds.iter().any(|(n, _)| n == m)) {
        Some((m, _)) => Err(not_bound(a, m)),
        None => Ok(()),
    }
}

fn not_bound(span: Span, name: &str) -> Diag {
    let msg = format!("`{name}` is bound in one alternative but not the other");
    err(
        span,
        msg,
        Some(&format!("bind `{name}` in every alternative, or in none")),
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
    /// `P1 | P2 | ..`
    Or(Vec<Cover>),
}

/// The rows `r` stands for, with an alternative of a leading `Cover::Or` in place of it, recursively.
fn expand(r: &[Cover], out: &mut Vec<Vec<Cover>>) {
    match r.first() {
        Some(Cover::Or(alts)) => {
            for a in alts {
                expand(&[std::slice::from_ref(a), &r[1..]].concat(), out);
            }
        }
        _ => out.push(r.to_vec()),
    }
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
    tparams: &'a TypeParams,
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
            Ty::Enum(..) | Ty::Option(_) | Ty::Result(..) => {
                let vs = shape(self.variants, self.tparams, ty).1.into_iter().enumerate();
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
        let mut expanded = Vec::new();
        rows.iter().for_each(|r| expand(r, &mut expanded));
        let rows = &expanded[..];
        let cost = (rows.len() + 1) * (q.len() + 1);
        self.budget = self.budget.checked_sub(cost).ok_or(Spent::Match)?;
        self.program
            .set(self.program.get().checked_sub(cost).ok_or(Spent::Program)?);
        let Some((head, rest)) = q.split_first() else {
            return Ok(None);
        };
        match head {
            Cover::Ctor(c, parts) => return self.specialized(rows, c, parts.clone(), rest, tys),
            // The values some alternative matches and no row does.
            Cover::Or(alts) => {
                for a in alts {
                    let q = [std::slice::from_ref(a), rest].concat();
                    if let Some(ws) = self.useful(rows, &q, tys)? {
                        return Ok(Some(ws));
                    }
                }
                return Ok(None);
            }
            Cover::All => {}
        }
        let seen: Vec<&Ctor> = (rows.iter())
            .filter_map(|r| match &r[0] {
                Cover::Ctor(c, _) => Some(c),
                Cover::All | Cover::Or(_) => None,
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
                Cover::Ctor(..) | Cover::Or(_) => None,
                Cover::All => Some([&vec![Cover::All; n], &r[1..]].concat()),
            })
            .collect();
        let part_tys = match c {
            Ctor::Variant(i) => shape(self.variants, self.tparams, &tys[0]).1.swap_remove(*i).1,
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
            Cover::Or(alts) => alts.iter().map(|a| self.show(a, ty)).collect::<Vec<_>>().join(" | "),
            Cover::Ctor(Ctor::Bool(b), _) => b.to_string(),
            Cover::Ctor(Ctor::Int(n), _) => n.to_string(),
            Cover::Ctor(Ctor::Str(s), _) => format!("{s:?}"),
            Cover::Ctor(Ctor::Variant(i), parts) => {
                let (en, vs) = shape(self.variants, self.tparams, ty);
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
    /// Type parameters, which `params` and `ret` name as `Ty::Param`; empty if not generic.
    pub(crate) tparams: Vec<String>,
    /// The bound of each type parameter.
    bounds: Vec<Option<TBound>>,
    pub(crate) params: Vec<Ty>,
    pub(crate) ret: Ty,
    /// For a generic struct literal or variant, typed like a call: how each value is named
    /// in messages (`field `b``), and the use and an example for "cannot infer".
    parts: Option<Vec<String>>,
    infer: Option<(String, String)>,
}

impl Sig {
    /// How the `arg`th value (from 1) of a use is named in messages.
    fn part(&self, arg: usize) -> String {
        match &self.parts {
            Some(parts) => parts[arg - 1].clone(),
            None => format!("argument {arg}"),
        }
    }
}

/// What a call has fixed a type parameter to: its type, and the span and argument number
/// (from 1; 0 for the expected type or a turbofish) that fixed it.
#[derive(Clone)]
struct Bound {
    ty: Ty,
    at: Span,
    arg: usize,
}

/// Distinct instances one generic function may have, and the size of each type argument
/// (`ty_size`), beyond which instantiating stops: recursion that changes the type arguments.
const MAX_INSTANCES: usize = 64;
const MAX_TYPE_ARG: usize = 256;

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
        "+" | "<" | "<=" | ">" | ">=" => "two Int, two Float or two Str",
        "-" | "*" | "/" | "%" => "two Int or two Float",
        "&&" | "||" => "two Bool",
        _ => "two values of one type that holds no capability",
    };
    format!("`{op}` works on {accepts}")
}

/// The error for `l op r` that `binary_ty` refused, at `span`; `written` is `op` or `op=`.
/// `own` are the type parameters in scope.
fn binary_err(
    op: &str,
    written: &str,
    (l, r): (&Ty, &Ty),
    own: &[TParam],
    span: Span,
    operands: (&Expr, &Expr),
) -> Diag {
    let text_and_number = matches!(
        (l, r),
        (Ty::Str, Ty::Int | Ty::Float | Ty::Bool) | (Ty::Int | Ty::Float | Ty::Bool, Ty::Str)
    );
    if op == "+" && text_and_number {
        let number = if *l == Ty::Str { operands.1 } else { operands.0 };
        let shown = match &number.kind {
            ExprKind::Int(n) if *n >= 0 => n.to_string(),
            ExprKind::Float(x) if f64::from_bits(*x) >= 0.0 => format!("{:?}", f64::from_bits(*x)),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Name(n) => n.clone(),
            _ => "x".to_string(),
        };
        let hint = format!("convert with `{shown}.to_str()`");
        return err(
            span,
            format!("`{written}` cannot be applied to {l} and {r}"),
            Some(&hint),
        );
    }
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
    if let (true, Some(t)) = (matches!(op, "==" | "!=") && l == r, unbounded(l, own)) {
        return no_param_equality(span, l, t);
    }
    if let (true, Ty::Param(t)) = (matches!(op, "<" | "<=" | ">" | ">=") && l == r, l) {
        return err(
            span,
            format!("`{written}` cannot be applied to {l} and {r}"),
            Some(&format!("add the bound: `{t}: Ord`")),
        );
    }
    if let Some(Ty::Param(t)) = [l, r].into_iter().find(|t| matches!(t, Ty::Param(_))) {
        let hint = format!("`{t}` is a type parameter, which has no operators; take a function for it, like `add: fn({t}, {t}) -> {t}`");
        return err(
            span,
            format!("`{written}` cannot be applied to {l} and {r}"),
            Some(&hint),
        );
    }
    err(
        span,
        format!("`{written}` cannot be applied to {l} and {r}"),
        Some(&op_hint(op)),
    )
}

/// The result type of `l op r`, or `None` if the operator does not apply; `own` are the type
/// parameters in scope, whose bounds allow `==` (`Eq`) and `<` (`Ord`).
fn binary_ty(op: &str, l: &Ty, r: &Ty, own: &[TParam]) -> Option<Ty> {
    use Ty::*;
    match (op, l, r) {
        ("+", Str, Str) => Some(Str),
        ("+" | "-" | "*" | "/" | "%", Int, Int) => Some(Int),
        ("<" | "<=" | ">" | ">=", Int, Int) => Some(Bool),
        ("+" | "-" | "*" | "/" | "%", Float, Float) => Some(Float),
        ("<" | "<=" | ">" | ">=", Float, Float) => Some(Bool),
        ("<" | "<=" | ">" | ">=", Str, Str) => Some(Bool),
        ("<" | "<=" | ">" | ">=", Param(t), Param(u)) if t == u && bound_of(own, t) == Some(TBound::Ord) => Some(Bool),
        ("&&" | "||", Bool, Bool) => Some(Bool),
        ("==" | "!=", a, b) if a == b && *a != Never && cap_in(a).is_none() && unbounded(a, own).is_none() => {
            Some(Bool)
        }
        _ => None,
    }
}

/// A generic function as written at a use (`first`, `web::first`) and its signature.
type Generic<'s> = (&'s str, &'s Sig);

/// `p`, a type in `sig`, with its type parameters replaced by their bindings; `None` while
/// one of them is unbound.
fn known(p: &Ty, sig: &Sig, binds: &[Option<Bound>]) -> Option<Ty> {
    let mut pairs = Vec::new();
    for (t, b) in sig.tparams.iter().zip(binds) {
        match b {
            Some(b) => pairs.push((t.clone(), b.ty.clone())),
            None if mentions(p, t) => return None,
            None => {}
        }
    }
    Some(subst(p, &pairs))
}

/// The module `e` is written in (`web::` before it) and what follows it.
fn in_module(e: &Expr) -> (Option<&str>, &ExprKind) {
    match &e.kind {
        ExprKind::Module(m, inner) => (Some(m), &inner.kind),
        kind => (None, kind),
    }
}

/// Whether `a` could be `p`, a type in a generic signature, once its type parameters are
/// known.
fn fits(p: &Ty, a: &Ty) -> bool {
    match (p, a) {
        (Ty::Param(_), _) | (_, Ty::Never) => true,
        (Ty::Option(p), Ty::Option(a)) | (Ty::List(p), Ty::List(a)) => fits(p, a),
        (Ty::Result(p, q), Ty::Result(a, b))
        | (Ty::Map(p, q), Ty::Map(a, b))
        | (Ty::HashMap(p, q), Ty::HashMap(a, b)) => fits(p, a) && fits(q, b),
        (Ty::Fn(ps, r), Ty::Fn(qs, s)) => {
            ps.len() == qs.len() && ps.iter().zip(qs).all(|(p, q)| fits(p, q)) && fits(r, s)
        }
        (Ty::Struct(n, ps), Ty::Struct(m, qs)) | (Ty::Enum(n, ps), Ty::Enum(m, qs)) => {
            n == m && ps.iter().zip(qs).all(|(p, q)| fits(p, q))
        }
        _ => p == a,
    }
}

/// When a use of `g` with unbound type parameters was expected to have type `want` and
/// cannot: "expected Tree<Int>, found Other<..>", with `..` for each unbound parameter.
fn not_expected(span: Span, (_, sig): Generic, binds: &[Option<Bound>], want: Option<&Ty>) -> Option<Diag> {
    let want = want.filter(|w| !fits(&sig.ret, w))?;
    let pairs: Vec<(String, Ty)> = (sig.tparams.iter().zip(binds))
        .map(|(t, b)| {
            (
                t.clone(),
                b.as_ref().map_or(Ty::Param("..".to_string()), |b| b.ty.clone()),
            )
        })
        .collect();
    let found = subst(&sig.ret, &pairs);
    Some(err(span, format!("expected {want}, found {found}"), None))
}

/// Whether `ty` mentions type parameter `t`.
fn mentions(ty: &Ty, t: &str) -> bool {
    match ty {
        Ty::Param(n) => n == t,
        Ty::Option(x) | Ty::List(x) => mentions(x, t),
        Ty::Result(x, y) | Ty::Map(x, y) | Ty::HashMap(x, y) => mentions(x, t) || mentions(y, t),
        Ty::Fn(ps, r) => ps.iter().any(|p| mentions(p, t)) || mentions(r, t),
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(|a| mentions(a, t)),
        _ => false,
    }
}

/// First-order matching of `p`, a type in the generic function's signature, against `a`, the
/// type found at `(span, argument number)` (0: the expected type): each type parameter in `p`
/// is bound where it first occurs. `strict` refuses a later occurrence of another type and a
/// shape that cannot match; otherwise only unbound parameters are filled. A capability is
/// never bound (rule 1). `!` fixes nothing.
fn unify(
    p: &Ty,
    a: &Ty,
    (written, sig): Generic,
    binds: &mut [Option<Bound>],
    (span, arg): (Span, usize),
    strict: bool,
) -> Result<(), Diag> {
    let mut go = |p, a| unify(p, a, (written, sig), binds, (span, arg), strict);
    match (p, a) {
        (_, Ty::Never) => Ok(()),
        (Ty::Param(t), a) => {
            let i = sig
                .tparams
                .iter()
                .position(|x| x == t)
                .expect("a type parameter of the callee");
            match &binds[i] {
                Some(b) if b.ty == *a || !strict => Ok(()),
                Some(b) => {
                    let first = match b.arg {
                        0 => "from the expected type".to_string(),
                        _ => "here".to_string(),
                    };
                    let msg = format!("`{t}` is {} {first} but {a} at {}", b.ty, sig.part(arg));
                    let hint = format!("`{written}` is declared as `{}`", sig.decl);
                    Err(err(b.at, msg, Some(&hint)))
                }
                None if cap_in(a).is_some() => Err(cap_type_arg(span, t, a)),
                None => {
                    binds[i] = Some(Bound {
                        ty: a.clone(),
                        at: span,
                        arg,
                    });
                    Ok(())
                }
            }
        }
        (Ty::Option(p), Ty::Option(a)) | (Ty::List(p), Ty::List(a)) => go(p, a),
        (Ty::Result(p, q), Ty::Result(a, b))
        | (Ty::Map(p, q), Ty::Map(a, b))
        | (Ty::HashMap(p, q), Ty::HashMap(a, b)) => {
            go(p, a)?;
            go(q, b)
        }
        (Ty::Fn(ps, r), Ty::Fn(qs, s)) if ps.len() == qs.len() => {
            for (p, q) in ps.iter().zip(qs) {
                go(p, q)?;
            }
            go(r, s)
        }
        (Ty::Struct(n, ps), Ty::Struct(m, qs)) | (Ty::Enum(n, ps), Ty::Enum(m, qs)) if n == m => {
            for (p, q) in ps.iter().zip(qs) {
                go(p, q)?;
            }
            Ok(())
        }
        // Without a type parameter, the caller compares the whole types.
        _ if !strict || !has_param(p) => Ok(()),
        _ => {
            let msg = format!("expected {p}, found {a}");
            let hint = format!("`{written}` is declared as `{}`", sig.decl);
            Err(err(span, msg, Some(&hint)))
        }
    }
}

/// The type of each type parameter, or "cannot infer" at `span` for the first unbound one;
/// `call` says whether the hint shows a call or a value.
fn all_bound(span: Span, g: Generic, binds: &[Option<Bound>], call: bool) -> Result<Vec<Ty>, Diag> {
    let tys: Option<Vec<Ty>> = binds.iter().map(|b| b.as_ref().map(|b| b.ty.clone())).collect();
    tys.ok_or_else(|| cannot_infer(span, g, binds, call))
}

fn cannot_infer(span: Span, (written, sig): Generic, binds: &[Option<Bound>], call: bool) -> Diag {
    let i = binds.iter().position(Option::is_none).unwrap_or(0);
    if let Some((what, example)) = &sig.infer {
        let msg = format!("cannot infer `{}` for `{what}`", sig.tparams[i]);
        return err(span, msg, Some(&format!("add a type: `{example}`")));
    }
    let placeholders = vec!["Int"; sig.tparams.len()].join(", ");
    let args = if call { "(..)" } else { "" };
    err(
        span,
        format!("cannot infer `{}`", sig.tparams[i]),
        Some(&format!("write `{written}::<{placeholders}>{args}`")),
    )
}

/// Rule 1 for type arguments: type parameter `t` cannot be `ty`, which holds a capability.
fn cap_type_arg(span: Span, t: &str, ty: &Ty) -> Diag {
    let cap = cap_in(ty).unwrap_or("Dir");
    err(
        span,
        format!("`{t}` cannot be `{ty}`: capabilities are parameters only"),
        Some(&format!(
            "pass the {cap} to a function whose parameter has type `{cap}`"
        )),
    )
}

/// `==` (or `List.contains`) on values of `ty`, which mentions type parameter `t` without a
/// bound.
fn no_param_equality(span: Span, ty: &Ty, t: &str) -> Diag {
    err(
        span,
        format!("cannot compare `{ty}` values"),
        Some(&format!("add the bound: `{t}: Eq`")),
    )
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
            let TypeKind::Named(n, args) = &inner.kind else {
                return Err(nested_path(t.span));
            };
            let k = scope.member(m, n, t.span)?;
            if let Some(s) = structs.get(&k) {
                scope.public(&k, s.public, "struct", t.span)?;
            }
            if let Some(e) = enums.get(&k) {
                scope.public(&k, e.public, "enum", t.span)?;
            }
            if let Some(ty) = user_type(t, &k, args, caps, structs, enums, scope)? {
                return Ok(ty);
            }
            let names = [sorted_names(structs, m), sorted_names(enums, m)].concat();
            Err(err(
                t.span,
                format!("unknown type `{k}`"),
                Some(&suggest(n, &names, "types")),
            ))
        }
        TypeKind::Named(n, args) => match (n.as_str(), args.as_slice()) {
            (_, []) if scope.is_tparam(n) => Ok(Ty::Param(n.clone())),
            (_, _) if scope.is_tparam(n) => {
                let msg = format!("type parameter `{n}` takes no type arguments");
                Err(err(t.span, msg, Some(&format!("write `{n}`"))))
            }
            ("Option", [a]) => Ok(Ty::Option(Box::new(resolve(a, caps, structs, enums, scope)?))),
            ("Result", [a, b]) => {
                let (a, b) = (
                    resolve(a, caps, structs, enums, scope)?,
                    resolve(b, caps, structs, enums, scope)?,
                );
                Ok(Ty::Result(Box::new(a), Box::new(b)))
            }
            ("List", [a]) => Ok(Ty::List(Box::new(resolve(a, caps, structs, enums, scope)?))),
            ("Map" | "HashMap", [k, v]) => {
                let key = resolve(k, caps, structs, enums, scope)?;
                if !matches!(key, Ty::Int | Ty::Str) {
                    let msg = format!("`{key}` cannot be a {n} key");
                    return Err(err(k.span, msg, Some(&format!("{n} keys must be Int or Str"))));
                }
                let (k, v) = (Box::new(key), Box::new(resolve(v, caps, structs, enums, scope)?));
                Ok(if n == "Map" { Ty::Map(k, v) } else { Ty::HashMap(k, v) })
            }
            ("Option" | "Result" | "List" | "Map" | "HashMap", _) => {
                let (count, hint) = match n.as_str() {
                    "Option" => (1, "write `Option<T>`"),
                    "Result" => (2, "write `Result<T, E>`"),
                    "Map" => (2, "write `Map<K, V>`"),
                    "HashMap" => (2, "write `HashMap<K, V>`"),
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
            _ => {
                let k = scope.own(caps, n);
                if let Some(ty) = user_type(t, &k, args, caps, structs, enums, scope)? {
                    return Ok(ty);
                }
                let mut d = unknown_type(t);
                if !scope.module.is_empty() && (structs.contains_key(n) || enums.contains_key(n)) {
                    d.hint = Some(ROOT_ITEM.to_string());
                }
                Err(d)
            }
        },
    }
}

/// Struct or enum `k`, written at `t` with type arguments `args`, each of which it must be
/// given, none of which may hold a capability (rule 1), and each of which satisfies its
/// parameter's bound once `scope.types` is known; `None` if there is no such type.
fn user_type(
    t: &Type,
    k: &str,
    args: &[Type],
    caps: &CapTable,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
    scope: Scope,
) -> Result<Option<Ty>, Diag> {
    let (kw, tparams) = match (structs.get(k), enums.get(k)) {
        (Some(s), _) => ("struct", &s.tparams),
        (_, Some(e)) => ("enum", &e.tparams),
        _ => return Ok(None),
    };
    let name = bare(k);
    if args.len() != tparams.len() {
        let msg = format!("`{name}` takes {} type arguments, found {}", tparams.len(), args.len());
        let hint = match tparams.is_empty() {
            true => format!("write `{name}`"),
            false => format!("`{name}` is declared as `{kw} {name}<{}>`", tparams_text(tparams)),
        };
        return Err(err(t.span, msg, Some(&hint)));
    }
    let mut tys = Vec::new();
    for (a, p) in args.iter().zip(tparams) {
        let ty = resolve(a, caps, structs, enums, scope)?;
        if cap_in(&ty).is_some() {
            return Err(cap_type_arg(a.span, &p.name, &ty));
        }
        if let (Some(b), Some((fields, variants))) = (p.bound, scope.types) {
            if let Some(why) = unmet(&ty, b, scope.tparams, fields, variants) {
                return Err(bound_err(a.span, &ty, (name, &p.name, b), &why));
            }
        }
        tys.push(ty);
    }
    Ok(Some(match kw {
        "struct" => Ty::Struct(k.to_string(), tys),
        _ => Ty::Enum(k.to_string(), tys),
    }))
}

fn unknown_type(t: &Type) -> Diag {
    let TypeKind::Named(name, _) = &t.kind else {
        unreachable!("only named types can be unknown");
    };
    // Rust names agents reach for, mapped to the kio type.
    let hint = match name.as_str() {
        "String" | "str" | "char" => "use `Str`".to_string(),
        "Vec" => "use `List<T>`".to_string(),
        "BTreeMap" => "use `Map<K, V>`".to_string(),
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => "use `Int`".to_string(),
        "f32" | "f64" => "use `Float`".to_string(),
        "bool" => "use `Bool`".to_string(),
        _ => {
            "types are Int, Float, Bool, Str, Bytes, (), Option, Result, List, Map, HashMap and your structs and enums"
                .to_string()
        }
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
        Ty::Result(t, e) | Ty::Map(t, e) | Ty::HashMap(t, e) => cap_in(t).or_else(|| cap_in(e)),
        Ty::Fn(ps, r) => ps.iter().find_map(cap_in).or_else(|| cap_in(r)),
        // Unreachable: no type argument holds a capability; checked again as a second wall.
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().find_map(cap_in),
        _ => None,
    }
}

/// Whether a `fn` type inside `ty` mentions a capability.
fn fn_takes_cap(ty: &Ty) -> bool {
    match ty {
        Ty::Fn(..) => cap_in(ty).is_some(),
        Ty::Option(t) | Ty::List(t) => fn_takes_cap(t),
        Ty::Result(t, e) | Ty::Map(t, e) | Ty::HashMap(t, e) => fn_takes_cap(t) || fn_takes_cap(e),
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(fn_takes_cap),
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

/// The most parameters a function takes, captures included for a closure: Wasm allows 1,000,
/// and a function used as a value takes one more, itself.
const MAX_PARAMS: usize = 999;

fn too_many_params(span: Span, what: String) -> Diag {
    let msg = format!("{what}; at most {MAX_PARAMS}");
    err(span, msg, Some("pass a struct instead"))
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
    if let Some(p) = f.params.get(MAX_PARAMS) {
        let what = format!("`{}` takes {} parameters", f.name, f.params.len());
        return Err(too_many_params(p.span, what));
    }
    if let (true, Some(t)) = (scope.module.is_empty() && f.name == "main", f.tparams.first()) {
        let hint = "remove `<..>`: `main` takes only capabilities";
        return Err(err(t.span, "`main` cannot be generic".to_string(), Some(hint)));
    }
    check_tparams(&f.tparams, caps, structs, enums, scope.module)?;
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
    let generic = match f.tparams.is_empty() {
        true => String::new(),
        false => format!("<{}>", tparams_text(&f.tparams)),
    };
    Ok(Sig {
        public: f.public,
        decl: format!("fn {}{generic}({})", f.name, typed.join(", ")),
        tparams: f.tparams.iter().map(|t| t.name.clone()).collect(),
        bounds: f.tparams.iter().map(|t| t.bound).collect(),
        params,
        ret,
        parts: None,
        infer: None,
    })
}

/// The type parameters of a function, struct or enum in `module`: uppercase, distinct, and
/// not named like a type or a built-in constructor.
fn check_tparams(
    tparams: &[TParam],
    caps: &CapTable,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
    module: &str,
) -> Result<(), Diag> {
    for (i, TParam { name: t, span, .. }) in tparams.iter().enumerate() {
        if !t.starts_with(|c: char| c.is_ascii_uppercase()) {
            let fixed = t.to_ascii_uppercase();
            let msg = format!("type parameters start with an uppercase letter: `{t}`");
            return Err(err(*span, msg, Some(&format!("rename it `{fixed}`"))));
        }
        if tparams[..i].iter().any(|u| u.name == *t) {
            let msg = format!("duplicate type parameter `{t}`");
            return Err(err(*span, msg, Some("rename one of them")));
        }
        let is_type = ["Int", "Float", "Bool", "Str", "Bytes", "List"].contains(&t.as_str()) || reserved(caps, t);
        if is_type {
            let msg = format!("`{t}` is a built-in type");
            return Err(err(*span, msg, Some("name type parameters `T`, `U`, ..")));
        }
        if builtin(t).is_some() {
            let msg = format!("`{t}` is a built-in constructor");
            return Err(err(*span, msg, Some("name type parameters `T`, `U`, ..")));
        }
        let k = key(module, t);
        let kind = (structs.contains_key(&*k).then_some("struct")).or(enums.contains_key(&*k).then_some("enum"));
        if let Some(kind) = kind {
            let msg = format!("type parameter `{t}` has the name of {kind} `{t}`");
            return Err(err(*span, msg, Some("rename the type parameter")));
        }
    }
    Ok(())
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

/// The enum's name and variants that `ty` matches on, with its type arguments in place;
/// empty for other types.
pub(crate) fn shape(variants: &Variants, tparams: &TypeParams, ty: &Ty) -> Shape {
    let some = |t: &Ty| vec![("None".to_string(), vec![]), ("Some".to_string(), vec![t.clone()])];
    match ty {
        Ty::Enum(en, _) => {
            let binds = type_binds(tparams, ty);
            let vs = variants[en]
                .iter()
                .map(|(v, ts)| (v.clone(), ts.iter().map(|t| subst(t, &binds)).collect()));
            (en.clone(), vs.collect())
        }
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
        (Ty::Int | Ty::Bool | Ty::Bytes, "to_str") => Some((vec![], Ty::Str)),
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
        (Ty::Map(..) | Ty::HashMap(..), "len") => Some((vec![], Ty::Int)),
        (Ty::Map(k, _) | Ty::HashMap(k, _), "keys") => Some((vec![], Ty::List(k.clone()))),
        (Ty::Map(k, v) | Ty::HashMap(k, v), "get") => Some((vec![(**k).clone()], Ty::Option(v.clone()))),
        (Ty::Map(k, v) | Ty::HashMap(k, v), "insert") => Some((vec![(**k).clone(), (**v).clone()], Ty::Unit)),
        (Ty::Map(k, _) | Ty::HashMap(k, _), "remove") => Some((vec![(**k).clone()], Ty::Unit)),
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
        (Ty::List(t), "slice") => Some((vec![Ty::Int, Ty::Int], Ty::List(t.clone()))),
        (Ty::List(_), "reverse") => Some((vec![], Ty::Unit)),
        (Ty::List(_), "join") => Some((vec![Ty::Str], Ty::Str)),
        (Ty::Int, "min" | "max") => Some((vec![Ty::Int], Ty::Int)),
        (Ty::Float, "min" | "max") => Some((vec![Ty::Float], Ty::Float)),
        (Ty::Str, "to_lowercase" | "to_uppercase") => Some((vec![], Ty::Str)),
        (Ty::Bytes, "extend") => Some((vec![Ty::Bytes], Ty::Unit)),
        (Ty::Bytes | Ty::Str, "sha256") => Some((vec![], Ty::Bytes)),
        (Ty::Bytes, "to_hex") => Some((vec![], Ty::Str)),
        (Ty::Bytes, "ct_eq") => Some((vec![Ty::Bytes], Ty::Bool)),
        (Ty::Str, "ct_eq") => Some((vec![Ty::Str], Ty::Bool)),
        (Ty::Str, "parse_json") => Some((vec![], Ty::Result(Box::new(json()), Box::new(Ty::Str)))),
        (Ty::Enum(k, _), _) if k == "Json" => {
            let opt = |t: Ty| Ty::Option(Box::new(t));
            match name {
                "to_json" => Some((vec![], Ty::Str)),
                "get" => Some((vec![Ty::Str], opt(json()))),
                "at" => Some((vec![Ty::Int], opt(json()))),
                "as_str" => Some((vec![], opt(Ty::Str))),
                "as_int" => Some((vec![], opt(Ty::Int))),
                "as_float" => Some((vec![], opt(Ty::Float))),
                "as_bool" => Some((vec![], opt(Ty::Bool))),
                "as_list" => Some((vec![], opt(Ty::List(Box::new(json()))))),
                "as_map" => Some((vec![], opt(Ty::Map(Box::new(Ty::Str), Box::new(json()))))),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The built-in `Json` enum's type.
fn json() -> Ty {
    Ty::Enum("Json".into(), Vec::new())
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
            | (
                Ty::List(_),
                "map" | "filter" | "any" | "all" | "sort_by" | "sort_by_key"
            )
            | (Ty::Option(_) | Ty::Result(..), "unwrap_or_else")
    )
}

/// How a method that takes a function is declared, as a hint; `U` and `F` are the function's result.
fn fn_declared(recv: &Ty, name: &str) -> String {
    let decl = match (recv, name) {
        (Ty::Option(t), "unwrap_or_else") => format!("unwrap_or_else(fn() -> {t}) -> {t}"),
        (Ty::Result(t, e), "unwrap_or_else") => format!("unwrap_or_else(fn({e}) -> {t}) -> {t}"),
        (Ty::List(t), "sort_by") => format!("sort_by(fn({t}, {t}) -> Bool)"),
        (Ty::List(t), "sort_by_key") => format!("sort_by_key(fn({t}) -> K)"),
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
    "as_bool",
    "as_float",
    "as_int",
    "as_list",
    "as_map",
    "as_str",
    "at",
    "ceil",
    "contains",
    "ct_eq",
    "ends_with",
    "err",
    "extend",
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
    "join",
    "keys",
    "len",
    "lines",
    "map",
    "map_err",
    "max",
    "min",
    "ok",
    "ok_or",
    "parse_float",
    "parse_int",
    "parse_json",
    "pop",
    "push",
    "remove",
    "replace",
    "reverse",
    "round",
    "sha256",
    "slice",
    "sort",
    "sort_by",
    "sort_by_key",
    "split",
    "sqrt",
    "starts_with",
    "strip_prefix",
    "strip_suffix",
    "to_bytes",
    "to_float",
    "to_hex",
    "to_int",
    "to_json",
    "to_lowercase",
    "to_str",
    "to_uppercase",
    "trim",
    "unwrap_or",
    "unwrap_or_else",
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
fn named(ty: &Ty) -> Vec<&Ty> {
    match ty {
        Ty::Struct(..) | Ty::Enum(..) => vec![ty],
        Ty::Option(t) => named(t),
        Ty::Result(t, e) => [named(t), named(e)].concat(),
        _ => Vec::new(),
    }
}

/// Whether type `name` reaches itself, with any type arguments, through its field or payload
/// types, each with the type arguments of the instance it is in (`struct X { w: W<X> }` with
/// `struct W<T> { t: T }`). Its own type parameters are opaque, so this holds for every
/// instance; any other instance it reaches inline is finite or is that type's own error, so
/// a type argument larger than `MAX_TYPE_ARG` ends the walk.
fn contains_itself(name: &str, fields: &Fields, variants: &Variants, tparams: &TypeParams) -> bool {
    // A type whose declaration has errors is left out.
    let known = |k: &str| fields.contains_key(k) || variants.contains_key(k);
    if !known(name) {
        return false;
    }
    let own = |ps: Option<&Vec<String>>| ps.into_iter().flatten().map(|t| Ty::Param(t.clone())).collect();
    let mut todo = vec![match fields.contains_key(name) {
        true => Ty::Struct(name.to_string(), own(tparams.get(name))),
        false => Ty::Enum(name.to_string(), own(tparams.get(name))),
    }];
    let mut seen = HashSet::new();
    while let Some(ty) = todo.pop() {
        let parts = match &ty {
            Ty::Struct(..) => fields_of(fields, tparams, &ty).into_iter().map(|(_, t)| t).collect(),
            _ => shape(variants, tparams, &ty)
                .1
                .into_iter()
                .flat_map(|(_, ts)| ts)
                .collect::<Vec<_>>(),
        };
        for part in &parts {
            for m in named(part) {
                let (Ty::Struct(k, _) | Ty::Enum(k, _)) = m else {
                    unreachable!("named");
                };
                if k == name {
                    return true;
                }
                if known(k) && ty_size(m) <= MAX_TYPE_ARG && seen.insert(m.clone()) {
                    todo.push(m.clone());
                }
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

/// Whether a value of `ty` can hold a function. `seen` holds the user types already walked. A
/// generic type holds one if its declaration does (its type parameters opaque) or one of its
/// type arguments does, even one no field uses.
fn has_fn<'a>(ty: &'a Ty, fields: &'a Fields, variants: &'a Variants, seen: &mut HashSet<&'a str>) -> bool {
    let mut walk = |t| has_fn(t, fields, variants, seen);
    match ty {
        Ty::Fn(..) => true,
        Ty::Option(t) | Ty::List(t) => walk(t),
        Ty::Result(t, e) | Ty::Map(t, e) | Ty::HashMap(t, e) => walk(t) || walk(e),
        Ty::Struct(_, args) | Ty::Enum(_, args) if args.iter().any(&mut walk) => true,
        Ty::Struct(n, _) => seen.insert(n) && fields[n].iter().any(|(_, t)| has_fn(t, fields, variants, seen)),
        Ty::Enum(n, _) => {
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

/// The type of constant `c`, which `check_consts` accepted.
fn const_ty(c: &Const) -> Ty {
    match &c.ty.kind {
        TypeKind::Named(n, _) if n == "Int" => Ty::Int,
        TypeKind::Named(n, _) if n == "Float" => Ty::Float,
        TypeKind::Named(n, _) if n == "Bool" => Ty::Bool,
        _ => Ty::Str,
    }
}

/// The constants by key: each a literal of its type, named by no other function, type or
/// constant of its module. Errors go to `diags`.
fn check_consts(
    consts: Vec<(&str, Const)>,
    sigs: &HashMap<String, Sig>,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
    diags: &mut Vec<Diag>,
) -> HashMap<String, Const> {
    let mut out = HashMap::new();
    for (module, c) in consts {
        let k = key(module, &c.name).into_owned();
        let used = match () {
            _ if out.contains_key(&k) => Some("another constant"),
            _ if sigs.contains_key(&k) => Some("a function"),
            _ if structs.contains_key(&k) || enums.contains_key(&k) => Some("a type"),
            _ if builtin(&c.name).is_some() => Some("a built-in variant"),
            _ => None,
        };
        if let Some(what) = used {
            let msg = format!("the name `{}` is already used by {what}", c.name);
            diags.push(err(c.span, msg, Some("rename one of them")));
            continue;
        }
        let scalar = |n: &str| matches!(n, "Int" | "Float" | "Bool" | "Str");
        if !matches!(&c.ty.kind, TypeKind::Named(n, a) if a.is_empty() && scalar(n)) {
            let msg = "a constant is an `Int`, `Float`, `Bool` or `Str`".to_string();
            let hint = "use a function for other values: `fn name() -> T { .. }`";
            diags.push(err(c.ty.span, msg, Some(hint)));
            continue;
        }
        // A bad value is reported, and the constant still has its declared type for its uses.
        let found = match &c.value.kind {
            ExprKind::Int(_) => Ok(Ty::Int),
            ExprKind::Float(_) => Ok(Ty::Float),
            ExprKind::Bool(_) => Ok(Ty::Bool),
            ExprKind::Str(_) => Ok(Ty::Str),
            ExprKind::Unary("-", x) if matches!(x.kind, ExprKind::Int(_)) => Ok(Ty::Int),
            ExprKind::Unary("-", x) if matches!(x.kind, ExprKind::Float(_)) => Ok(Ty::Float),
            _ => {
                let msg = "a constant's value must be a literal".to_string();
                let hint = "write the value, like `64`, or compute it in a function: `fn name() -> T { .. }`";
                Err(err(c.value.span, msg, Some(hint)))
            }
        };
        diags.extend(found.and_then(|t| want(c.value.span, &t, &const_ty(&c))).err());
        out.insert(k, c);
    }
    out
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
    let mut consts = Vec::new();
    let mut structs = HashMap::new();
    let mut enums = HashMap::new();
    let mut order = Vec::new();
    for e in caps.enums.iter().cloned().chain([crate::json::decl()]) {
        order.push(e.name.clone());
        enums.insert(e.name.clone(), e);
    }
    for (i, items) in parsed.into_iter().enumerate() {
        let module = if i == 0 { "" } else { names[i] };
        for item in items {
            let declared = match &item {
                Item::Struct(s) => Some((s.name.as_str(), s.span)),
                Item::Enum(e) => Some((e.name.as_str(), e.span)),
                Item::Fn(_) | Item::Mod(..) | Item::Const(_) => None,
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
                Item::Const(c) => consts.push((module, c)),
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
    let mut tparams = TypeParams::new();
    for name in &order {
        let own = match structs.get(name) {
            Some(s) => &s.tparams,
            None => &enums[name].tparams,
        };
        tparams.insert(name.clone(), own.iter().map(|t| t.name.clone()).collect());
        let scope = Scope {
            module: module_of(name),
            mods: &mods,
            tparams: own,
            types: None,
        };
        let resolved =
            check_tparams(own, caps, &structs, &enums, scope.module).and_then(|()| match structs.get(name) {
                Some(s) => {
                    struct_fields(s, caps, &structs, &enums, scope).map(|f| drop(fields.insert(name.clone(), f)))
                }
                None => enum_variants(&enums[name], caps, &structs, &enums, scope)
                    .map(|v| drop(variants.insert(name.clone(), v))),
            });
        diags.extend(resolved.err());
    }
    for name in &order {
        if contains_itself(name, &fields, &variants, &tparams) {
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
    // The bounds of the type arguments fields and payloads name, now that `has_fn` can walk
    // every type: the same types resolved again.
    for name in &order {
        let scope = Scope {
            module: module_of(name),
            mods: &mods,
            tparams: structs.get(name).map_or_else(|| &enums[name].tparams, |s| &s.tparams),
            types: Some((&fields, &variants)),
        };
        let resolved = match structs.get(name) {
            Some(s) => struct_fields(s, caps, &structs, &enums, scope).map(drop),
            None => enum_variants(&enums[name], caps, &structs, &enums, scope).map(drop),
        };
        diags.extend(resolved.err());
    }
    if !diags.is_empty() {
        return Err(diags);
    }

    // Signatures first, so call order in the file does not matter.
    let mut sigs = HashMap::new();
    let mut sig_errs = Vec::new();
    for (module, f) in &fns {
        let scope = Scope {
            module,
            mods: &mods,
            tparams: &f.tparams,
            types: Some((&fields, &variants)),
        };
        match signature(f, &sigs, caps, &structs, &enums, scope) {
            Ok(sig) => {
                sigs.insert(key(module, &f.name).into_owned(), sig);
                sig_errs.push(None);
            }
            Err(d) => sig_errs.push(Some(d)),
        }
    }

    let consts = check_consts(consts, &sigs, &structs, &enums, &mut diags);

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
    let mut uses = HashMap::new();
    let mut seeds = HashMap::new();
    for ((module, f), sig_err) in fns.iter().zip(sig_errs) {
        let mut c = Checker {
            caps,
            sigs: &sigs,
            fields: &fields,
            variants: &variants,
            tparams: &tparams,
            structs: &structs,
            enums: &enums,
            consts: &consts,
            home: Scope {
                module,
                mods: &mods,
                tparams: &f.tparams,
                types: Some((&fields, &variants)),
            },
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
            uses: Vec::new(),
        };
        diags.extend(sig_err.or_else(|| c.function(f).err()));
        // The types this function's lowering names that are or may become (by substituting
        // a type parameter) instances of generic types: of its expressions and `let`s, and
        // of its parameters and return type.
        let mut own: Vec<(Span, Ty)> = (c.types.iter())
            .filter(|(_, t)| has_app(t) || has_param(t))
            .map(|(s, t)| (*s, t.clone()))
            .collect();
        own.sort_by_key(|(s, _)| (s.file, s.start, s.end));
        if let Some(sig) = sigs.get(&*key(module, &f.name)) {
            let decl = f.params.iter().map(|p| p.ty.span).chain([f.ret.span]);
            own.extend(
                decl.zip(sig.params.iter().chain([&sig.ret]))
                    .map(|(s, t)| (s, t.clone())),
            );
        }
        seeds.insert(key(module, &f.name).into_owned(), own);
        types.extend(c.types);
        captures.extend(c.captures);
        uses.insert(key(module, &f.name).into_owned(), c.uses);
    }
    if !fns.iter().any(|(m, f)| m.is_empty() && f.name == "main") {
        let hint = "add `fn main() { .. }`";
        diags.push(err(Span::default(), "no `main` function".to_string(), Some(hint)));
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    let mut insts = TypeInsts {
        fields: &fields,
        variants: &variants,
        tparams: &tparams,
        seen: HashMap::new(),
    };
    // The types every lowering names: those of the types that are not generic.
    for name in &order {
        if !tparams[name].is_empty() {
            continue;
        }
        let span = structs.get(name).map_or_else(|| enums[name].span, |s| s.span);
        let parts = match fields.get(name) {
            Some(fs) => fs.iter().map(|(_, t)| t).collect(),
            None => variants[name].iter().flat_map(|(_, ts)| ts).collect::<Vec<_>>(),
        };
        for t in parts {
            insts.add(t, span, &structs, &enums).map_err(|d| vec![d])?;
        }
    }
    instantiate(&sigs, &uses, &seeds, &mut insts, &structs, &enums).map_err(|d| vec![d])?;
    let instances = (uses.into_values().flatten())
        .map(|(span, g, args)| (span, (g, args)))
        .collect();
    let mut p = Program {
        funcs: fns
            .into_iter()
            .map(|(m, f)| (key(m, &f.name).into_owned(), f))
            .collect(),
        structs,
        enums,
        consts,
        variants,
        fields,
        tparams,
        sigs,
        types,
        captures,
        instances,
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

/// A use of a generic function in a function's body: the span naming it, its key and its type
/// arguments, which may hold the enclosing function's type parameters.
type Use = (Span, String, Vec<Ty>);

/// Whether `ty` mentions a generic struct or enum.
fn has_app(ty: &Ty) -> bool {
    let mut found = Vec::new();
    apps(ty, &mut found);
    !found.is_empty()
}

/// The generic struct and enum types inside `ty`, outermost first.
fn apps<'t>(ty: &'t Ty, out: &mut Vec<&'t Ty>) {
    match ty {
        Ty::Struct(_, args) | Ty::Enum(_, args) => {
            if !args.is_empty() {
                out.push(ty);
            }
            args.iter().for_each(|a| apps(a, out));
        }
        Ty::Option(t) | Ty::List(t) => apps(t, out),
        Ty::Result(t, e) | Ty::Map(t, e) | Ty::HashMap(t, e) => {
            apps(t, out);
            apps(e, out);
        }
        Ty::Fn(ps, r) => {
            ps.iter().for_each(|p| apps(p, out));
            apps(r, out);
        }
        _ => {}
    }
}

/// The error for a type argument over `MAX_TYPE_ARG` or a 65th instance, of generic function
/// or type `name`, used directly in code that is not generic.
fn too_many(span: Span, name: &str, args: &[Ty]) -> Diag {
    if let Some(t) = args.iter().find(|t| ty_size(t) > MAX_TYPE_ARG) {
        let msg = format!(
            "a type argument of `{name}` is too large: {} types, at most {MAX_TYPE_ARG}",
            ty_size(t)
        );
        return err(span, msg, Some("name a struct for part of it"));
    }
    let msg = format!("`{name}` has more than {MAX_INSTANCES} instances");
    let hint = "use fewer distinct type arguments, or a copy that is not generic for some uses";
    err(span, msg, Some(hint))
}

/// The instances of generic types lowering will make, found from the types it names.
struct TypeInsts<'a> {
    fields: &'a Fields,
    variants: &'a Variants,
    tparams: &'a TypeParams,
    seen: HashMap<String, HashSet<Vec<Ty>>>,
}

impl TypeInsts<'_> {
    /// Adds the instances in `ty`, a type named at `span`, and those their fields name. A
    /// generic type with more than `MAX_INSTANCES` instances or a type argument larger than
    /// `MAX_TYPE_ARG` is refused: at the declaration whose fields keep growing (`struct S<T>
    /// { s: List<S<List<T>>> }`), else at `span`, where `ty` names it or a type holding it.
    fn add(
        &mut self,
        ty: &Ty,
        span: Span,
        structs: &HashMap<String, Struct>,
        enums: &HashMap<String, Enum>,
    ) -> Result<(), Diag> {
        // Each type to visit with the index in `owners` of the instance whose field holds it;
        // `owners` holds each owner's key and its own owner's index.
        let mut owners: Vec<(String, Option<usize>)> = Vec::new();
        let mut todo: Vec<(Ty, Option<usize>)> = vec![(ty.clone(), None)];
        while let Some((ty, from)) = todo.pop() {
            let mut found = Vec::new();
            apps(&ty, &mut found);
            for app in found {
                let (Ty::Struct(k, args) | Ty::Enum(k, args)) = app else {
                    unreachable!("apps");
                };
                let instances = self.seen.entry(k.clone()).or_default();
                if instances.contains(args) {
                    continue;
                }
                if instances.len() == MAX_INSTANCES || args.iter().any(|t| ty_size(t) > MAX_TYPE_ARG) {
                    // The owner grows if the chain of owners from `span` to `k` holds a type twice.
                    let mut chain = HashSet::from([k.as_str()]);
                    let mut up = from;
                    let mut grows = false;
                    while let Some(i) = up {
                        grows |= !chain.insert(owners[i].0.as_str());
                        up = owners[i].1;
                    }
                    let (Some(i), true) = (from, grows) else {
                        return Err(too_many(span, bare(k), args));
                    };
                    let owner = owners[i].0.as_str();
                    let at = structs.get(owner).map_or_else(|| enums[owner].span, |s| s.span);
                    let msg = format!("generic type `{}` instantiates without end", bare(owner));
                    let hint = "a field or payload holds this type with larger type arguments; keep them the same";
                    return Err(err(at, msg, Some(hint)));
                }
                instances.insert(args.clone());
                let parts: Vec<Ty> = match app {
                    Ty::Struct(..) => fields_of(self.fields, self.tparams, app)
                        .into_iter()
                        .map(|(_, t)| t)
                        .collect(),
                    _ => (shape(self.variants, self.tparams, app).1.into_iter())
                        .flat_map(|(_, ts)| ts)
                        .collect(),
                };
                owners.push((k.clone(), from));
                let at = Some(owners.len() - 1);
                todo.extend(parts.into_iter().map(|t| (t, at)));
            }
        }
        Ok(())
    }
}

/// Finds every instance lowering will make, from the functions that are not generic, as
/// lowering does, and refuses a generic function with more than `MAX_INSTANCES` of them or a
/// type argument larger than `MAX_TYPE_ARG`: `f<T>` calling `f<List<T>>` never ends. Adds the
/// generic types each instance names (`seeds`, by function) to `insts`.
fn instantiate(
    sigs: &HashMap<String, Sig>,
    uses: &HashMap<String, Vec<Use>>,
    seeds: &HashMap<String, Vec<(Span, Ty)>>,
    insts: &mut TypeInsts,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
) -> Result<(), Diag> {
    let mut seen: HashMap<&str, HashSet<Vec<Ty>>> = HashMap::new();
    let mut todo: Vec<(&str, Vec<(String, Ty)>)> = (uses.keys())
        .filter(|k| sigs[k.as_str()].tparams.is_empty())
        .map(|k| (k.as_str(), Vec::new()))
        .collect();
    // Popped in name order, so the error is the same on every run.
    todo.sort_by(|a, b| b.0.cmp(a.0));
    while let Some((f, binds)) = todo.pop() {
        for (span, t) in &seeds[f] {
            insts.add(&subst(t, &binds), *span, structs, enums)?;
        }
        for (span, g, args) in &uses[f] {
            let args: Vec<Ty> = args.iter().map(|t| subst(t, &binds)).collect();
            let instances = seen.entry(g).or_default();
            if instances.contains(&args) {
                continue;
            }
            if instances.len() == MAX_INSTANCES || args.iter().any(|t| ty_size(t) > MAX_TYPE_ARG) {
                let name = bare(g);
                if binds.is_empty() {
                    return Err(too_many(*span, name, &args));
                }
                let hint = format!(
                    "each call gives `{name}` larger type arguments; keep them the same in calls inside `{name}`"
                );
                let msg = "this generic function instantiates without end".to_string();
                return Err(err(*span, msg, Some(&hint)));
            }
            instances.insert(args.clone());
            let binds = sigs[g].tparams.iter().cloned().zip(args).collect();
            todo.push((g, binds));
        }
    }
    Ok(())
}

struct Checker<'a> {
    caps: &'a CapTable,
    sigs: &'a HashMap<String, Sig>,
    fields: &'a Fields,
    variants: &'a Variants,
    tparams: &'a TypeParams,
    structs: &'a HashMap<String, Struct>,
    enums: &'a HashMap<String, Enum>,
    consts: &'a HashMap<String, Const>,
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
    /// The uses of generic functions in this function, closures included.
    uses: Vec<Use>,
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
            ("Map" | "HashMap", _) => format!("the only {en} function is `{en}::new()`"),
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
            let hint = format!("use a named function that takes it as a parameter, `{name}: {ty}`");
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
            StmtKind::LetElse { pat, ty, init, els } => {
                let declared = ty
                    .as_ref()
                    .map(|t| resolve(t, self.caps, self.structs, self.enums, self.home));
                let st = self.scrutinee(init, declared.transpose()?.as_ref())?;
                // The pattern's names are not in scope in the else block.
                self.block(els, None)?;
                let leaves = els.tail.is_none()
                    && (els.stmts.last())
                        .is_some_and(|s| matches!(s.kind, StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue));
                if !leaves {
                    let msg = "the `else` block of `let`-else must leave the function or loop";
                    let hint = "end it with `return ..`: its last statement must be `return`, `break` or `continue`";
                    return Err(err(els.span, msg.to_string(), Some(hint)));
                }
                let mut binds = Vec::new();
                let cover = self.pattern(pat, &st, &mut binds)?;
                let missing = (self.usefulness())
                    .useful(&[vec![cover]], &[Cover::All], &[st])
                    .map_err(|spent| too_complex(init.span, spent))?;
                if missing.is_none() {
                    return Err(err(
                        pat.span,
                        "this pattern always matches".to_string(),
                        Some("write `let`"),
                    ));
                }
                let scope = self.scopes.last_mut().unwrap();
                for (name, ty) in binds {
                    scope.insert(name, Local { ty, mutable: false });
                }
                Ok(Ty::Unit)
            }
            StmtKind::Assign { target, op, value } => {
                let ty = self.place(target)?;
                let found = self.expr(value, (*op == "=").then_some(&ty))?;
                if *op == "=" {
                    want_expr(value, &found, &ty)?;
                } else if binary_ty(&op[..1], &ty, &found, self.home.tparams) != Some(ty.clone()) {
                    return Err(binary_err(
                        &op[..1],
                        op,
                        (&ty, &found),
                        self.home.tparams,
                        s.span,
                        (target, value),
                    ));
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
                    if let Some(c) = self.consts.get(&*k) {
                        return Ok(const_ty(c));
                    }
                    if self.sigs.contains_key(&*k) {
                        return self.fn_value(e.span, &k, None, expected);
                    }
                    let mut d = self.unknown_name(e.span, name);
                    let in_root = self.sigs.contains_key(name.as_str()) || self.consts.contains_key(name.as_str());
                    if !self.home.module.is_empty() && in_root {
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
                binary_ty(op, &l, &r, self.home.tparams)
                    .ok_or_else(|| binary_err(op, op, (&l, &r), self.home.tparams, e.span, (a, b)))
            }
            ExprKind::If(cond, then, els) => {
                want_expr(cond, &self.expr(cond, None)?, &Ty::Bool)?;
                let t = self.block(then, expected)?;
                self.else_branch(e, t, then, els.as_deref(), expected)
            }
            ExprKind::IfLet(pat, scrut, then, els) => {
                let st = self.scrutinee(scrut, None)?;
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
            ExprKind::StructLit(name, inits, base) => {
                self.struct_lit(e, &key(self.home.module, name), inits, base.as_deref(), expected)
            }
            ExprKind::Path(en, v) => self.variant_value(e, &self.enum_key(en), v, &[], expected),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::StructLit(name, inits, base) => {
                    let k = self.home.member(m, name, e.span)?;
                    if let Some(s) = self.structs.get(&k) {
                        self.home.public(&k, s.public, "struct", e.span)?;
                    }
                    self.struct_lit(e, &k, inits, base.as_deref(), expected)
                }
                ExprKind::Name(name) if self.consts.contains_key(&format!("{m}::{name}")) => {
                    let k = format!("{m}::{name}");
                    let c = &self.consts[&k];
                    self.home.public(&k, c.public, "const", e.span)?;
                    Ok(const_ty(c))
                }
                ExprKind::Path(en, v) => self.module_variant(e, m, en, v, &[], expected),
                ExprKind::Name(name) => {
                    let (k, _) = self.named_fn(e.span, name, Some(m))?;
                    self.fn_value(e.span, &k, None, expected)
                }
                ExprKind::Turbofish(name, tys) => {
                    let (k, _) = self.named_fn(e.span, name, Some(m))?;
                    self.fn_value(e.span, &k, Some(tys), expected)
                }
                _ => Err(nested_path(e.span)),
            },
            ExprKind::Turbofish(name, tys) => {
                let (k, _) = self.turbofish_fn(e.span, name)?;
                self.fn_value(e.span, &k, Some(tys), expected)
            }
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
            (
                Ty::List(_),
                "push" | "sort" | "pop" | "sort_by" | "sort_by_key" | "reverse"
            ) | (Ty::Bytes, "push" | "extend")
                | (Ty::Map(..) | Ty::HashMap(..), "insert" | "remove")
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
                Ty::Struct(..) => fields_of(self.fields, self.tparams, &recv_ty)
                    .iter()
                    .any(|(f, t)| f == name && matches!(t, Ty::Fn(..))),
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
            } else if let Ty::Param(t) = &recv_ty {
                format!(
                    "`{t}` is a type parameter, which has no methods; take a function for it, like `f: fn({t}) -> Str`"
                )
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
            if let Some(p) = unbounded(t, self.home.tparams) {
                return Err(no_param_equality(e.span, t, p));
            }
        }
        if let (Ty::List(t), "join") = (&recv_ty, name) {
            if **t != Ty::Str {
                let hint = "join works on List<Str>; make one first: `xs.map(|x| x.to_str()).join(sep)`";
                return Err(err(e.span, format!("cannot join {recv_ty}"), Some(hint)));
            }
        }
        if let (Ty::List(t), "sort") = (&recv_ty, name) {
            let hint = match &**t {
                Ty::Int | Ty::Str => None,
                Ty::Param(p) if bound_of(self.home.tparams, p) == Some(TBound::Ord) => None,
                Ty::Param(p) => Some(format!("add the bound: `{p}: Ord`")),
                _ => Some("sort works on List<Int> and List<Str>".to_string()),
            };
            if let Some(hint) = hint {
                return Err(err(e.span, format!("cannot sort {recv_ty}"), Some(&hint)));
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
        let params = match (recv_ty, name) {
            (Ty::Option(_), "unwrap_or_else") => vec![],
            (Ty::Result(_, err_ty), "map_err" | "unwrap_or_else") => vec![(**err_ty).clone()],
            (Ty::List(t), "sort_by") => vec![(**t).clone(), (**t).clone()],
            (Ty::Option(t) | Ty::Result(t, _) | Ty::List(t), _) => vec![(**t).clone()],
            _ => unreachable!("takes_fn"),
        };
        // The result a closure written here must have, when the method fixes it.
        let want_ret = match (recv_ty, name) {
            (Ty::Option(t) | Ty::Result(t, _), "unwrap_or_else") => Some((**t).clone()),
            (_, "sort_by") => Some(Ty::Bool),
            _ => None,
        };
        let shape = match (recv_ty, name) {
            (Ty::Option(_), "and_then") => Some(Ty::Option(Box::new(Ty::Never))),
            (Ty::Result(_, err_ty), "and_then") => Some(Ty::Result(Box::new(Ty::Never), err_ty.clone())),
            _ => None,
        };
        let n = params.len();
        let arity = match n {
            1 => "1 parameter".to_string(),
            n => format!("{n} parameters"),
        };
        let f = &args[0];
        let found = match &f.kind {
            ExprKind::Closure(ps, body) => {
                if ps.len() != n {
                    let msg = format!(
                        "`{name}` takes a function of {arity}, found a closure of {} parameters",
                        ps.len()
                    );
                    return Err(err(f.span, msg, Some(&decl)));
                }
                let want = (params.as_slice(), want_ret);
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
        if ps.len() != n {
            let msg = format!("`{name}` takes a function of {arity}, found {found}");
            return Err(err(f.span, msg, Some(&decl)));
        }
        if *ps != params {
            let tys: Vec<_> = params.iter().map(Ty::to_string).collect();
            let msg = format!("`{name}` needs a function taking {}, found {found}", tys.join(", "));
            return Err(err(f.span, msg, Some(&decl)));
        }
        let ret = ret.clone();
        let wrong_ret = |what: String| {
            let msg = format!("`{name}` needs a function returning {what}, found {found}");
            Err(err(f.span, msg, Some(&decl)))
        };
        match (recv_ty, name) {
            (_, "filter" | "any" | "all" | "sort_by") if *ret != Ty::Bool => wrong_ret("Bool".to_string()),
            (_, "sort_by") => Ok(Ty::Unit),
            (_, "sort_by_key") => match *ret {
                Ty::Int | Ty::Str => Ok(Ty::Unit),
                _ => wrong_ret("Int or Str".to_string()),
            },
            (Ty::Option(t) | Ty::Result(t, _), "unwrap_or_else") if ret != *t => wrong_ret(t.to_string()),
            (Ty::Option(t) | Ty::Result(t, _), "unwrap_or_else") => Ok((**t).clone()),
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

    /// `Map::new()` or `HashMap::new()` (`map` names which), which takes its type from
    /// `expected`.
    fn map_new(&self, e: &Expr, map: &str, args: &[Expr], expected: Option<&Ty>) -> Result<Ty, Diag> {
        if !args.is_empty() {
            let msg = format!("`{map}::new` takes 0 arguments, found {}", args.len());
            return Err(err(e.span, msg, Some(&format!("write `{map}::new()`"))));
        }
        match expected {
            Some(t @ Ty::Map(..)) if map == "Map" => Ok(t.clone()),
            Some(t @ Ty::HashMap(..)) if map == "HashMap" => Ok(t.clone()),
            Some(t) => {
                let msg = format!("expected {t}, found a {map}");
                Err(err(e.span, msg, Some(&format!("`{map}::new()` makes a `{map}<K, V>`"))))
            }
            None => {
                let hint = format!("add a type: `let m: {map}<Str, Int> = {map}::new();`");
                Err(err(
                    e.span,
                    format!("cannot tell the type of `{map}::new()`"),
                    Some(&hint),
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
            (Ty::Result(..), ret @ Ty::Result(..)) => {
                let msg = format!("`?` on {found} in a function returning {ret}");
                let hint = "change the return type, convert the error with `.map_err(f)?`, or `match` on the result";
                Err(err(e.span, msg, Some(hint)))
            }
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
    fn module_variant(
        &mut self,
        e: &Expr,
        m: &str,
        en: &str,
        v: &str,
        args: &[Expr],
        expected: Option<&Ty>,
    ) -> Result<Ty, Diag> {
        let k = self.home.member(m, en, e.span)?;
        if let Some(d) = self.enums.get(&k) {
            self.home.public(&k, d.public, "enum", e.span)?;
        }
        self.variant_value(e, &k, v, args, expected)
    }

    /// `E::V` or `E::V(args)`, with `en` the enum's key; arity and payload types must match
    /// the declaration. A generic enum's type arguments are inferred as a generic call's.
    fn variant_value(&mut self, e: &Expr, en: &str, v: &str, args: &[Expr], expected: Option<&Ty>) -> Result<Ty, Diag> {
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
        let tparams = &self.tparams[en];
        if tparams.is_empty() {
            for (a, ty) in args.iter().zip(payload) {
                want_expr(a, &self.expr(a, Some(ty))?, ty)?;
            }
            return Ok(Ty::Enum(en.to_string(), Vec::new()));
        }
        let name = self.written(en);
        let written = format!("{name}::{v}");
        let parts = (1..=args.len()).map(|i| format!("value {i}")).collect();
        let shown = if args.is_empty() {
            written.clone()
        } else {
            format!("{written}(..)")
        };
        let sig = self.type_sig("enum", en, payload.to_vec(), parts, &shown);
        let args: Vec<&Expr> = args.iter().collect();
        let tys = self.infer(e, e.span, (&written, &sig), None, &args, expected)?;
        Ok(Ty::Enum(en.to_string(), tys))
    }

    /// Type `k` as written in this module: without its module if it is this one's.
    fn written<'k>(&self, k: &'k str) -> &'k str {
        if module_of(k) == self.home.module {
            bare(k)
        } else {
            k
        }
    }

    /// The signature a literal or variant of generic type `k` (`kw` is `struct` or `enum`)
    /// is inferred with, as a call taking values of types `params`, named `parts` in
    /// messages; `shown` is the literal as written, for "cannot infer".
    fn type_sig(&self, kw: &str, k: &str, params: Vec<Ty>, parts: Vec<String>, shown: &str) -> Sig {
        let tparams = self.tparams[k].clone();
        let name = self.written(k);
        let local = bare(k).to_ascii_lowercase()[..1].to_string();
        let ints = vec!["Int"; tparams.len()].join(", ");
        let own = tparams.iter().map(|t| Ty::Param(t.clone())).collect();
        let declared = self
            .structs
            .get(k)
            .map_or_else(|| &self.enums[k].tparams, |s| &s.tparams);
        Sig {
            public: true,
            decl: format!("{kw} {name}<{}>", tparams_text(declared)),
            bounds: declared.iter().map(|t| t.bound).collect(),
            ret: match kw {
                "struct" => Ty::Struct(k.to_string(), own),
                _ => Ty::Enum(k.to_string(), own),
            },
            params,
            parts: Some(parts),
            infer: Some((shown.to_string(), format!("let {local}: {name}<{ints}> = {shown};"))),
            tparams,
        }
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
    /// `expected` is a `let`-else's declared type.
    fn scrutinee(&mut self, scrut: &Expr, expected: Option<&Ty>) -> Result<Ty, Diag> {
        let st = self.expr(scrut, expected)?;
        if let Some(t) = expected {
            want_expr(scrut, &st, t)?;
        }
        if st == Ty::Float {
            let hint = Some("compare with `<` or `==`");
            return Err(err(scrut.span, "cannot match on Float".to_string(), hint));
        }
        // A type parameter is matched only as a whole: by a name or `_`.
        if !matches!(
            st,
            Ty::Int | Ty::Bool | Ty::Str | Ty::Enum(..) | Ty::Option(_) | Ty::Result(..) | Ty::Param(_)
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
        let st = self.scrutinee(scrut, None)?;
        let too_complex = |spent| too_complex(scrut.span, spent);
        let mut u = self.usefulness();
        let tys = [st.clone()];
        let mut ty = Ty::Never;
        let mut rows = Vec::new();
        for arm in arms {
            let cover = self.bind_pattern(&arm.pat, &st)?;
            // Each alternative of the arm must match a value that no earlier one does.
            let alts = match (&arm.pat.kind, cover) {
                (PatKind::Or(ps), Cover::Or(cs)) => ps.iter().map(|p| p.span).zip(cs).collect(),
                (_, c) => vec![(arm.pat.span, c)],
            };
            let (n, mut useless) = (alts.len(), Vec::new());
            for (span, c) in alts {
                let row = vec![c];
                if u.useful(&rows, &row, &tys).map_err(too_complex)?.is_none() {
                    useless.push(span);
                }
                rows.push(row);
            }
            if useless.len() == n {
                let hint = "earlier arms match every value it matches; remove it";
                return Err(err(arm.pat.span, "unreachable arm".to_string(), Some(hint)));
            }
            if let Some(&span) = useless.first() {
                let hint = "earlier patterns match every value it matches; remove it";
                return Err(err(span, "unreachable alternative".to_string(), Some(hint)));
            }
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

    fn usefulness(&self) -> Usefulness<'a> {
        Usefulness {
            variants: self.variants,
            tparams: self.tparams,
            budget: MATCH_BUDGET,
            program: self.match_budget,
            depth: 0,
        }
    }

    /// What pattern `p` covers, typed against `st`, the type of the value it matches. Adds the
    /// names it binds to `binds`, left to right.
    fn pattern(&self, p: &Pat, st: &Ty, binds: &mut Vec<(String, Ty)>) -> Result<Cover, Diag> {
        if let Some(name) = self.const_pattern(p) {
            // kio has no match guards, so not Rust's `x if x == N`.
            let hint = format!("bind a name and compare in the arm: `x => if x == {name} {{ .. }} else {{ .. }}`");
            return Err(err(p.span, "constants cannot be patterns".to_string(), Some(&hint)));
        }
        let lit = |ty: Ty, c: Ctor| want(p.span, &ty, st).map(|_| Cover::Ctor(c, Vec::new()));
        match &p.kind {
            PatKind::Wild => Ok(Cover::All),
            PatKind::Int(n) => lit(Ty::Int, Ctor::Int(*n)),
            PatKind::Str(s) => lit(Ty::Str, Ctor::Str(s.clone())),
            PatKind::Bool(b) => lit(Ty::Bool, Ctor::Bool(*b)),
            PatKind::Name(n) if variant(&shape(self.variants, self.tparams, st).1, n).is_some() => {
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
            PatKind::Or(alts) => {
                let mut first = Vec::new();
                let mut covers = vec![self.pattern(&alts[0], st, &mut first)?];
                for alt in &alts[1..] {
                    let mut these = Vec::new();
                    covers.push(self.pattern(alt, st, &mut these)?);
                    same_binds(&first, &these, alts[0].span, alt.span)?;
                }
                for (n, ty) in first {
                    if binds.iter().any(|(b, _)| *b == n) {
                        let msg = format!("`{n}` is bound twice in this pattern");
                        return Err(err(p.span, msg, Some("rename one of them")));
                    }
                    binds.push((n, ty));
                }
                Ok(Cover::Or(covers))
            }
        }
    }

    /// The constant that pattern `p` names, as written: `N` or `web::N`.
    fn const_pattern(&self, p: &Pat) -> Option<String> {
        let (k, shown) = match &p.kind {
            PatKind::Name(n) => (key(self.home.module, n).into_owned(), n.clone()),
            PatKind::Module(m, inner) => match &inner.kind {
                PatKind::Name(n) => (format!("{m}::{n}"), format!("{m}::{n}")),
                _ => return None,
            },
            _ => return None,
        };
        self.consts.contains_key(&k).then_some(shown)
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
        if k != shape(self.variants, self.tparams, st).0 {
            want(p.span, &Ty::Enum(k, Vec::new()), st)?;
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
        let (en, vs) = shape(self.variants, self.tparams, st);
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
        let Ty::Struct(s, _) = ty else {
            let hint = "only structs have fields";
            return Err(err(span, format!("no field `{name}` on {ty}"), Some(hint)));
        };
        let decl = fields_of(self.fields, self.tparams, ty);
        match decl.iter().find(|(n, _)| n == name) {
            Some((_, t)) => Ok(t.clone()),
            None => Err(no_field(span, name, s, &decl)),
        }
    }

    /// `Name { .. }`, with `name` the struct's key, and its `..base` if written.
    fn struct_lit(
        &mut self,
        e: &Expr,
        name: &str,
        inits: &[(String, Expr)],
        base: Option<&Expr>,
        expected: Option<&Ty>,
    ) -> Result<Ty, Diag> {
        let fields = self.fields;
        let Some(decl) = fields.get(name) else {
            let hint = match self.is_cap(name) {
                true => NO_CAP_SYNTAX.to_string(),
                false if !self.home.module.is_empty() && self.structs.contains_key(bare(name)) => ROOT_ITEM.to_string(),
                false => suggest(bare(name), &sorted_names(self.structs, module_of(name)), "structs"),
            };
            return Err(err(e.span, format!("unknown struct `{name}`"), Some(&hint)));
        };
        let generic = !self.tparams[name].is_empty();
        let mut seen = Vec::new();
        let mut params = Vec::new();
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
            match generic {
                // Checked below, once every field is known to exist.
                true => params.push(ty.clone()),
                false => want_expr(x, &self.expr(x, Some(ty))?, ty)?,
            }
        }
        if let (None, Some((missing, _))) = (base, decl.iter().find(|(n, _)| !seen.contains(&n))) {
            let msg = format!("missing field `{missing}` in `{name}`");
            return Err(err(e.span, msg, Some(&format!("add `{missing}: ..`"))));
        }
        let written = self.written(name).to_string();
        // The base has the literal's own type, as in Rust (no type-changing struct update).
        let base_hint = |mut d: Diag| {
            if base.is_some_and(|b| b.span == d.span) {
                d.hint = Some(format!("`..` takes the fields not written from another `{written}`"));
            }
            d
        };
        if !generic {
            let ty = Ty::Struct(name.to_string(), Vec::new());
            if let Some(b) = base {
                want_expr(b, &self.expr(b, Some(&ty))?, &ty).map_err(base_hint)?;
            }
            return Ok(ty);
        }
        let mut parts: Vec<String> = inits.iter().map(|(f, _)| format!("field `{f}`")).collect();
        let mut args: Vec<&Expr> = inits.iter().map(|(_, x)| x).collect();
        if let Some(b) = base {
            let own = self.tparams[name].iter().map(|t| Ty::Param(t.clone())).collect();
            params.push(Ty::Struct(name.to_string(), own));
            parts.push("`..`".to_string());
            args.push(b);
        }
        let sig = self.type_sig("struct", name, params, parts, &format!("{written} {{ .. }}"));
        let tys = (self.infer(e, e.span, (&written, &sig), None, &args, expected)).map_err(base_hint)?;
        Ok(Ty::Struct(name.to_string(), tys))
    }

    /// The type of an assignment target: a `let mut` local followed by fields and indexes.
    /// Recorded like an expression's.
    fn place(&mut self, target: &Expr) -> Result<Ty, Diag> {
        let ty = match &target.kind {
            ExprKind::Name(name) => {
                let captured = self.is_capture(name);
                let Some(local) = self.lookup(name, target.span)? else {
                    if self.consts.contains_key(&*key(self.home.module, name)) {
                        let msg = format!("cannot assign to constant `{name}`");
                        let hint = format!("assign to a `let mut` copy: `let mut x = {name};`");
                        return Err(err(target.span, msg, Some(&hint)));
                    }
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
            (Ty::List(_), "sort" | "sort_by" | "sort_by_key" | "reverse") => ("list", ""),
            (Ty::Bytes, "extend") => ("bytes", ""),
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
        let (name, module, explicit) = match &callee.kind {
            ExprKind::Name(name) if builtin(name).is_some() => return self.constructor(e, name, args, expected),
            ExprKind::Name(name) => (name, None, None),
            ExprKind::Turbofish(name, tys) => (name, None, Some(tys.as_slice())),
            ExprKind::Path(en, v) if (en == "Map" || en == "HashMap") && v == "new" => {
                return self.map_new(e, en, args, expected)
            }
            ExprKind::Path(en, v) => return self.variant_value(e, &self.enum_key(en), v, args, expected),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::Name(name) => (name, Some(m.as_str()), None),
                ExprKind::Turbofish(name, tys) => (name, Some(m.as_str()), Some(tys.as_slice())),
                ExprKind::Path(en, v) => return self.module_variant(e, m, en, v, args, expected),
                _ => return Err(nested_path(callee.span)),
            },
            _ => return self.call_value(e, callee, args),
        };
        let (k, sig) = match (module, explicit) {
            (None, Some(_)) => self.turbofish_fn(callee.span, name)?,
            _ => self.named_fn(callee.span, name, module)?,
        };
        if args.len() != sig.params.len() {
            let msg = format!("`{name}` takes {} arguments, found {}", sig.params.len(), args.len());
            let hint = format!("`{name}` is declared as `{}`", sig.decl);
            return Err(err(e.span, msg, Some(&hint)));
        }
        if !sig.tparams.is_empty() || explicit.is_some() {
            let written = module.map_or(name.to_string(), |m| format!("{m}::{name}"));
            let (at, sig) = (callee.span, (k.as_str(), written.as_str(), sig));
            return self.generic_call(e, at, sig, explicit, args, expected);
        }
        for (a, param) in args.iter().zip(&sig.params) {
            want_expr(a, &self.operand(a, Some(param))?, param)?;
        }
        Ok(sig.ret.clone())
    }

    /// A call of a generic function, named at `at` and given as (key, name as written,
    /// signature). Its type arguments come from `explicit`, else from the arguments' types,
    /// then from `expected` for those only the return type mentions. An argument that needs an
    /// expected type (`None`, `[]`, a closure) and whose parameter type is not known yet is
    /// checked after the others. Records the instance; returns the call's type.
    fn generic_call(
        &mut self,
        e: &Expr,
        at: Span,
        (k, written, sig): (&str, &str, &Sig),
        explicit: Option<&[Type]>,
        args: &[Expr],
        expected: Option<&Ty>,
    ) -> Result<Ty, Diag> {
        let args: Vec<&Expr> = args.iter().collect();
        let tys = self.infer(e, at, (written, sig), explicit, &args, expected)?;
        let pairs: Vec<(String, Ty)> = sig.tparams.iter().cloned().zip(tys.iter().cloned()).collect();
        self.uses.push((at, k.to_string(), tys));
        Ok(subst(&sig.ret, &pairs))
    }

    /// Argument `a` of a use of `sig`: a capability may be one in a call (rule 1 allows it as
    /// an argument), never in a literal or variant.
    fn value(&mut self, a: &Expr, sig: &Sig, expected: Option<&Ty>) -> Result<Ty, Diag> {
        match sig.parts {
            Some(_) => self.expr(a, expected),
            None => self.operand(a, expected),
        }
    }

    /// The type arguments of a use of generic `g` at `e` (named at `at`) with values `args`,
    /// as `generic_call` describes; the values are checked against the instance.
    fn infer(
        &mut self,
        e: &Expr,
        at: Span,
        g: Generic,
        explicit: Option<&[Type]>,
        args: &[&Expr],
        expected: Option<&Ty>,
    ) -> Result<Vec<Ty>, Diag> {
        let sig = g.1;
        let mut binds = self.explicit_args(at, g, explicit)?;
        let mut found = vec![None; args.len()];
        let mut later = Vec::new();
        for (i, (a, p)) in args.iter().zip(&sig.params).enumerate() {
            if let Some(k) = known(p, sig, &binds) {
                // Matched too, so that `same(1, "a")` names where `T` became Int. A turbofish
                // fixed every type parameter: the caller compares the whole types.
                let t = self.value(a, sig, Some(&k))?;
                if explicit.is_none() {
                    unify(p, &t, g, &mut binds, (a.span, i + 1), true)?;
                }
                found[i] = Some(t);
            } else if self.needs_expected(a) {
                later.push(i);
            } else {
                let t = self.value(a, sig, None)?;
                unify(p, &t, g, &mut binds, (a.span, i + 1), true)?;
                found[i] = Some(t);
            }
        }
        if let Some(want) = expected {
            unify(&sig.ret, want, g, &mut binds, (e.span, 0), false)?;
        }
        let open = |binds: &[Option<Bound>]| {
            not_expected(e.span, g, binds, expected).unwrap_or_else(|| cannot_infer(e.span, g, binds, true))
        };
        for i in later {
            let (a, p) = (args[i], &sig.params[i]);
            let t = if let Some(p) = known(p, sig, &binds) {
                self.value(a, sig, Some(&p))?
            } else if self.open_use(a) {
                // Nothing more is known: checked as alone, where it says what it cannot infer.
                self.value(a, sig, None)?
            } else {
                // Only the parameter types of a `fn` parameter may be known: enough for a
                // closure, or for a generic function named as a value.
                let Ty::Fn(fps, _) = p else {
                    return Err(open(&binds));
                };
                let pts: Option<Vec<Ty>> = fps.iter().map(|t| known(t, sig, &binds)).collect();
                match (&a.kind, pts) {
                    (ExprKind::Closure(ps, body), Some(pts)) if ps.len() == pts.len() => {
                        let ty = self.closure_with(a, ps, body, Some((&pts, None)), None, None)?;
                        self.record(a, &ty);
                        ty
                    }
                    (ExprKind::Closure(ps, _), _) if ps.iter().all(|q| q.ty.is_some()) => self.expr(a, None)?,
                    (ExprKind::Name(_) | ExprKind::Module(..), Some(pts)) => {
                        self.expr(a, Some(&Ty::Fn(pts, Box::new(Ty::Never))))?
                    }
                    _ => return Err(open(&binds)),
                }
            };
            if explicit.is_none() {
                unify(p, &t, g, &mut binds, (a.span, i + 1), true)?;
            }
            found[i] = Some(t);
        }
        let tys = all_bound(e.span, g, &binds, true).map_err(|_| open(&binds))?;
        let pairs: Vec<(String, Ty)> = sig.tparams.iter().cloned().zip(tys.iter().cloned()).collect();
        for ((a, p), t) in args.iter().zip(&sig.params).zip(found) {
            want_expr(a, &t.expect("every argument is checked"), &subst(p, &pairs))?;
        }
        self.bounds_hold(g, &binds)?;
        Ok(tys)
    }

    /// Each type argument of a use of `g`, all of them bound, satisfies its parameter's bound;
    /// the error is at the span that fixed the argument.
    fn bounds_hold(&self, (written, sig): Generic, binds: &[Option<Bound>]) -> Result<(), Diag> {
        // A literal or variant names its type: `Set`, not `Set { .. }` or `Tree::Leaf`.
        let owner = match (&sig.parts, &sig.ret) {
            (Some(_), Ty::Struct(k, _) | Ty::Enum(k, _)) => self.written(k),
            _ => written,
        };
        for ((t, b), bind) in sig.tparams.iter().zip(&sig.bounds).zip(binds) {
            let (Some(b), Some(bind)) = (*b, bind) else { continue };
            if let Some(why) = unmet(&bind.ty, b, self.home.tparams, self.fields, self.variants) {
                return Err(bound_err(bind.at, &bind.ty, (owner, t, b), &why));
            }
        }
        Ok(())
    }

    /// The type arguments written after `::<` for a generic function given as (name as
    /// written, signature), one slot per type parameter; all empty without a turbofish.
    fn explicit_args(
        &self,
        at: Span,
        (written, sig): (&str, &Sig),
        explicit: Option<&[Type]>,
    ) -> Result<Vec<Option<Bound>>, Diag> {
        let mut binds = vec![None; sig.tparams.len()];
        let Some(tys) = explicit else { return Ok(binds) };
        if sig.tparams.is_empty() {
            let msg = format!("`{written}` is not generic");
            return Err(err(at, msg, Some(&format!("remove `::<..>`: write `{written}`"))));
        }
        if tys.len() != sig.tparams.len() {
            let n = sig.tparams.len();
            let msg = format!("`{written}` takes {n} type arguments, found {}", tys.len());
            return Err(err(
                at,
                msg,
                Some(&format!("`{written}` is declared as `{}`", sig.decl)),
            ));
        }
        for ((t, slot), name) in tys.iter().zip(&mut binds).zip(&sig.tparams) {
            let ty = resolve(t, self.caps, self.structs, self.enums, self.home)?;
            if cap_in(&ty).is_some() {
                return Err(cap_type_arg(t.span, name, &ty));
            }
            *slot = Some(Bound { ty, at: t.span, arg: 0 });
        }
        Ok(binds)
    }

    /// Whether argument `a` takes its type from the parameter's: it is checked after the
    /// arguments that can fix the type parameters.
    fn needs_expected(&self, a: &Expr) -> bool {
        match &a.kind {
            ExprKind::Closure(..) => true,
            // The first element gives the others their type: `[Tree::Leaf]`.
            ExprKind::List(items) => items.first().is_none_or(|x| self.needs_expected(x)),
            ExprKind::Name(n) if n == "None" => true,
            ExprKind::Call(f, args) => match &f.kind {
                ExprKind::Name(n) if n == "Some" => args.first().is_some_and(|x| self.needs_expected(x)),
                ExprKind::Name(n) if n == "Ok" || n == "Err" => true,
                ExprKind::Path(m, n) if (m == "Map" || m == "HashMap") && n == "new" => true,
                _ => self.open_use(a),
            },
            // A generic function named as a value.
            ExprKind::Name(n) => {
                let local = self.scopes.iter().any(|s| s.contains_key(n));
                let sig = self.sigs.get(&*key(self.home.module, n));
                !local && sig.is_some_and(|s| !s.tparams.is_empty())
            }
            // A variant without a payload of a generic enum, such as `Tree::Leaf`.
            ExprKind::Path(en, _) => self.tparams.get(&self.enum_key(en)).is_some_and(|t| !t.is_empty()),
            ExprKind::StructLit(..) => self.open_use(a),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::Name(n) => self
                    .sigs
                    .get(&format!("{m}::{n}"))
                    .is_some_and(|s| !s.tparams.is_empty()),
                ExprKind::Path(en, _) => self.tparams.get(&format!("{m}::{en}")).is_some_and(|t| !t.is_empty()),
                ExprKind::StructLit(..) => self.open_use(a),
                _ => false,
            },
            _ => false,
        }
    }

    /// Whether `a` is a generic struct literal, a variant of a generic enum with values, or a
    /// call of a generic function (no turbofish) whose values leave a type parameter of its
    /// type open: `Box { v: None }`, `E::L("x")` of `E<A, B>`, `empty()` returning `List<T>`.
    /// Such a use is checked once the expected type is known, as `Some(None)` is.
    fn open_use(&self, a: &Expr) -> bool {
        let key_in = |m: Option<&str>, n: &str| match m {
            Some(m) => format!("{m}::{n}"),
            None => key(self.home.module, n).into_owned(),
        };
        // The type parameters, each value with its declared type, and the use's type for a call.
        let (tparams, parts, ret): (&[String], Vec<(&Ty, &Expr)>, Option<&Ty>) = match in_module(a) {
            // A base fixes every type parameter, unless it needs the expected type itself.
            (_, ExprKind::StructLit(_, _, Some(b))) if !self.needs_expected(b) => return false,
            (m, ExprKind::StructLit(name, inits, _)) => {
                let k = key_in(m, name);
                let (Some(tparams), Some(decl)) = (self.tparams.get(&k), self.fields.get(&k)) else {
                    return false;
                };
                let parts = (inits.iter())
                    .filter_map(|(f, x)| decl.iter().find(|(n, _)| n == f).map(|(_, t)| (t, x)))
                    .collect();
                (tparams, parts, None)
            }
            (None, ExprKind::Call(f, args)) => match in_module(f) {
                (m, ExprKind::Path(en, v)) => {
                    let en = m.map_or_else(|| self.enum_key(en), |m| format!("{m}::{en}"));
                    let payload = self.variants.get(&en).and_then(|vs| variant(vs, v));
                    let (Some(tparams), Some((_, payload))) = (self.tparams.get(&en), payload) else {
                        return false;
                    };
                    (tparams, payload.iter().zip(args).collect(), None)
                }
                (m, ExprKind::Name(n)) if builtin(n).is_none() => {
                    if m.is_none() && self.scopes.iter().any(|s| s.contains_key(n)) {
                        return false;
                    }
                    let Some(sig) = self.sigs.get(&key_in(m, n)) else {
                        return false;
                    };
                    (&sig.tparams, sig.params.iter().zip(args).collect(), Some(&sig.ret))
                }
                _ => return false,
            },
            _ => return false,
        };
        let fixed: Vec<&Ty> = (parts.into_iter())
            .filter(|(_, x)| !self.needs_expected(x))
            .map(|(t, _)| t)
            .collect();
        (tparams.iter()).any(|t| ret.is_none_or(|r| mentions(r, t)) && !fixed.iter().any(|p| mentions(p, t)))
    }

    /// The key and signature of function `name` written with a turbofish, which a local of
    /// that name would hide.
    fn turbofish_fn(&self, span: Span, name: &str) -> Result<(String, &'a Sig), Diag> {
        if self.scopes.iter().any(|s| s.contains_key(name)) {
            let msg = format!("the local `{name}` hides the function `{name}`");
            return Err(err(span, msg, Some("rename the local")));
        }
        self.named_fn(span, name, None)
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
    /// the value never adds authority. A generic function's type arguments come from
    /// `explicit`, else from the `expected` `fn` type.
    fn fn_value(&mut self, span: Span, k: &str, explicit: Option<&[Type]>, expected: Option<&Ty>) -> Result<Ty, Diag> {
        if k == "main" {
            let hint = "move the code into another function and use that";
            return Err(err(span, "`main` cannot be used as a value".to_string(), Some(hint)));
        }
        let sigs = self.sigs;
        let sig = &sigs[k];
        if sig.params.iter().any(|p| cap_in(p).is_some()) {
            return Err(fn_cap_err(span));
        }
        let ty = Ty::Fn(sig.params.clone(), Box::new(sig.ret.clone()));
        if sig.tparams.is_empty() && explicit.is_none() {
            return Ok(ty);
        }
        let written = if module_of(k) == self.home.module { bare(k) } else { k };
        let g = (written, sig);
        let mut binds = self.explicit_args(span, g, explicit)?;
        if let Some(want) = expected {
            unify(&ty, want, g, &mut binds, (span, 0), false)?;
        }
        let tys = all_bound(span, g, &binds, false)?;
        self.bounds_hold(g, &binds)?;
        let pairs: Vec<(String, Ty)> = sig.tparams.iter().cloned().zip(tys.iter().cloned()).collect();
        self.uses.push((span, k.to_string(), tys));
        Ok(subst(&ty, &pairs))
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
        if params.len() + frame.captures.len() > MAX_PARAMS {
            let (p, c) = (params.len(), frame.captures.len());
            return Err(too_many_params(
                e.span,
                format!("this closure takes {p} parameters and captures {c} locals"),
            ));
        }
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
