use crate::ast::*;
use crate::parse::parse;
use crate::{Diag, Span};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// The capability types a program may use. Empty until task 8.
#[derive(Debug, Default)]
pub struct CapTable {}

/// A program that passed `check`. Only `check` builds one.
#[derive(Debug)]
pub struct Program {
    pub(crate) funcs: HashMap<String, Fn>,
    pub(crate) structs: HashMap<String, Struct>,
}

#[derive(Debug, Clone, PartialEq)]
enum Ty {
    Int,
    Bool,
    Str,
    Unit,
    Struct(String),
    /// A block that always returns; it matches any type.
    Never,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Ty::Int => f.write_str("Int"),
            Ty::Bool => f.write_str("Bool"),
            Ty::Str => f.write_str("Str"),
            Ty::Unit => f.write_str("()"),
            Ty::Struct(name) => f.write_str(name),
            Ty::Never => f.write_str("!"),
        }
    }
}

/// Resolved field names and types of each struct, in declaration order.
type Fields = HashMap<String, Vec<(String, Ty)>>;

struct Sig {
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

/// The result type of `l op r`, or `None` if the operator does not apply.
fn binary_ty(op: &str, l: &Ty, r: &Ty) -> Option<Ty> {
    use Ty::*;
    match (op, l, r) {
        ("+", Str, Str) => Some(Str),
        ("+" | "-" | "*" | "/" | "%", Int, Int) => Some(Int),
        ("<" | "<=" | ">" | ">=", Int, Int) => Some(Bool),
        ("&&" | "||", Bool, Bool) => Some(Bool),
        ("==" | "!=", a, b) if a == b && !matches!(a, Never | Struct(_)) => Some(Bool),
        _ => None,
    }
}

fn resolve(t: &Type, structs: &HashMap<String, Struct>) -> Result<Ty, Diag> {
    match &t.kind {
        TypeKind::Unit => Ok(Ty::Unit),
        TypeKind::Named(n, args) if args.is_empty() => match n.as_str() {
            "Int" => Ok(Ty::Int),
            "Bool" => Ok(Ty::Bool),
            "Str" => Ok(Ty::Str),
            _ if structs.contains_key(n) => Ok(Ty::Struct(n.clone())),
            _ => Err(unknown_type(t)),
        },
        TypeKind::Named(..) => Err(unknown_type(t)),
    }
}

fn unknown_type(t: &Type) -> Diag {
    err(
        t.span,
        "type is not available yet".to_string(),
        Some("the types so far are Int, Bool, Str, () and your structs"),
    )
}

fn signature(f: &Fn, sigs: &HashMap<String, Sig>, structs: &HashMap<String, Struct>) -> Result<Sig, Diag> {
    if sigs.contains_key(&f.name) {
        return Err(err(f.span, format!("duplicate function `{}`", f.name), None));
    }
    if f.name == "main" && !f.params.is_empty() {
        return Err(err(f.span, "`main` takes no parameters yet".to_string(), None));
    }
    let mut params = Vec::new();
    for (i, p) in f.params.iter().enumerate() {
        if f.params[..i].iter().any(|q| q.name == p.name) {
            return Err(err(p.span, format!("duplicate parameter `{}`", p.name), None));
        }
        params.push(resolve(&p.ty, structs)?);
    }
    Ok(Sig {
        params,
        ret: resolve(&f.ret, structs)?,
    })
}

fn struct_fields(s: &Struct, structs: &HashMap<String, Struct>) -> Result<Vec<(String, Ty)>, Diag> {
    let mut fields: Vec<(String, Ty)> = Vec::new();
    for p in &s.fields {
        if fields.iter().any(|(name, _)| *name == p.name) {
            return Err(err(p.span, format!("duplicate field `{}`", p.name), None));
        }
        fields.push((p.name.clone(), resolve(&p.ty, structs)?));
    }
    Ok(fields)
}

/// Whether `name` reaches itself through its field types.
fn contains_itself(name: &str, fields: &Fields) -> bool {
    let mut todo = vec![name];
    let mut seen = HashSet::new();
    while let Some(n) = todo.pop() {
        for (_, ty) in fields.get(n).into_iter().flatten() {
            if let Ty::Struct(m) = ty {
                if m == name {
                    return true;
                }
                if seen.insert(m.as_str()) {
                    todo.push(m);
                }
            }
        }
    }
    false
}

fn no_field(span: Span, field: &str, ty: &str, decl: &[(String, Ty)]) -> Diag {
    let names: Vec<_> = decl.iter().map(|(n, _)| n.as_str()).collect();
    let hint = format!("fields: {}", names.join(", "));
    err(span, format!("no field `{field}` on {ty}"), Some(&hint))
}

/// Checks `src`. Reports at most one `Diag` per function, in source order.
pub fn check(src: &str, _caps: &CapTable) -> Result<Program, Vec<Diag>> {
    let items = parse(src).map_err(|d| vec![d])?;
    let mut fns = Vec::new();
    let mut structs = HashMap::new();
    let mut order = Vec::new();
    let mut diags = Vec::new();
    for item in items {
        match item {
            Item::Fn(f) => fns.push(f),
            Item::Struct(s) if structs.contains_key(&s.name) => {
                diags.push(err(s.span, format!("duplicate struct `{}`", s.name), None));
            }
            Item::Struct(s) => {
                order.push(s.name.clone());
                structs.insert(s.name.clone(), s);
            }
        }
    }

    // Structs before signatures, which may name them.
    let mut fields = Fields::new();
    for name in &order {
        match struct_fields(&structs[name], &structs) {
            Ok(f) => {
                fields.insert(name.clone(), f);
            }
            Err(d) => diags.push(d),
        }
    }
    for name in &order {
        if contains_itself(name, &fields) {
            let msg = format!("struct `{name}` contains itself");
            diags.push(err(structs[name].span, msg, Some("use indexes into a `List` instead")));
        }
    }
    if !diags.is_empty() {
        return Err(diags);
    }

    // Signatures first, so call order in the file does not matter.
    let mut sigs = HashMap::new();
    let mut sig_errs = Vec::new();
    for f in &fns {
        match signature(f, &sigs, &structs) {
            Ok(sig) => {
                sigs.insert(f.name.clone(), sig);
                sig_errs.push(None);
            }
            Err(d) => sig_errs.push(Some(d)),
        }
    }

    for (f, sig_err) in fns.iter().zip(sig_errs) {
        let mut c = Checker {
            sigs: &sigs,
            fields: &fields,
            structs: &structs,
            scopes: Vec::new(),
            ret: Ty::Unit,
        };
        diags.extend(sig_err.or_else(|| c.function(f).err()));
    }
    if !fns.iter().any(|f| f.name == "main") {
        let none = Span { start: 0, end: 0 };
        diags.push(err(none, "no `main` function".to_string(), None));
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(Program {
        funcs: fns.into_iter().map(|f| (f.name.clone(), f)).collect(),
        structs,
    })
}

struct Checker<'a> {
    sigs: &'a HashMap<String, Sig>,
    fields: &'a Fields,
    structs: &'a HashMap<String, Struct>,
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
        let found = self.block(&f.body)?;
        if f.body.tail.is_none() && found == Ty::Unit && sig.ret != Ty::Unit {
            let msg = format!("missing return: `{}` must return {}", f.name, sig.ret);
            return Err(err(f.body.span, msg, Some("end the body with a value or `return`")));
        }
        let span = f.body.tail.as_ref().map_or(f.body.span, |t| t.span);
        want(span, &found, &sig.ret)
    }

