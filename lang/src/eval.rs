use crate::ast::*;
use crate::check::{builtin, key, module_of, Program};
use crate::Span;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

/// Bounds Rust recursion, and so stack use, for any program.
const MAX_DEPTH: usize = 3_000;

/// What the host grants and does for `main`'s capability parameters.
pub trait Caps {
    /// Whether the host granted a capability of type `ty` for parameter `name`.
    fn has(&self, name: &str, ty: &str) -> bool;
    /// Runs `method` on the capability granted as `cap`. `Ok` must have the method's declared
    /// type; `Err` is a host failure that stops the run.
    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    Unit,
    List(Vec<Value>),
    /// Ordered by key, so `keys()` and iteration are deterministic.
    Map(BTreeMap<Key, Value>),
    /// The struct's name and its field values in declaration order.
    Struct(String, Vec<Value>),
    /// The enum's name, the variant's index in declaration order and its payload.
    Enum(String, usize, Vec<Value>),
    /// A capability, named by the `main` parameter it came from.
    Cap(String),
}

/// A `Map` key: an `Int` or a `Str`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Int(i64),
    Str(String),
}

impl Key {
    fn from_value(v: Value) -> Key {
        match v {
            Value::Int(n) => Key::Int(n),
            Value::Str(s) => Key::Str(s),
            _ => unreachable!("checked key"),
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        match self {
            Key::Int(n) => Value::Int(*n),
            Key::Str(s) => Value::Str(s.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunErr {
    pub span: Span,
    pub msg: String,
}

/// Why evaluation left the normal path.
enum Stop {
    Return(Value),
    Break,
    Continue,
    Err(RunErr),
}

type Res = Result<Value, Box<Stop>>;

fn fail(span: Span, msg: &str) -> Box<Stop> {
    Box::new(Stop::Err(RunErr {
        span,
        msg: msg.to_string(),
    }))
}

/// One step from a local to a part of it, with indexes already evaluated.
#[derive(Clone, Copy)]
enum Step<'a> {
    Field(&'a str),
    /// The index and the span of the expression that indexes.
    Index(i64, Span),
}

/// Evaluates `main`. Recursion is capped at `MAX_DEPTH` nested expressions, which needs
/// up to 8 MB of stack in debug builds and 3 MB in release. Callers provide that stack.
pub fn run(p: &Program, caps: &mut dyn Caps) -> Result<Value, RunErr> {
    let main = &p.funcs["main"];
    let mut locals = Vec::new();
    for param in &main.params {
        let TypeKind::Named(ty, _) = &param.ty.kind else {
            unreachable!("checked parameter");
        };
        if !caps.has(&param.name, ty) {
            return Err(RunErr {
                span: param.span,
                msg: format!("no grant for `{}: {ty}`", param.name),
            });
        }
        locals.push((param.name.clone(), Value::Cap(param.name.clone())));
    }
    let mut it = Interp {
        program: p,
        funcs: &p.funcs,
        structs: &p.structs,
        enums: &p.enums,
        caps,
        module: "",
        locals,
        base: 0,
        depth: 0,
    };
    match returned(it.block(&main.body)) {
        Ok(v) => Ok(v),
        Err(s) => match *s {
            Stop::Err(e) => Err(e),
            Stop::Return(_) | Stop::Break | Stop::Continue => unreachable!(),
        },
    }
}

struct Interp<'a> {
    program: &'a Program,
    funcs: &'a HashMap<String, Fn>,
    structs: &'a HashMap<String, Struct>,
    enums: &'a HashMap<String, Enum>,
    caps: &'a mut dyn Caps,
    /// Module of the running function; "" for the root.
    module: &'a str,
    /// Locals of all running calls, innermost last; a block truncates it on exit.
    locals: Vec<(String, Value)>,
    /// Index of the running function's first local; callers' locals below it are invisible.
    base: usize,
    /// Nested `expr` calls in progress.
    depth: usize,
}

impl Interp<'_> {
    /// Evaluates `args` in the caller's frame, then starts a frame for `f`.
    /// Returns the caller's `base`.
    fn enter(&mut self, f: &Fn, args: &[Expr]) -> Result<usize, Box<Stop>> {
        let vals = args.iter().map(|a| self.expr(a)).collect::<Result<Vec<_>, _>>()?;
        let caller = self.base;
        self.base = self.locals.len();
        self.locals.extend(f.params.iter().map(|p| p.name.clone()).zip(vals));
        Ok(caller)
    }

    fn leave(&mut self, caller: usize) {
        self.locals.truncate(self.base);
        self.base = caller;
    }

    fn lookup(&mut self, name: &str) -> &mut Value {
        let locals = &mut self.locals[self.base..];
        let local = locals.iter_mut().rev().find(|(n, _)| n == name);
        &mut local.expect("checked name").1
    }

    /// A `Stop` skips the truncate; `leave` does it.
    fn block(&mut self, b: &Block) -> Res {
        let mark = self.locals.len();
        self.stmts(&b.stmts)?;
        let v = match &b.tail {
            Some(t) => self.expr(t)?,
            None => Value::Unit,
        };
        self.locals.truncate(mark);
        Ok(v)
    }

    fn stmts(&mut self, stmts: &[Stmt]) -> Result<(), Box<Stop>> {
        for s in stmts {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), Box<Stop>> {
        match &s.kind {
            StmtKind::Let { name, init, .. } => self.let_stmt(name, init),
            StmtKind::Assign { target, op, value } => self.assign(target, op, value, s.span),
            StmtKind::Return(value) => self.return_stmt(value.as_ref()),
            StmtKind::While { cond, body } => self.while_stmt(cond, body),
            StmtKind::For { name, iter, body } => self.for_stmt(name, iter, body),
            StmtKind::Break => Err(Box::new(Stop::Break)),
            StmtKind::Continue => Err(Box::new(Stop::Continue)),
            StmtKind::Expr(e) => self.expr(e).map(drop),
        }
    }

    fn let_stmt(&mut self, name: &str, init: &Expr) -> Result<(), Box<Stop>> {
        let v = self.expr(init)?;
        self.locals.push((name.to_string(), v));
        Ok(())
    }

    fn return_stmt(&mut self, value: Option<&Expr>) -> Result<(), Box<Stop>> {
        let v = match value {
            Some(e) => self.expr(e)?,
            None => Value::Unit,
        };
        Err(Box::new(Stop::Return(v)))
    }

    /// Splits a place into its root local and the steps from it, evaluating indexes in source order.
    fn path<'e>(&mut self, place: &'e Expr) -> Result<(&'e str, Vec<Step<'e>>), Box<Stop>> {
        let mut chain = Vec::new();
        let mut root = place;
        while let ExprKind::Field(base, _) | ExprKind::Index(base, _) = &root.kind {
            chain.push(root);
            root = base;
        }
        let ExprKind::Name(name) = &root.kind else {
            unreachable!("checked place");
        };
        let mut steps = Vec::new();
        for e in chain.into_iter().rev() {
            steps.push(match &e.kind {
                ExprKind::Field(_, field) => Step::Field(field),
                ExprKind::Index(_, index) => {
                    let Value::Int(i) = self.expr(index)? else {
                        unreachable!("checked index");
                    };
                    Step::Index(i, e.span)
                }
                _ => unreachable!("chain holds fields and indexes"),
            });
        }
        Ok((name, steps))
    }

    /// The part of local `root` that `steps` lead to.
    fn slot(&mut self, root: &str, steps: &[Step]) -> Result<&mut Value, Box<Stop>> {
        let structs = self.structs;
        let mut slot = self.lookup(root);
        for &step in steps {
            slot = match (step, slot) {
                (Step::Field(field), Value::Struct(s, vals)) => &mut vals[field_index(structs, s, field)],
                (Step::Index(i, span), Value::List(vals)) => {
                    let len = vals.len();
                    match usize::try_from(i).ok().and_then(|i| vals.get_mut(i)) {
                        Some(v) => v,
                        None => return Err(fail(span, &format!("index {i} out of bounds for length {len}"))),
                    }
                }
                // A byte is not a `Value`: bounds-check and stay on the `Bytes`, which `read` indexes.
                (Step::Index(i, span), slot @ Value::Bytes(_)) => {
                    let Value::Bytes(b) = &*slot else { unreachable!() };
                    if usize::try_from(i).map_or(true, |i| i >= b.len()) {
                        return Err(fail(span, &format!("index {i} out of bounds for length {}", b.len())));
                    }
                    slot
                }
                _ => unreachable!("checked place"),
            };
        }
        Ok(slot)
    }

    /// Like `path`, for reading `place` in the local instead of from a copy of it. `None`, with
    /// nothing evaluated, unless `place` is a local with fields and indexes and neither its indexes
    /// nor the `later` operands can assign: a copy would have been taken before they ran.
    /// Fails where evaluating a copy would, in the same order. `entry` is the depth `place` is
    /// evaluated at.
    fn walk<'e>(
        &mut self,
        place: &'e Expr,
        entry: usize,
        later: &[Expr],
    ) -> Result<Option<(&'e str, Vec<Step<'e>>)>, Box<Stop>> {
        let mut chain = Vec::new();
        let mut root = place;
        while let ExprKind::Field(base, _) | ExprKind::Index(base, _) = &root.kind {
            chain.push(root);
            root = base;
        }
        let ExprKind::Name(name) = &root.kind else {
            return Ok(None);
        };
        let pure_index = |e: &&Expr| !matches!(&e.kind, ExprKind::Index(_, i) if !pure(i));
        if builtin(name).is_some() || !chain.iter().all(pure_index) || !later.iter().all(pure) {
            return Ok(None);
        }
        chain.push(root);
        if let Some(node) = chain.get(MAX_DEPTH - entry) {
            return Err(fail(node.span, "evaluation is too deep"));
        }
        let keep = self.depth;
        let mut steps = Vec::new();
        for (j, e) in chain.iter().enumerate().rev().skip(1) {
            steps.push(match &e.kind {
                ExprKind::Field(_, field) => Step::Field(field),
                ExprKind::Index(_, index) => {
                    self.depth = entry + j + 1;
                    let i = self.expr(index);
                    self.depth = keep;
                    let Value::Int(i) = i? else {
                        unreachable!("checked index");
                    };
                    Step::Index(i, e.span)
                }
                _ => unreachable!("chain holds fields and indexes"),
            });
            self.slot(name, &steps)?;
        }
        Ok(Some((name, steps)))
    }

    /// A copy of the field or element `e` names, without copying the rest of its local.
    fn read(&mut self, e: &Expr) -> Result<Option<Value>, Box<Stop>> {
        let Some((root, steps)) = self.walk(e, self.depth - 1, &[])? else {
            return Ok(None);
        };
        let (last, parents) = steps.split_last().expect("a field or index has a step");
        if let (Step::Index(i, _), Value::Bytes(b)) = (*last, self.slot(root, parents)?) {
            return Ok(Some(Value::Int(i64::from(b[i as usize]))));
        }
        Ok(Some(self.slot(root, &steps)?.clone()))
    }

    #[inline(never)]
    fn assign(&mut self, target: &Expr, op: &str, value: &Expr, span: Span) -> Result<(), Box<Stop>> {
        let (root, steps) = self.path(target)?;
        let mut v = self.expr(value)?;
        let slot = self.slot(root, &steps)?;
        if op != "=" {
            v = binary(&op[..1], slot.clone(), v, span)?;
        }
        *slot = v;
        Ok(())
    }

    fn while_stmt(&mut self, cond: &Expr, body: &Block) -> Result<(), Box<Stop>> {
        while self.expr(cond)? == Value::Bool(true) {
            if !self.iteration(body)? {
                break;
            }
        }
        Ok(())
    }

    /// Runs `name` over `iter`; a list is the value it had when the loop started.
    #[inline(never)]
    fn for_stmt(&mut self, name: &str, iter: &Iter, body: &Block) -> Result<(), Box<Stop>> {
        let items: Box<dyn Iterator<Item = Value>> = match iter {
            Iter::List(e) => {
                let Value::List(vals) = self.expr(e)? else {
                    unreachable!("checked list");
                };
                Box::new(vals.into_iter())
            }
            Iter::Range(a, b) => {
                let (Value::Int(a), Value::Int(b)) = (self.expr(a)?, self.expr(b)?) else {
                    unreachable!("checked range");
                };
                Box::new((a..b).map(Value::Int))
            }
        };
        for v in items {
            self.locals.push((name.to_string(), v));
            let go = self.iteration(body)?;
            self.locals.pop();
            if !go {
                break;
            }
        }
        Ok(())
    }

    /// Runs one pass of a loop body. False when it ends the loop with `break`.
    fn iteration(&mut self, body: &Block) -> Result<bool, Box<Stop>> {
        let mark = self.locals.len();
        match self.block(body) {
            Err(s) => match *s {
                Stop::Break => {
                    self.locals.truncate(mark);
                    Ok(false)
                }
                Stop::Continue => {
                    self.locals.truncate(mark);
                    Ok(true)
                }
                e => Err(Box::new(e)),
            },
            Ok(_) => Ok(true),
        }
    }

    fn expr(&mut self, e: &Expr) -> Res {
        if self.depth == MAX_DEPTH {
            return Err(fail(e.span, "evaluation is too deep"));
        }
        self.depth += 1;
        let v = self.expr_kind(e);
        self.depth -= 1;
        v
    }

    fn expr_kind(&mut self, e: &Expr) -> Res {
        match &e.kind {
            ExprKind::Name(name) if builtin(name).is_some() => Ok(constructor(name, Vec::new())),
            ExprKind::Name(name) => Ok(self.lookup(name).clone()),
            ExprKind::Int(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Unit => Ok(literal(e)),
            ExprKind::Unary(op, x) => self.unary(op, x, e.span),
            ExprKind::Binary(op @ ("&&" | "||"), a, b) => self.logical(op, a, b),
            ExprKind::Binary(op, a, b) => self.arith(op, a, b, e.span),
            ExprKind::If(cond, then, els) => self.if_expr(cond, then, els.as_deref()),
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(callee, args) => self.call_expr(callee, args),
            ExprKind::Field(base, field) => match self.read(e)? {
                Some(v) => Ok(v),
                None => self.field(base, field),
            },
            ExprKind::StructLit(name, inits) => self.struct_lit(&key(self.module, name), inits),
            ExprKind::Path(en, v) => Ok(self.variant(&self.enum_key(en), v, Vec::new())),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::StructLit(name, inits) => self.struct_lit(&format!("{m}::{name}"), inits),
                ExprKind::Path(en, v) => Ok(self.variant(&format!("{m}::{en}"), v, Vec::new())),
                _ => unreachable!("checked module path"),
            },
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms),
            ExprKind::Try(x) => self.try_expr(x),
            ExprKind::Method(recv, name, args) => self.method(recv, name, args, e.span),
            ExprKind::Index(base, index) => match self.read(e)? {
                Some(v) => Ok(v),
                None => self.index(base, index, e.span),
            },
            ExprKind::List(items) => Ok(Value::List(self.values(items)?)),
        }
    }

    /// The capability type of `main`'s parameter `cap`, such as `Dir`.
    fn cap_type(&self, cap: &str) -> String {
        let param = self.funcs["main"].params.iter().find(|p| p.name == cap);
        match param.map(|p| &p.ty.kind) {
            Some(TypeKind::Named(ty, _)) => ty.clone(),
            _ => unreachable!("Value::Cap names a main parameter"),
        }
    }

    /// The key of enum `en` as written in the running module; built-in enums keep their name.
    fn enum_key(&self, en: &str) -> String {
        let k = key(self.module, en).into_owned();
        if self.enums.contains_key(&k) {
            k
        } else {
            en.to_string()
        }
    }

    fn values(&mut self, exprs: &[Expr]) -> Result<Vec<Value>, Box<Stop>> {
        exprs.iter().map(|x| self.expr(x)).collect()
    }

    #[inline(never)]
    fn index(&mut self, base: &Expr, index: &Expr, span: Span) -> Res {
        let (base, Value::Int(i)) = (self.expr(base)?, self.expr(index)?) else {
            unreachable!("checked index");
        };
        let len = match &base {
            Value::List(vals) => vals.len(),
            Value::Bytes(b) => b.len(),
            _ => unreachable!("checked index"),
        };
        let Some(at) = usize::try_from(i).ok().filter(|&i| i < len) else {
            return Err(fail(span, &format!("index {i} out of bounds for length {len}")));
        };
        Ok(match base {
            Value::List(mut vals) => vals.swap_remove(at),
            Value::Bytes(b) => Value::Int(i64::from(b[at])),
            _ => unreachable!("checked index"),
        })
    }

    /// `push`, `insert` and `remove` change their receiver in place; the checker only allows
    /// them on a place. Other built-in methods work on the value; capability methods go to the host.
    #[inline(never)]
    fn method(&mut self, recv: &Expr, name: &str, args: &[Expr], span: Span) -> Res {
        // A capability could have its own `push`, so only a receiver that is not a capability
        // takes this path.
        let on_cap = matches!(&recv.kind, ExprKind::Name(n) if matches!(self.lookup(n), Value::Cap(_)));
        if matches!(name, "push" | "insert" | "remove") && !on_cap {
            let (root, steps) = self.path(recv)?;
            let mut args = self.values(args)?.into_iter();
            let (a, b) = (args.next(), args.next());
            match (self.slot(root, &steps)?, name, a, b) {
                (Value::List(vals), "push", Some(item), _) => vals.push(item),
                (Value::Bytes(b), "push", Some(Value::Int(n)), _) => match u8::try_from(n) {
                    Ok(n) => b.push(n),
                    Err(_) => return Err(fail(span, &format!("byte {n} out of range 0..=255"))),
                },
                (Value::Map(map), "insert", Some(k), Some(v)) => {
                    map.insert(Key::from_value(k), v);
                }
                (Value::Map(map), "remove", Some(k), _) => {
                    map.remove(&Key::from_value(k));
                }
                _ => unreachable!("checked mutation"),
            }
            return Ok(Value::Unit);
        }
        let place = if on_cap {
            None
        } else {
            self.walk(recv, self.depth, args)?
        };
        let Some((root, steps)) = place else {
            let recv = self.expr(recv)?;
            let vals = self.values(args)?;
            if let Value::Cap(cap) = recv {
                let v = self.caps.call(&cap, name, vals).map_err(|msg| fail(span, &msg))?;
                // The host is trusted code but checked anyway: a value of the wrong type stops the run.
                let ty = &self.program.cap_methods[&(self.cap_type(&cap), name.to_string())];
                if !self.program.fits(&v, ty) {
                    return Err(fail(span, &format!("host returned a malformed value for `{name}`")));
                }
                return Ok(v);
            }
            return builtin_method(&recv, name, vals, span);
        };
        let vals = self.values(args)?;
        builtin_method(self.slot(root, &steps)?, name, vals, span)
    }

    #[inline(never)]
    fn field(&mut self, base: &Expr, field: &str) -> Res {
        let Value::Struct(s, mut vals) = self.expr(base)? else {
            unreachable!("checked field");
        };
        Ok(vals.swap_remove(field_index(self.structs, &s, field)))
    }

    /// Fields are evaluated in source order and stored in declaration order.
    #[inline(never)]
    fn struct_lit(&mut self, name: &str, inits: &[(String, Expr)]) -> Res {
        let mut vals = vec![Value::Unit; inits.len()];
        for (field, x) in inits {
            vals[field_index(self.structs, name, field)] = self.expr(x)?;
        }
        Ok(Value::Struct(name.to_string(), vals))
    }

    #[inline(never)]
    fn variant(&self, en: &str, v: &str, payload: Vec<Value>) -> Value {
        let i = variant_index(self.enums, en, v).expect("checked variant");
        Value::Enum(en.to_string(), i, payload)
    }

    /// `Some` and `Ok` yield their payload; `None` and `Err` return from the function.
    fn try_expr(&mut self, x: &Expr) -> Res {
        let Value::Enum(en, i, mut payload) = self.expr(x)? else {
            unreachable!("checked operand");
        };
        let success = if en == "Option" { "Some" } else { "Ok" };
        if builtin(success) == Some((en.as_str(), i)) {
            return Ok(payload.remove(0));
        }
        Err(Box::new(Stop::Return(Value::Enum(en, i, payload))))
    }

    /// The first arm whose pattern matches runs, with its bindings as locals.
    #[inline(never)]
    fn match_expr(&mut self, scrut: &Expr, arms: &[Arm]) -> Res {
        let v = self.expr(scrut)?;
        for arm in arms {
            let mark = self.locals.len();
            if self.bind(&arm.pat, &v) {
                let r = self.expr(&arm.body)?;
                self.locals.truncate(mark);
                return Ok(r);
            }
        }
        unreachable!("checked exhaustive")
    }

    /// Whether `v` matches `p`; on a match, pushes the pattern's bindings.
    fn bind(&mut self, p: &Pat, v: &Value) -> bool {
        match (&p.kind, v) {
            (PatKind::Wild, _) => true,
            (PatKind::Int(a), Value::Int(b)) => a == b,
            (PatKind::Str(a), Value::Str(b)) => a == b,
            (PatKind::Bool(a), Value::Bool(b)) => a == b,
            (PatKind::Name(n), _) => {
                if let Value::Enum(en, i, _) = v {
                    if let Some(j) = variant_index(self.enums, en, n) {
                        return j == *i;
                    }
                }
                self.locals.push((n.clone(), v.clone()));
                true
            }
            (PatKind::Module(_, inner), _) => self.bind(inner, v),
            (PatKind::Variant(_, name, names), Value::Enum(en, i, payload)) => {
                if variant_index(self.enums, en, name) != Some(*i) {
                    return false;
                }
                for (n, x) in names.iter().zip(payload) {
                    self.locals.extend(n.clone().map(|n| (n, x.clone())));
                }
                true
            }
            _ => unreachable!("checked pattern"),
        }
    }

    fn call_expr(&mut self, callee: &Expr, args: &[Expr]) -> Res {
        let k = match &callee.kind {
            ExprKind::Name(name) if builtin(name).is_some() => {
                let vals = args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?;
                return Ok(constructor(name, vals));
            }
            ExprKind::Name(name) => key(self.module, name),
            ExprKind::Path(en, _) if en == "Map" => return Ok(Value::Map(BTreeMap::new())),
            ExprKind::Path(en, v) => {
                let vals = args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?;
                return Ok(self.variant(&self.enum_key(en), v, vals));
            }
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::Name(name) => Cow::Owned(format!("{m}::{name}")),
                ExprKind::Path(en, v) => {
                    let vals = args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?;
                    return Ok(self.variant(&format!("{m}::{en}"), v, vals));
                }
                _ => unreachable!("checked callee"),
            },
            _ => unreachable!("checked callee"),
        };
        let funcs = self.funcs;
        let (k, f) = funcs.get_key_value(&*k).expect("checked function");
        let caller = self.enter(f, args)?;
        let module = std::mem::replace(&mut self.module, module_of(k));
        let result = self.block(&f.body);
        self.module = module;
        self.leave(caller);
        returned(result)
    }

