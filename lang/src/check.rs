use crate::ast::*;
use crate::parse::parse;
use crate::{Diag, Span};
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
    /// `Dir` and `Http` and their error enums, mirroring caps.wit until a build step generates this.
    pub fn v0() -> CapTable {
        let at = Span { start: 0, end: 0 };
        let err_enum = |name: &str, vs: &[(&str, bool)]| Enum {
            name: name.to_string(),
            variants: vs
                .iter()
                .map(|(v, payload)| Variant {
                    name: v.to_string(),
                    fields: if *payload {
                        vec![Type {
                            kind: TypeKind::Named("Int".to_string(), vec![]),
                            span: at,
                        }]
                    } else {
                        vec![]
                    },
                    span: at,
                })
                .collect(),
            span: at,
        };
        let result = |e: &str| Ty::Result(Box::new(Ty::Bytes), Box::new(Ty::Enum(e.to_string())));
        let method = |name: &str, params, ret| (name.to_string(), params, ret);
        CapTable {
            caps: vec![
                CapType {
                    name: "Dir".to_string(),
                    methods: vec![
                        method("read", vec![Ty::Str], result("IoErr")),
                        method(
                            "create",
                            vec![Ty::Str, Ty::Bytes],
                            Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("IoErr".to_string()))),
                        ),
                    ],
                },
                CapType {
                    name: "Http".to_string(),
                    methods: vec![method("get", vec![Ty::Str], result("HttpErr"))],
                },
            ],
            enums: vec![
                err_enum(
                    "IoErr",
                    &[
                        ("NotFound", false),
                        ("Denied", false),
                        ("TooLarge", false),
                        ("InvalidPath", false),
                    ],
                ),
                err_enum(
                    "HttpErr",
                    &[
                        ("Denied", false),
                        ("Timeout", false),
                        ("TooLarge", false),
                        ("Status", true),
                    ],
                ),
            ],
        }
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
    pub(crate) funcs: HashMap<String, Fn>,
    pub(crate) structs: HashMap<String, Struct>,
    pub(crate) enums: HashMap<String, Enum>,
    /// Names of the capability types, such as `Dir`.
    pub(crate) caps: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Int,
    Bool,
    Str,
    Bytes,
    Unit,
    Struct(String),
    Enum(String),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    /// A capability, which only a parameter can hold.
    Cap(String),
    /// A block that always returns; it matches any type.
    Never,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Ty::Int => f.write_str("Int"),
            Ty::Bool => f.write_str("Bool"),
            Ty::Str => f.write_str("Str"),
            Ty::Bytes => f.write_str("Bytes"),
            Ty::Unit => f.write_str("()"),
            Ty::Struct(name) | Ty::Enum(name) | Ty::Cap(name) => f.write_str(name),
            Ty::Option(t) => write!(f, "Option<{t}>"),
            Ty::Result(t, e) => write!(f, "Result<{t}, {e}>"),
            Ty::Never => f.write_str("!"),
        }
    }
}

/// Hint for rule 2: no expression produces a capability.
const NO_CAP_SYNTAX: &str = "capabilities come only as parameters of `main`";

/// Resolved field names and types of each struct, in declaration order.
type Fields = HashMap<String, Vec<(String, Ty)>>;

/// Resolved variant names and payload types of each enum, in declaration order.
type Variants = HashMap<String, Vec<(String, Vec<Ty>)>>;

/// An enum's name and its variants with payload types, as `Checker::shape` returns.
type Shape = (String, Vec<(String, Vec<Ty>)>);

/// How a `match` pattern counts toward exhaustiveness.
#[derive(PartialEq)]
enum Cover {
    All,
    Variant(usize),
    Bool(bool),
    Lit,
}

struct Sig {
    /// The declaration as written, like `fn f(a: Int, b: Str)`, for hints.
    decl: String,
    params: Vec<Ty>,
    ret: Ty,
}