    fn lookup(&mut self, name: &str) -> Option<&mut Local> {
        self.scopes.iter_mut().rev().find_map(|s| s.get_mut(name))
    }

    fn block(&mut self, b: &Block) -> Result<Ty, Diag> {
        self.scopes.push(HashMap::new());
        let mut last = Ty::Unit;
        for s in &b.stmts {
            last = self.stmt(s)?;
        }
        let ty = match &b.tail {
            Some(t) => self.expr(t)?,
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
                let mut found = self.expr(init)?;
                if let Some(t) = ty {
                    let declared = resolve(t, self.structs)?;
                    want(init.span, &found, &declared)?;
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
                let found = self.expr(value)?;
                let ty = self.place(target)?;
                if *op == "=" {
                    want(value.span, &found, &ty)?;
                } else if binary_ty(&op[..1], &ty, &found) != Some(ty.clone()) {
                    let msg = format!("`{op}` cannot be applied to {ty} and {found}");
                    return Err(err(s.span, msg, None));
                }
                Ok(Ty::Unit)
            }
            StmtKind::Return(value) => {
                let found = match value {
                    Some(e) => self.expr(e)?,
                    None => Ty::Unit,
                };
                want(value.as_ref().map_or(s.span, |e| e.span), &found, &self.ret)?;
                Ok(Ty::Never)
            }
            StmtKind::While { cond, body } => {
                want(cond.span, &self.expr(cond)?, &Ty::Bool)?;
                want(body.span, &self.block(body)?, &Ty::Unit)?;
                Ok(Ty::Unit)
            }
            StmtKind::Expr(e) => Ok(match self.expr(e)? {
                Ty::Never => Ty::Never,
                _ => Ty::Unit,
            }),
        }
    }