    fn unary(&mut self, op: &str, x: &Expr, span: Span) -> Res {
        let v = self.expr(x)?;
        negate(op, v, span)
    }

    /// `&&` and `||` skip the right side when the left decides the result.
    fn logical(&mut self, op: &str, a: &Expr, b: &Expr) -> Res {
        match self.expr(a)? {
            Value::Bool(l) if l == (op == "||") => Ok(Value::Bool(l)),
            _ => self.expr(b),
        }
    }

    fn arith(&mut self, op: &str, a: &Expr, b: &Expr, span: Span) -> Res {
        let l = self.expr(a)?;
        binary(op, l, self.expr(b)?, span)
    }

    fn if_expr(&mut self, cond: &Expr, then: &Block, els: Option<&Expr>) -> Res {
        if self.expr(cond)? == Value::Bool(true) {
            return self.block(then);
        }
        match els.map(|e| &e.kind) {
            None => Ok(Value::Unit),
            Some(ExprKind::Block(b)) => self.block(b),
            Some(ExprKind::If(c, t, e)) => self.if_expr(c, t, e.as_deref()),
            Some(_) => unreachable!("else is a block or if"),
        }
    }
}

fn field_index(structs: &HashMap<String, Struct>, name: &str, field: &str) -> usize {
    let fields = &structs[name].fields;
    fields.iter().position(|f| f.name == field).expect("checked field")
}