struct Local {
    ty: Ty,
    mutable: bool,
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

fn sorted_keys<V>(m: &HashMap<String, V>) -> Vec<&str> {
    let mut keys: Vec<_> = m.keys().map(String::as_str).collect();
    keys.sort();
    keys
}

/// What an operator accepts, for the hint on `cannot be applied to`.
fn op_hint(op: &str) -> String {
    let accepts = match op {
        "+" => "two Int or two Str",
        "-" | "*" | "/" | "%" | "<" | "<=" | ">" | ">=" => "two Int",
        "&&" | "||" => "two Bool",
        _ => "two Int, two Bool, two Str or two Bytes",
    };
    format!("`{op}` works on {accepts}")
}

/// The result type of `l op r`, or `None` if the operator does not apply.
fn binary_ty(op: &str, l: &Ty, r: &Ty) -> Option<Ty> {
    use Ty::*;
    match (op, l, r) {
        ("+", Str, Str) => Some(Str),
        ("+" | "-" | "*" | "/" | "%", Int, Int) => Some(Int),
        ("<" | "<=" | ">" | ">=", Int, Int) => Some(Bool),
        ("&&" | "||", Bool, Bool) => Some(Bool),
        ("==" | "!=", a, b)
            if a == b && !matches!(a, Never | Cap(_) | Struct(_) | Enum(_) | Option(_) | Result(..)) =>
        {
            Some(Bool)
        }
        _ => None,
    }
}

fn resolve(
    t: &Type,
    caps: &CapTable,
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
) -> Result<Ty, Diag> {
    match &t.kind {
        TypeKind::Unit => Ok(Ty::Unit),
        TypeKind::Named(n, args) => match (n.as_str(), args.as_slice()) {
            ("Option", [a]) => Ok(Ty::Option(Box::new(resolve(a, caps, structs, enums)?))),
            ("Result", [a, b]) => {
                let (a, b) = (resolve(a, caps, structs, enums)?, resolve(b, caps, structs, enums)?);
                Ok(Ty::Result(Box::new(a), Box::new(b)))
            }
            ("Option" | "Result", _) => {
                let (count, hint) = if n == "Option" {
                    (1, "write `Option<T>`")
                } else {
                    (2, "write `Result<T, E>`")
                };
                let msg = format!("`{n}` takes {count} type arguments, found {}", args.len());
                Err(err(t.span, msg, Some(hint)))
            }
            ("Int", []) => Ok(Ty::Int),
            ("Bool", []) => Ok(Ty::Bool),
            ("Str", []) => Ok(Ty::Str),
            ("Bytes", []) => Ok(Ty::Bytes),
            (_, []) if structs.contains_key(n) => Ok(Ty::Struct(n.clone())),
            (_, []) if enums.contains_key(n) => Ok(Ty::Enum(n.clone())),
            (_, []) if caps.caps.iter().any(|c| c.name == *n) => Ok(Ty::Cap(n.clone())),
            _ => Err(unknown_type(t)),
        },
    }
}

fn unknown_type(t: &Type) -> Diag {
    err(
        t.span,
        "type is not available yet".to_string(),
        Some("the types so far are Int, Bool, Str, Bytes, (), Option, Result and your structs and enums"),
    )
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
        Ty::Option(t) => cap_in(t),
        Ty::Result(t, e) => cap_in(t).or_else(|| cap_in(e)),
        _ => None,
    }
}

