use crate::ast::*;
use crate::parse::parse;
use crate::{Diag, Span};
use std::collections::HashMap;

/// The capability types a program may use. Empty until task 10.
#[derive(Debug, Default)]
pub struct CapTable {}

/// A program that passed `check`. Only `check` builds one.
#[derive(Debug)]
pub struct Program {
    pub(crate) funcs: HashMap<String, Fn>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ty {
    Int,
    Bool,
    Str,
    Unit,
    /// A block that always returns; it matches any type.
    Never,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Int => "Int",
            Ty::Bool => "Bool",
            Ty::Str => "Str",
            Ty::Unit => "()",
            Ty::Never => "!",
        }
    }
}

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

fn want(span: Span, found: Ty, expected: Ty) -> Result<(), Diag> {
    if found == Ty::Never || found == expected {
        Ok(())
    } else {
        let msg = format!("expected {}, found {}", expected.name(), found.name());
        Err(err(span, msg, None))
    }
}

/// The result type of `l op r`, or `None` if the operator does not apply.
fn binary_ty(op: &str, l: Ty, r: Ty) -> Option<Ty> {
    use Ty::*;
    match (op, l, r) {
        ("+", Str, Str) => Some(Str),
        ("+" | "-" | "*" | "/" | "%", Int, Int) => Some(Int),
        ("<" | "<=" | ">" | ">=", Int, Int) => Some(Bool),
        ("&&" | "||", Bool, Bool) => Some(Bool),
        ("==" | "!=", a, b) if a == b && a != Never => Some(Bool),
        _ => None,
    }
}

fn resolve(t: &Type) -> Result<Ty, Diag> {
    match &t.kind {
        TypeKind::Unit => Ok(Ty::Unit),
        TypeKind::Named(n, args) if args.is_empty() => match n.as_str() {
            "Int" => Ok(Ty::Int),
            "Bool" => Ok(Ty::Bool),
            "Str" => Ok(Ty::Str),
            _ => Err(unknown_type(t)),
        },
        TypeKind::Named(..) => Err(unknown_type(t)),
    }
}

fn unknown_type(t: &Type) -> Diag {
    err(
        t.span,
        "type is not available yet".to_string(),
        Some("the types so far are Int, Bool, Str and ()"),
    )
}

fn signature(f: &Fn, sigs: &HashMap<String, Sig>) -> Result<Sig, Diag> {
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
        params.push(resolve(&p.ty)?);
    }
    Ok(Sig {
        params,
        ret: resolve(&f.ret)?,
    })
}

/// Checks `src`. Reports at most one `Diag` per function, in source order.
pub fn check(src: &str, _caps: &CapTable) -> Result<Program, Vec<Diag>> {
    let items = parse(src).map_err(|d| vec![d])?;
    let fns: Vec<Fn> = items.into_iter().map(|Item::Fn(f)| f).collect();

    // Signatures first, so call order in the file does not matter.
    let mut sigs = HashMap::new();
    let mut sig_errs = Vec::new();
    for f in &fns {
        match signature(f, &sigs) {
            Ok(sig) => {
                sigs.insert(f.name.clone(), sig);
                sig_errs.push(None);
            }
            Err(d) => sig_errs.push(Some(d)),
        }
    }

    let mut diags = Vec::new();
    for (f, sig_err) in fns.iter().zip(sig_errs) {
        let mut c = Checker {
            sigs: &sigs,
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
    })
}

struct Checker<'a> {
    sigs: &'a HashMap<String, Sig>,
    scopes: Vec<HashMap<String, Local>>,
    /// Return type of the function being checked.
    ret: Ty,
}