fn variant_index(enums: &HashMap<String, Enum>, name: &str, variant: &str) -> Option<usize> {
    if let Some((en, i)) = builtin(variant) {
        if en == name {
            return Some(i);
        }
    }
    enums.get(name)?.variants.iter().position(|v| v.name == variant)
}

/// A `return` ends the call with its value; errors keep unwinding.
fn returned(r: Res) -> Res {
    match r {
        Err(s) => match *s {
            Stop::Return(v) => Ok(v),
            e => Err(Box::new(e)),
        },
        ok => ok,
    }
}

fn literal(e: &Expr) -> Value {
    match &e.kind {
        ExprKind::Int(n) => Value::Int(*n),
        ExprKind::Str(s) => Value::Str(s.clone()),
        ExprKind::Bool(b) => Value::Bool(*b),
        _ => Value::Unit,
    }
}

fn negate(op: &str, x: Value, span: Span) -> Res {
    match (op, x) {
        ("-", Value::Int(n)) => n
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| fail(span, "Int overflow")),
        (_, Value::Bool(b)) => Ok(Value::Bool(!b)),
        _ => unreachable!("checked operand"),
    }
}

/// A built-in method on `recv`, which may be part of a local.
fn builtin_method(recv: &Value, name: &str, mut args: Vec<Value>, span: Span) -> Res {
    Ok(match (recv, name) {
        (Value::Int(n), "to_str") => Value::Str(n.to_string()),
        (Value::Str(s), "len") => Value::Int(s.len() as i64),
        (Value::Str(s), "to_bytes") => Value::Bytes(s.clone().into_bytes()),
        (Value::Str(s), "split") => {
            let Value::Str(sep) = args.remove(0) else {
                unreachable!("checked argument");
            };
            if sep.is_empty() {
                return Err(fail(span, "`split` needs a non-empty separator"));
            }
            Value::List(s.split(&sep).map(|p| Value::Str(p.to_string())).collect())
        }
        (Value::Str(s), "lines") => Value::List(s.lines().map(|l| Value::Str(l.to_string())).collect()),
        (Value::Str(s), "trim") => Value::Str(s.trim().to_string()),
        (Value::Str(s), "contains" | "starts_with") => {
            let Value::Str(arg) = args.remove(0) else {
                unreachable!("checked argument");
            };
            Value::Bool(if name == "contains" {
                s.contains(&arg)
            } else {
                s.starts_with(&arg)
            })
        }
        (Value::Str(s), "parse_int") => match s.parse::<i64>() {
            Ok(n) => constructor("Some", vec![Value::Int(n)]),
            Err(_) => constructor("None", vec![]),
        },
        (Value::List(v), "len") => Value::Int(v.len() as i64),
        (Value::Map(m), "len") => Value::Int(m.len() as i64),
        (Value::Map(m), "keys") => Value::List(m.keys().map(Key::to_value).collect()),
        (Value::Map(m), "get") => match m.get(&Key::from_value(args.remove(0))) {
            Some(v) => constructor("Some", vec![v.clone()]),
            None => constructor("None", vec![]),
        },
        (Value::Bytes(b), "len") => Value::Int(b.len() as i64),
        (Value::Bytes(b), "to_str") => match String::from_utf8(b.clone()) {
            Ok(s) => Value::Str(s),
            Err(_) => return Err(fail(span, "invalid UTF-8")),
        },
        (Value::Enum(en, i, payload), "unwrap_or") if en == "Option" => {
            if builtin("Some") == Some((en.as_str(), *i)) {
                payload[0].clone()
            } else {
                args.remove(0)
            }
        }
        _ => unreachable!("checked method"),
    })
}