/// Rule 1 for types: a capability type is allowed only as the whole type of a parameter.
fn cap_type_ok(ty: &Ty, span: Span, place: Place) -> Result<(), Diag> {
    let Some(cap) = cap_in(ty) else { return Ok(()) };
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
) -> Result<Sig, Diag> {
    if sigs.contains_key(&f.name) {
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
        let ty = resolve(&p.ty, caps, structs, enums)?;
        cap_type_ok(&ty, p.ty.span, Place::Param)?;
        if f.name == "main" && !matches!(ty, Ty::Cap(_)) {
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
    let ret = resolve(&f.ret, caps, structs, enums)?;
    cap_type_ok(&ret, f.ret.span, Place::Return(&f.name))?;
    Ok(Sig {
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
) -> Result<Vec<(String, Ty)>, Diag> {
    let mut fields: Vec<(String, Ty)> = Vec::new();
    for p in &s.fields {
        if fields.iter().any(|(name, _)| *name == p.name) {
            let msg = format!("duplicate field `{}`", p.name);
            return Err(err(p.span, msg, Some("rename one of them")));
        }
        let ty = resolve(&p.ty, caps, structs, enums)?;
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
) -> Result<Vec<(String, Vec<Ty>)>, Diag> {
    let mut variants: Vec<(String, Vec<Ty>)> = Vec::new();
    for v in &e.variants {
        if variants.iter().any(|(name, _)| *name == v.name) {
            let msg = format!("duplicate variant `{}`", v.name);
            return Err(err(v.span, msg, Some("rename one of them")));
        }
        let mut payload = Vec::new();
        for t in &v.fields {
            let ty = resolve(t, caps, structs, enums)?;
            cap_type_ok(&ty, t.span, Place::Payload)?;
            payload.push(ty);
        }
        variants.push((v.name.clone(), payload));
    }
    Ok(variants)
}

/// The index and payload types of variant `v` among `variants`.
fn variant<'a>(variants: &'a [(String, Vec<Ty>)], v: &str) -> Option<(usize, &'a [Ty])> {
    let i = variants.iter().position(|(name, _)| name == v)?;
    Some((i, &variants[i].1))
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
        (Ty::Str, "len") | (Ty::Bytes, "len") => Some((vec![], Ty::Int)),
        (Ty::Str, "to_bytes") => Some((vec![], Ty::Bytes)),
        _ => None,
    }
}

/// Every built-in method name, so hints can list the ones `method` accepts on a type.
const METHODS: &[&str] = &["len", "to_bytes", "to_str"];

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
        .filter(|n| method(caps, recv, n).is_some())
        .collect()
}

/// The user types named inside `ty`.
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

/// Checks `src`. Reports at most one `Diag` per function, in source order.
pub fn check(src: &str, caps: &CapTable) -> Result<Program, Vec<Diag>> {
    let items = parse(src).map_err(|d| vec![d])?;
    let mut fns = Vec::new();
    let mut structs = HashMap::new();
    let mut enums = HashMap::new();
    let mut order = Vec::new();
    let mut diags = Vec::new();
    for e in &caps.enums {
        order.push(e.name.clone());
        enums.insert(e.name.clone(), e.clone());
    }
    let reserved = |n: &str| {
        matches!(n, "Option" | "Result")
            || caps.enums.iter().any(|e| e.name == n)
            || caps.caps.iter().any(|c| c.name == n)
    };
    for item in items {
        let declared = match &item {
            Item::Struct(s) => Some((s.name.as_str(), s.span)),
            Item::Enum(e) => Some((e.name.as_str(), e.span)),
            Item::Fn(_) => None,
        };
        if let Some((name, span)) = declared.filter(|(n, _)| reserved(n)) {
            let msg = format!("`{name}` is a built-in type");
            diags.push(err(span, msg, Some("rename your type")));
            continue;
        }
        match item {
            Item::Fn(f) => fns.push(f),
            Item::Enum(e) if structs.contains_key(&e.name) || enums.contains_key(&e.name) => {
                diags.push(err(
                    e.span,
                    format!("duplicate type `{}`", e.name),
                    Some("rename one of them"),
                ));
            }
            Item::Enum(e) => {
                order.push(e.name.clone());
                enums.insert(e.name.clone(), e);
            }
            Item::Struct(s) if structs.contains_key(&s.name) || enums.contains_key(&s.name) => {
                diags.push(err(
                    s.span,
                    format!("duplicate type `{}`", s.name),
                    Some("rename one of them"),
                ));
            }
            Item::Struct(s) => {
                order.push(s.name.clone());
                structs.insert(s.name.clone(), s);
            }
        }
    }

    // Types before signatures, which may name them.
    let mut fields = Fields::new();
    let mut variants = Variants::new();
    for name in &order {
        let resolved = match structs.get(name) {
            Some(s) => struct_fields(s, caps, &structs, &enums).map(|f| drop(fields.insert(name.clone(), f))),
            None => enum_variants(&enums[name], caps, &structs, &enums).map(|v| drop(variants.insert(name.clone(), v))),
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
    for f in &fns {
        match signature(f, &sigs, caps, &structs, &enums) {
            Ok(sig) => {
                sigs.insert(f.name.clone(), sig);
                sig_errs.push(None);
            }
            Err(d) => sig_errs.push(Some(d)),
        }
    }

    for (f, sig_err) in fns.iter().zip(sig_errs) {
        let mut c = Checker {
            caps,
            sigs: &sigs,
            fields: &fields,
            variants: &variants,
            structs: &structs,
            enums: &enums,
            scopes: Vec::new(),
            ret: Ty::Unit,
        };
        diags.extend(sig_err.or_else(|| c.function(f).err()));
    }
    if !fns.iter().any(|f| f.name == "main") {
        let none = Span { start: 0, end: 0 };
        let hint = "add `fn main() { .. }`";
        diags.push(err(none, "no `main` function".to_string(), Some(hint)));
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(Program {
        funcs: fns.into_iter().map(|f| (f.name.clone(), f)).collect(),
        structs,
        enums,
        caps: caps.caps.iter().map(|c| c.name.clone()).collect(),
    })
}

struct Checker<'a> {
    caps: &'a CapTable,
    sigs: &'a HashMap<String, Sig>,
    fields: &'a Fields,
    variants: &'a Variants,
    structs: &'a HashMap<String, Struct>,
    enums: &'a HashMap<String, Enum>,
    scopes: Vec<HashMap<String, Local>>,
    /// Return type of the function being checked.
    ret: Ty,
}

impl Checker<'_> {
    fn function(&mut self, f: &Fn) -> Result<(), Diag> {
        let sig = &self.sigs[f.name.as_str()];
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
        let hint = match closest(name, &names) {
            _ if self.is_cap(name) => NO_CAP_SYNTAX.to_string(),
            Some(c) => format!("did you mean `{c}`?"),
            None => "declare it with `let`".to_string(),
        };
        err(span, format!("unknown name `{name}`"), Some(&hint))
    }

    fn is_cap(&self, name: &str) -> bool {
        self.caps.caps.iter().any(|c| c.name == name)
    }

    fn unknown_variant(&self, span: Span, en: &str, v: &str) -> Diag {
        let hint = match (en, self.variants.get(en)) {
            _ if self.is_cap(en) => NO_CAP_SYNTAX.to_string(),
            ("Option", _) => suggest(v, &["None", "Some"], "variants"),
            ("Result", _) => suggest(v, &["Ok", "Err"], "variants"),
            (_, Some(vs)) => suggest(v, &vs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), "variants"),
            _ => suggest(en, &sorted_keys(self.enums), "enums"),
        };
        err(span, format!("unknown variant `{en}::{v}`"), Some(&hint))
    }

    fn lookup(&mut self, name: &str) -> Option<&mut Local> {
        self.scopes.iter_mut().rev().find_map(|s| s.get_mut(name))
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
                let declared = ty.as_ref().map(|t| resolve(t, self.caps, self.structs, self.enums));
                let declared = declared.transpose()?;
                if let ExprKind::Name(from) = &init.kind {
                    if let Some(Local { ty: Ty::Cap(_), .. }) = self.lookup(from) {
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
                let local = Local {
                    ty: found,
                    mutable: *mutable,
                };
                self.scopes.last_mut().unwrap().insert(name.clone(), local);
                Ok(Ty::Unit)
            }
            StmtKind::Assign { target, op, value } => {
                let found = self.expr(value, None)?;
                let ty = self.place(target)?;
                if *op == "=" {
                    want_expr(value, &found, &ty)?;
                } else if binary_ty(&op[..1], &ty, &found) != Some(ty.clone()) {
                    let msg = format!("`{op}` cannot be applied to {ty} and {found}");
                    return Err(err(s.span, msg, Some(&op_hint(&op[..1]))));
                }
                Ok(Ty::Unit)
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
                want(body.span, &self.block(body, None)?, &Ty::Unit)?;
                Ok(Ty::Unit)
            }
            StmtKind::Expr(e) => Ok(match self.expr(e, None)? {
                Ty::Never => Ty::Never,
                _ => Ty::Unit,
            }),
        }
    }

    fn expr(&mut self, e: &Expr, expected: Option<&Ty>) -> Result<Ty, Diag> {
        match &e.kind {
            ExprKind::Int(_) => Ok(Ty::Int),
            ExprKind::Str(_) => Ok(Ty::Str),
            ExprKind::Bool(_) => Ok(Ty::Bool),
            ExprKind::Unit => Ok(Ty::Unit),
            ExprKind::Name(name) if builtin(name).is_some() => self.constructor(e, name, &[], expected),
            ExprKind::Name(name) => match self.lookup(name) {
                Some(Local { ty: Ty::Cap(_), .. }) => {
                    let hint = "call methods on the parameter itself";
                    Err(err(e.span, "capability used as a value".to_string(), Some(hint)))
                }
                Some(local) => Ok(local.ty.clone()),
                None => Err(self.unknown_name(e.span, name)),
            },
            ExprKind::Unary(op, x) => {
                let want_ty = if *op == "-" { Ty::Int } else { Ty::Bool };
                want_expr(x, &self.expr(x, None)?, &want_ty)?;
                Ok(want_ty)
            }
            ExprKind::Binary(op, a, b) => {
                let (l, r) = (self.expr(a, None)?, self.expr(b, None)?);
                binary_ty(op, &l, &r).ok_or_else(|| {
                    let msg = format!("`{op}` cannot be applied to {l} and {r}");
                    err(e.span, msg, Some(&op_hint(op)))
                })
            }
            ExprKind::If(cond, then, els) => {
                want_expr(cond, &self.expr(cond, None)?, &Ty::Bool)?;
                let t = self.block(then, expected)?;
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
            ExprKind::Block(b) => self.block(b, expected),
            ExprKind::Call(callee, args) => self.call(e, callee, args, expected),
            ExprKind::Method(recv, name, args) => self.method_call(e, recv, name, args),
            ExprKind::Field(base, name) => {
                let ty = self.expr(base, None)?;
                self.field(&ty, name, e.span)
            }
            ExprKind::Index(..) => {
                let hint = "indexing comes with `List`, later";
                Err(err(e.span, "indexing is not available yet".to_string(), Some(hint)))
            }
            ExprKind::StructLit(name, inits) => self.struct_lit(e, name, inits),
            ExprKind::Path(en, v) => self.variant_value(e, en, v, &[]),
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms, expected),
            ExprKind::Try(x) => self.try_expr(e, x),
        }
    }

    /// Like `expr`, but a capability parameter is allowed: the one place rule 1 lets it appear.
    fn operand(&mut self, e: &Expr, expected: Option<&Ty>) -> Result<Ty, Diag> {
        if let ExprKind::Name(name) = &e.kind {
            if builtin(name).is_none() {
                if let Some(Local {
                    ty: ty @ Ty::Cap(_), ..
                }) = self.lookup(name)
                {
                    return Ok(ty.clone());
                }
            }
        }
        self.expr(e, expected)
    }

    fn method_call(&mut self, e: &Expr, recv: &Expr, name: &str, args: &[Expr]) -> Result<Ty, Diag> {
        let recv_ty = self.operand(recv, None)?;
        let Some((params, ret)) = method(self.caps, &recv_ty, name) else {
            let names = method_names(self.caps, &recv_ty);
            let hint = if names.is_empty() {
                format!("{recv_ty} has no methods")
            } else {
                suggest(name, &names, &format!("{recv_ty} has"))
            };
            return Err(err(e.span, format!("no method `{name}` on {recv_ty}"), Some(&hint)));
        };
        if args.len() != params.len() {
            let msg = format!("`{name}` takes {} arguments, found {}", params.len(), args.len());
            let tys: Vec<_> = params.iter().map(Ty::to_string).collect();
            let hint = format!(
                "`{recv_ty}.{name}` is declared as `{name}({}) -> {ret}`",
                tys.join(", ")
            );
            return Err(err(e.span, msg, Some(&hint)));
        }
        for (a, param) in args.iter().zip(&params) {
            want_expr(a, &self.expr(a, Some(param))?, param)?;
        }
        Ok(ret)
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
                return Err(err(e.span, msg, Some(&format!("add a type: `{example}`"))));
            }
        };
        if let Some(t) = payload {
            want_expr(&args[0], &self.expr(&args[0], Some(t))?, t)?;
        }
        Ok(expected.expect("matched above").clone())
    }

    /// `x?`: the value of an `Option` or `Result`, or an early return of the `None` or `Err`.
    fn try_expr(&mut self, e: &Expr, x: &Expr) -> Result<Ty, Diag> {
        let found = self.expr(x, None)?;
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

    /// `E::V` or `E::V(args)`; arity and payload types must match the declaration.
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

    /// The enum's name and variants that `ty` matches on; empty for other types.
    fn shape(&self, ty: &Ty) -> Shape {
        let some = |t: &Ty| vec![("None".to_string(), vec![]), ("Some".to_string(), vec![t.clone()])];
        match ty {
            Ty::Enum(en) => (en.clone(), self.variants[en].clone()),
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

    fn match_expr(&mut self, scrut: &Expr, arms: &[Arm], expected: Option<&Ty>) -> Result<Ty, Diag> {
        let st = self.expr(scrut, None)?;
        if !matches!(
            st,
            Ty::Int | Ty::Bool | Ty::Str | Ty::Enum(_) | Ty::Option(_) | Ty::Result(..)
        ) {
            let hint = "match works on Int, Bool, Str and enums";
            return Err(err(scrut.span, format!("cannot match on {st}"), Some(hint)));
        }
        let shape = self.shape(&st);
        let mut ty = Ty::Never;
        let mut covers = Vec::new();
        for arm in arms {
            if covers.contains(&Cover::All) {
                let hint = "an earlier arm matches everything";
                return Err(err(arm.pat.span, "unreachable arm".to_string(), Some(hint)));
            }
            let (cover, binds) = self.pattern(&arm.pat, &st, &shape)?;
            covers.push(cover);
            let binds = binds.into_iter().map(|(name, ty)| (name, Local { ty, mutable: false }));
            self.scopes.push(binds.collect());
            let body = self.expr(&arm.body, expected)?;
            self.scopes.pop();
            if ty == Ty::Never {
                ty = body;
            } else if body != Ty::Never && body != ty {
                let msg = format!("`match` arms have different types: {ty} and {body}");
                return Err(err(arm.body.span, msg, Some("every branch must have the same type")));
            }
        }
        let seen = |c: Cover| covers.contains(&c) || covers.contains(&Cover::All);
        let missing: Vec<String> = match &st {
            Ty::Enum(_) | Ty::Option(_) | Ty::Result(..) => (shape.1.iter().enumerate())
                .filter(|(i, _)| !seen(Cover::Variant(*i)))
                .map(|(_, (v, _))| format!("{}::{v}", shape.0))
                .collect(),
            Ty::Bool => [true, false]
                .into_iter()
                .filter(|b| !seen(Cover::Bool(*b)))
                .map(|b| b.to_string())
                .collect(),
            _ if !covers.contains(&Cover::All) => vec!["_".to_string()],
            _ => Vec::new(),
        };
        if missing.is_empty() {
            Ok(ty)
        } else {
            let msg = format!("non-exhaustive `match`: missing {}", missing.join(", "));
            let hint = "add arms for the missing cases, or `_ => ..`";
            Err(err(scrut.span, msg, Some(hint)))
        }
    }

    /// What the pattern covers and the names it binds, typed against scrutinee type `st`.
    fn pattern(&self, p: &Pat, st: &Ty, shape: &Shape) -> Result<(Cover, Vec<(String, Ty)>), Diag> {
        let lit = |ty: Ty| want(p.span, &ty, st).map(|_| (Cover::Lit, Vec::new()));
        let (en, vs) = (shape.0.as_str(), &shape.1);
        match &p.kind {
            PatKind::Wild => Ok((Cover::All, Vec::new())),
            PatKind::Int(_) => lit(Ty::Int),
            PatKind::Str(_) => lit(Ty::Str),
            PatKind::Bool(b) => want(p.span, &Ty::Bool, st).map(|_| (Cover::Bool(*b), Vec::new())),
            PatKind::Name(n) => match variant(vs, n) {
                Some(_) => self.variant_pattern(p, shape, n, &[]),
                None => Ok((Cover::All, vec![(n.clone(), st.clone())])),
            },
            PatKind::Variant(named, v, names) => {
                if named.as_deref().is_some_and(|n| n != en) {
                    want(p.span, &Ty::Enum(named.clone().unwrap()), st)?;
                }
                self.variant_pattern(p, shape, v, names)
            }
        }
    }

    fn variant_pattern(
        &self,
        p: &Pat,
        (en, vs): &Shape,
        v: &str,
        names: &[Option<String>],
    ) -> Result<(Cover, Vec<(String, Ty)>), Diag> {
        let Some((i, payload)) = variant(vs, v) else {
            return Err(self.unknown_variant(p.span, en, v));
        };
        if names.len() != payload.len() {
            let msg = format!(
                "`{en}::{v}` has {} values, found {} bindings",
                payload.len(),
                names.len()
            );
            return Err(err(p.span, msg, Some(&holds(en, v, payload))));
        }
        let binds = names.iter().zip(payload);
        let binds = binds.filter_map(|(name, ty)| Some((name.clone()?, ty.clone())));
        Ok((Cover::Variant(i), binds.collect()))
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

    fn struct_lit(&mut self, e: &Expr, name: &str, inits: &[(String, Expr)]) -> Result<Ty, Diag> {
        let fields = self.fields;
        let Some(decl) = fields.get(name) else {
            let hint = match self.is_cap(name) {
                true => NO_CAP_SYNTAX.to_string(),
                false => suggest(name, &sorted_keys(self.structs), "structs"),
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

    /// The type of an assignment target: a `let mut` local or a field chain rooted at one.
    fn place(&mut self, target: &Expr) -> Result<Ty, Diag> {
        match &target.kind {
            ExprKind::Name(name) => {
                let Some(local) = self.lookup(name) else {
                    return Err(self.unknown_name(target.span, name));
                };
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
            _ => Err(err(
                target.span,
                "assigning to indexes is not available yet".to_string(),
                Some("assign to a name or a field"),
            )),
        }
    }

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Expr], expected: Option<&Ty>) -> Result<Ty, Diag> {
        let name = match &callee.kind {
            ExprKind::Name(name) if builtin(name).is_some() => return self.constructor(e, name, args, expected),
            ExprKind::Name(name) => name,
            ExprKind::Path(en, v) => return self.variant_value(e, en, v, args),
            _ => {
                let hint = "call a function by name, like `f(x)`";
                return Err(err(callee.span, "only functions can be called".to_string(), Some(hint)));
            }
        };
        let sigs = self.sigs;
        let Some(sig) = sigs.get(name.as_str()) else {
            let hint = suggest(name, &sorted_keys(sigs), "functions");
            return Err(err(callee.span, format!("unknown function `{name}`"), Some(&hint)));
        };
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
}