    fn expr(&mut self, e: &Expr) -> Result<Ty, Diag> {
        match &e.kind {
            ExprKind::Int(_) => Ok(Ty::Int),
            ExprKind::Str(_) => Ok(Ty::Str),
            ExprKind::Bool(_) => Ok(Ty::Bool),
            ExprKind::Unit => Ok(Ty::Unit),
            ExprKind::Name(name) => match self.lookup(name) {
                Some(local) => Ok(local.ty.clone()),
                None => Err(err(e.span, format!("unknown name `{name}`"), None)),
            },
            ExprKind::Unary(op, x) => {
                let want_ty = if *op == "-" { Ty::Int } else { Ty::Bool };
                want(x.span, &self.expr(x)?, &want_ty)?;
                Ok(want_ty)
            }
            ExprKind::Binary(op, a, b) => {
                let (l, r) = (self.expr(a)?, self.expr(b)?);
                binary_ty(op, &l, &r).ok_or_else(|| {
                    let msg = format!("`{op}` cannot be applied to {l} and {r}");
                    err(e.span, msg, None)
                })
            }
            ExprKind::If(cond, then, els) => {
                want(cond.span, &self.expr(cond)?, &Ty::Bool)?;
                let t = self.block(then)?;
                let Some(els) = els else {
                    want(then.span, &t, &Ty::Unit)?;
                    return Ok(Ty::Unit);
                };
                let f = self.expr(els)?;
                if t == Ty::Never {
                    Ok(f)
                } else if f == Ty::Never || t == f {
                    Ok(t)
                } else {
                    let msg = format!("`if` branches have different types: {t} and {f}");
                    Err(err(e.span, msg, None))
                }
            }
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(callee, args) => self.call(e, callee, args),
            ExprKind::Method(..) => Err(err(e.span, "method calls are not available yet".to_string(), None)),
            ExprKind::Field(base, name) => {
                let ty = self.expr(base)?;
                self.field(&ty, name, e.span)
            }
            ExprKind::Index(..) => Err(err(e.span, "indexing is not available yet".to_string(), None)),
            ExprKind::StructLit(name, inits) => self.struct_lit(e, name, inits),
        }
    }

    /// The type of field `name` of a value of type `ty`.
    fn field(&self, ty: &Ty, name: &str, span: Span) -> Result<Ty, Diag> {
        let Ty::Struct(s) = ty else {
            return Err(err(span, format!("no field `{name}` on {ty}"), None));
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
            return Err(err(e.span, format!("unknown struct `{name}`"), None));
        };
        let mut seen = Vec::new();
        for (f, x) in inits {
            let Some((_, ty)) = decl.iter().find(|(n, _)| n == f) else {
                return Err(no_field(x.span, f, name, decl));
            };
            if seen.contains(&f) {
                return Err(err(x.span, format!("duplicate field `{f}`"), None));
            }
            seen.push(f);
            want(x.span, &self.expr(x)?, ty)?;
        }
        if let Some((missing, _)) = decl.iter().find(|(n, _)| !seen.contains(&n)) {
            return Err(err(e.span, format!("missing field `{missing}` in `{name}`"), None));
        }
        Ok(Ty::Struct(name.to_string()))
    }

    /// The type of an assignment target: a `let mut` local or a field chain rooted at one.
    fn place(&mut self, target: &Expr) -> Result<Ty, Diag> {
        match &target.kind {
            ExprKind::Name(name) => {
                let Some(local) = self.lookup(name) else {
                    return Err(err(target.span, format!("unknown name `{name}`"), None));
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
                None,
            )),
        }
    }

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Expr]) -> Result<Ty, Diag> {
        let ExprKind::Name(name) = &callee.kind else {
            return Err(err(callee.span, "only functions can be called".to_string(), None));
        };
        let sigs = self.sigs;
        let Some(sig) = sigs.get(name.as_str()) else {
            return Err(err(callee.span, format!("unknown function `{name}`"), None));
        };
        if args.len() != sig.params.len() {
            let msg = format!("`{name}` takes {} arguments, found {}", sig.params.len(), args.len());
            return Err(err(e.span, msg, None));
        }
        for (a, param) in args.iter().zip(&sig.params) {
            want(a.span, &self.expr(a)?, param)?;
        }
        Ok(sig.ret.clone())
    }
}