fn constructor(name: &str, payload: Vec<Value>) -> Value {
    let (en, i) = builtin(name).expect("checked constructor");
    Value::Enum(en.to_string(), i, payload)
}

/// True when evaluating `e` cannot assign to a local, since only statements do and only
/// blocks hold them. Calls cannot: a callee has its own locals.
fn pure(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Unit => true,
        ExprKind::Name(_) | ExprKind::Path(..) | ExprKind::Module(..) => true,
        ExprKind::Block(_) | ExprKind::If(..) | ExprKind::Match(..) => false,
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) => pure(x),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => pure(a) && pure(b),
        ExprKind::Call(f, xs) | ExprKind::Method(f, _, xs) => pure(f) && xs.iter().all(pure),
        ExprKind::List(xs) => xs.iter().all(pure),
        ExprKind::StructLit(_, inits) => inits.iter().all(|(_, x)| pure(x)),
    }
}

fn binary(op: &str, l: Value, r: Value, span: Span) -> Res {
    let int = |n: Option<i64>| n.map(Value::Int).ok_or_else(|| fail(span, "Int overflow"));
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => match op {
            "+" => int(a.checked_add(b)),
            "-" => int(a.checked_sub(b)),
            "*" => int(a.checked_mul(b)),
            "/" | "%" if b == 0 => Err(fail(span, "division by zero")),
            "/" => int(a.checked_div(b)),
            "%" => int(a.checked_rem(b)),
            "<" => Ok(Value::Bool(a < b)),
            "<=" => Ok(Value::Bool(a <= b)),
            ">" => Ok(Value::Bool(a > b)),
            ">=" => Ok(Value::Bool(a >= b)),
            "==" => Ok(Value::Bool(a == b)),
            _ => Ok(Value::Bool(a != b)),
        },
        (Value::Str(a), Value::Str(b)) if op == "+" => Ok(Value::Str(a + &b)),
        (a, b) => Ok(Value::Bool((a == b) == (op == "=="))),
    }
}