impl Checker<'_> {
    fn function(&mut self, f: &Fn) -> Result<(), Diag> {
        let sig = &self.sigs[f.name.as_str()];
        let params = f.params.iter().zip(&sig.params);
        let params = params.map(|(p, &ty)| (p.name.clone(), Local { ty, mutable: false }));
        self.scopes = vec![params.collect()];
        self.ret = sig.ret;
        let found = self.block(&f.body)?;
        if f.body.tail.is_none() && found == Ty::Unit && sig.ret != Ty::Unit {
            let msg = format!("missing return: `{}` must return {}", f.name, sig.ret.name());
            return Err(err(f.body.span, msg, Some("end the body with a value or `return`")));
        }
        let span = f.body.tail.as_ref().map_or(f.body.span, |t| t.span);
        want(span, found, sig.ret)
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
                    let declared = resolve(t)?;
                    want(init.span, found, declared)?;
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
                let ExprKind::Name(name) = &target.kind else {
                    let msg = "assigning to fields and indexes is not available yet";
                    return Err(err(target.span, msg.to_string(), None));
                };
                let found = self.expr(value)?;
                let Some(local) = self.lookup(name) else {
                    return Err(err(target.span, format!("unknown name `{name}`"), None));
                };
                if !local.mutable {
                    let msg = format!("cannot assign to `{name}`, it is not `mut`");
                    return Err(err(target.span, msg, Some("declare it with `let mut`")));
                }
                let ty = local.ty;
                if *op == "=" {
                    want(value.span, found, ty)?;
                } else if binary_ty(&op[..1], ty, found) != Some(ty) {
                    let msg = format!("`{op}` cannot be applied to {} and {}", ty.name(), found.name());
                    return Err(err(s.span, msg, None));
                }
                Ok(Ty::Unit)
            }
            StmtKind::Return(value) => {
                let found = match value {
                    Some(e) => self.expr(e)?,
                    None => Ty::Unit,
                };
                let ret = self.ret;
                want(value.as_ref().map_or(s.span, |e| e.span), found, ret)?;
                Ok(Ty::Never)
            }
            StmtKind::While { cond, body } => {
                want(cond.span, self.expr(cond)?, Ty::Bool)?;
                want(body.span, self.block(body)?, Ty::Unit)?;
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
                Some(local) => Ok(local.ty),
                None => Err(err(e.span, format!("unknown name `{name}`"), None)),
            },
            ExprKind::Unary(op, x) => {
                let want_ty = if *op == "-" { Ty::Int } else { Ty::Bool };
                want(x.span, self.expr(x)?, want_ty)?;
                Ok(want_ty)
            }
            ExprKind::Binary(op, a, b) => {
                let (l, r) = (self.expr(a)?, self.expr(b)?);
                binary_ty(op, l, r).ok_or_else(|| {
                    let msg = format!("`{op}` cannot be applied to {} and {}", l.name(), r.name());
                    err(e.span, msg, None)
                })
            }
            ExprKind::If(cond, then, els) => {
                want(cond.span, self.expr(cond)?, Ty::Bool)?;
                let t = self.block(then)?;
                let Some(els) = els else {
                    want(then.span, t, Ty::Unit)?;
                    return Ok(Ty::Unit);
                };
                let f = self.expr(els)?;
                match (t, f) {
                    (Ty::Never, _) => Ok(f),
                    (_, Ty::Never) => Ok(t),
                    _ if t == f => Ok(t),
                    _ => {
                        let msg = format!("`if` branches have different types: {} and {}", t.name(), f.name());
                        Err(err(e.span, msg, None))
                    }
                }
            }
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(callee, args) => self.call(e, callee, args),
            ExprKind::Method(..) => Err(err(e.span, "method calls are not available yet".to_string(), None)),
            ExprKind::Field(..) => Err(err(e.span, "field access is not available yet".to_string(), None)),
            ExprKind::Index(..) => Err(err(e.span, "indexing is not available yet".to_string(), None)),
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
        for (a, &param) in args.iter().zip(&sig.params) {
            want(a.span, self.expr(a)?, param)?;
        }
        Ok(sig.ret)
    }
}
