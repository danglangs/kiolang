use crate::check::Program;
use crate::ir::*;
use crate::Span;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::rc::Rc;

/// Bounds Rust recursion, and so stack use: a call nested deeper stops the run with "evaluation
/// is too deep". Measured (P3c): a release guest on the default 2 MB `--stack` holds 1_141 calls
/// recursing through loops, the costliest common shape, and 3_271 plain ones. Deep nesting
/// within each call can still exhaust the stack first, which traps as "stack overflow".
const MAX_CALLS: usize = 1_000;

/// What the host grants and does for `main`'s capability parameters.
pub trait Caps {
    /// Whether the host granted a capability of type `ty` for parameter `name`.
    fn has(&self, name: &str, ty: &str) -> bool;
    /// Runs `method` on the capability granted as `cap`. `Ok` must have the method's declared
    /// type; `Err` is a host failure that stops the run.
    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String>;
}

/// `==` is structural, and IEEE on floats (`NaN != NaN`), as Rust's derived `PartialEq`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    Bytes(Rc<Vec<u8>>),
    Unit,
    List(Rc<Vec<Value>>),
    /// Ordered by key, so `keys()` and iteration are deterministic.
    Map(Rc<BTreeMap<Key, Value>>),
    /// The struct and its field values in declaration order.
    Struct(StructId, Vec<Value>),
    /// The enum, the variant's index in declaration order and its payload.
    Enum(EnumRef, usize, Vec<Value>),
    /// A capability, named by the `main` parameter it came from.
    Cap(String),
    /// A function used as a value.
    Fn(FuncId),
    /// A closure: its lifted function and the values it captured, which that function takes
    /// before its own arguments.
    Closure(FuncId, Rc<[Value]>),
}

// Lists, structs and enums hold values inline, so keep `Value` small: 40 bytes on 64-bit targets.
const _: () = assert!(std::mem::size_of::<Value>() <= 40);

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
            Value::Str(s) => Key::Str(s.to_string()),
            _ => unreachable!("checked key"),
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        match self {
            Key::Int(n) => Value::Int(*n),
            Key::Str(s) => Value::Str(s.as_str().into()),
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

fn out_of_bounds(span: Span, i: i64, len: usize) -> Box<Stop> {
    fail(span, &format!("index {i} out of bounds for length {len}"))
}

/// Evaluates `main`. Calls nest at most `MAX_CALLS` deep, which natively needs up to about 2 MB
/// of stack in release builds and 9 MB in debug ones, more with deep nesting in each call.
/// Callers provide that stack.
pub fn run(p: &Program, caps: &mut dyn Caps) -> Result<Value, RunErr> {
    let m = &p.ir;
    let main = &m.funcs[m.main.0 as usize];
    let mut stack = Vec::new();
    for param in &main.locals[..main.params as usize] {
        let Ty::Cap(c) = param.ty else {
            unreachable!("checked parameter");
        };
        let ty = &m.caps[c.0 as usize].name;
        if !caps.has(&param.name, ty) {
            return Err(RunErr {
                span: param.span,
                msg: format!("no grant for `{}: {ty}`", param.name),
            });
        }
        stack.push(Value::Cap(param.name.clone()));
    }
    stack.resize(main.locals.len(), Value::Unit);
    let mut it = Interp {
        m,
        caps,
        stack,
        base: 0,
        calls: 0,
        path: Vec::new(),
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
    m: &'a Module,
    caps: &'a mut dyn Caps,
    /// The locals of every running call, innermost last, each frame indexed by `LocalId`.
    stack: Vec<Value>,
    /// Where the running call's frame starts in `stack`.
    base: usize,
    /// Calls in progress, not counting `main`.
    calls: usize,
    /// The evaluated steps of places being read or written, innermost last: a field's index
    /// or an index's value.
    path: Vec<i64>,
}

impl<'a> Interp<'a> {
    fn local(&mut self, id: LocalId) -> &mut Value {
        &mut self.stack[self.base + id.0 as usize]
    }

    /// Runs `b`, then drops its locals, as a block's end does in Rust, so that a list they
    /// shared is not copied on its next change.
    fn block(&mut self, b: &'a Block) -> Res {
        let r = self.block_value(b);
        for s in &b.stmts {
            if let StmtKind::Let(id, _) = s.kind {
                *self.local(id) = Value::Unit;
            }
        }
        r
    }

    fn block_value(&mut self, b: &'a Block) -> Res {
        for s in &b.stmts {
            self.stmt(s)?;
        }
        match &b.tail {
            Some(t) => self.expr(t),
            None => Ok(Value::Unit),
        }
    }

    #[inline(never)]
    fn stmt(&mut self, s: &'a Stmt) -> Result<(), Box<Stop>> {
        match &s.kind {
            StmtKind::Let(id, init) => self.let_stmt(*id, init),
            StmtKind::Assign(place, op, value) => self.assign(place, *op, value, s.span),
            StmtKind::Return(value) => self.return_stmt(value.as_ref()),
            StmtKind::While(cond, body) => self.while_stmt(cond, body),
            StmtKind::Loop(body) => {
                while self.iteration(body)? {}
                Ok(())
            }
            StmtKind::ForList(x, xs, body) => self.for_list(*x, xs, body),
            StmtKind::ForRange(i, a, b, body) => self.for_range(*i, a, b, body),
            StmtKind::Break => Err(Box::new(Stop::Break)),
            StmtKind::Continue => Err(Box::new(Stop::Continue)),
            StmtKind::Expr(e) => self.expr(e).map(drop),
        }
    }

    fn let_stmt(&mut self, id: LocalId, init: &'a Expr) -> Result<(), Box<Stop>> {
        let v = self.expr(init)?;
        *self.local(id) = v;
        Ok(())
    }

    fn return_stmt(&mut self, value: Option<&'a Expr>) -> Result<(), Box<Stop>> {
        let v = match value {
            Some(e) => self.expr(e)?,
            None => Value::Unit,
        };
        Err(Box::new(Stop::Return(v)))
    }

    #[inline(never)]
    fn while_stmt(&mut self, cond: &'a Expr, body: &'a Block) -> Result<(), Box<Stop>> {
        while self.cond(cond)? {
            if !self.iteration(body)? {
                break;
            }
        }
        Ok(())
    }

    #[inline(never)]
    fn for_range(&mut self, i: LocalId, a: &'a Expr, b: &'a Expr, body: &'a Block) -> Result<(), Box<Stop>> {
        let (a, b) = (self.int(a)?, self.int(b)?);
        for n in a..b {
            *self.local(i) = Value::Int(n);
            if !self.iteration(body)? {
                break;
            }
        }
        Ok(())
    }

    /// Runs `x` over `xs` as it was when the loop started.
    #[inline(never)]
    fn for_list(&mut self, x: LocalId, xs: &'a Expr, body: &'a Block) -> Result<(), Box<Stop>> {
        let Value::List(vals) = self.expr(xs)? else {
            unreachable!("checked list");
        };
        let mut r = Ok(());
        for v in vals.iter() {
            *self.local(x) = v.clone();
            match self.iteration(body) {
                Ok(true) => {}
                Ok(false) => break,
                Err(s) => {
                    r = Err(s);
                    break;
                }
            }
        }
        *self.local(x) = Value::Unit;
        r
    }

    /// Runs one pass of a loop body. False when it ends the loop with `break`.
    fn iteration(&mut self, body: &'a Block) -> Result<bool, Box<Stop>> {
        match self.block(body) {
            Ok(_) => Ok(true),
            Err(s) => match *s {
                Stop::Break => Ok(false),
                Stop::Continue => Ok(true),
                e => Err(Box::new(e)),
            },
        }
    }

    /// Evaluates the indexes of `place` in order, then `value`, then writes it, so a bounds error
    /// on the way comes after both.
    #[inline(never)]
    fn assign(&mut self, place: &'a Place, op: Option<BinOp>, value: &'a Expr, span: Span) -> Result<(), Box<Stop>> {
        let start = self.path.len();
        let r = self.assign_at(start, place, op, value, span);
        self.path.truncate(start);
        r
    }

    fn assign_at(
        &mut self,
        start: usize,
        place: &'a Place,
        op: Option<BinOp>,
        value: &'a Expr,
        span: Span,
    ) -> Result<(), Box<Stop>> {
        self.place_indexes(place)?;
        let mut v = self.expr(value)?;
        let slot = self.slot(place, start)?;
        if let Some(op) = op {
            v = binary(op, slot.clone(), v, span)?;
        }
        *slot = v;
        Ok(())
    }

    /// Pushes the value of each step of `place` to `path`, evaluating indexes in order.
    fn place_indexes(&mut self, place: &'a Place) -> Result<(), Box<Stop>> {
        for step in &place.steps {
            let n = match step {
                PlaceStep::Field(f) => i64::from(*f),
                PlaceStep::Index(i, _) => self.int(i)?,
            };
            self.path.push(n);
        }
        Ok(())
    }

    /// The part of the local that `place` names, with its steps from `path[start..]`, to write:
    /// a shared list on the way is copied first.
    fn slot(&mut self, place: &Place, start: usize) -> Result<&mut Value, Box<Stop>> {
        let mut slot = &mut self.stack[self.base + place.root.0 as usize];
        for (step, &n) in place.steps.iter().zip(&self.path[start..]) {
            slot = match (step, slot) {
                (PlaceStep::Field(_), Value::Struct(_, vals)) => &mut vals[n as usize],
                (PlaceStep::Index(_, span), Value::List(vals)) => {
                    let vals = Rc::make_mut(vals);
                    let len = vals.len();
                    match usize::try_from(n).ok().and_then(|i| vals.get_mut(i)) {
                        Some(v) => v,
                        None => return Err(out_of_bounds(*span, n, len)),
                    }
                }
                _ => unreachable!("checked place"),
            };
        }
        Ok(slot)
    }

    /// `push`, `insert`, `remove`, `pop` and `sort`: the place's indexes, then the arguments, then the change.
    #[inline(never)]
    fn mutate(&mut self, place: &'a Place, op: Mut, args: &'a [Expr], span: Span) -> Res {
        let start = self.path.len();
        let r = self.mutate_at(start, place, op, args, span);
        self.path.truncate(start);
        r
    }

    fn mutate_at(&mut self, start: usize, place: &'a Place, op: Mut, args: &'a [Expr], span: Span) -> Res {
        self.place_indexes(place)?;
        let a = match args.first() {
            Some(e) => self.expr(e)?,
            None => Value::Unit,
        };
        let b = match args.get(1) {
            Some(e) => Some(self.expr(e)?),
            None => None,
        };
        match (self.slot(place, start)?, op, a, b) {
            (Value::List(vals), Mut::ListPush, item, _) => Rc::make_mut(vals).push(item),
            (Value::List(vals), Mut::ListSort, ..) => Rc::make_mut(vals).sort_by(|a, b| match (a, b) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Str(a), Value::Str(b)) => a.cmp(b),
                _ => unreachable!("checked sort"),
            }),
            (Value::List(vals), Mut::ListPop, ..) => {
                return Ok(match Rc::make_mut(vals).pop() {
                    Some(v) => some(v),
                    None => none(),
                })
            }
            (Value::Bytes(b), Mut::BytesPush, Value::Int(n), _) => match u8::try_from(n) {
                Ok(n) => Rc::make_mut(b).push(n),
                Err(_) => return Err(fail(span, &format!("byte {n} out of range 0..=255"))),
            },
            (Value::Map(map), Mut::MapInsert, k, Some(v)) => {
                Rc::make_mut(map).insert(Key::from_value(k), v);
            }
            (Value::Map(map), Mut::MapRemove, k, _) => {
                Rc::make_mut(map).remove(&Key::from_value(k));
            }
            _ => unreachable!("checked mutation"),
        }
        Ok(Value::Unit)
    }

    fn cond(&mut self, e: &'a Expr) -> Result<bool, Box<Stop>> {
        match self.expr(e)? {
            Value::Bool(b) => Ok(b),
            _ => unreachable!("checked Bool"),
        }
    }

    fn int(&mut self, e: &'a Expr) -> Result<i64, Box<Stop>> {
        match self.expr(e)? {
            Value::Int(n) => Ok(n),
            _ => unreachable!("checked Int"),
        }
    }

    fn expr(&mut self, e: &'a Expr) -> Res {
        match &e.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(x) => Ok(Value::Float(*x)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Unit => Ok(Value::Unit),
            ExprKind::Local(id) => Ok(self.local(*id).clone()),
            ExprKind::Unary(op, x) => self.unary(*op, x, e.span),
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, e.span),
            ExprKind::If(cond, then, els) => self.if_expr(cond, then, els.as_deref()),
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(f, args) => self.call(*f, &[], args, e.span),
            ExprKind::FnRef(f) => Ok(Value::Fn(*f)),
            ExprKind::CallValue(f, args) => match self.expr(f)? {
                Value::Fn(f) => self.call(f, &[], args, e.span),
                Value::Closure(f, captured) => self.call(f, &captured, args, e.span),
                _ => unreachable!("checked function value"),
            },
            ExprKind::Closure(f, captured) => {
                let vals = captured.iter().map(|x| self.local(*x).clone()).collect();
                Ok(Value::Closure(*f, vals))
            }
            ExprKind::CapCall(cap, method, args) => self.cap_call(*cap, *method, args, e.span),
            ExprKind::Builtin(b, recv, args) => self.builtin(*b, recv, args, e.span),
            ExprKind::Mutate(place, op, args) => self.mutate(place, *op, args, e.span),
            ExprKind::List(items) => self.list(items),
            ExprKind::MapNew => Ok(Value::Map(Rc::default())),
            ExprKind::StructLit(s, inits) => self.struct_lit(*s, inits),
            ExprKind::Field(..) | ExprKind::Index(..) => self.read(e),
            ExprKind::Variant(en, i, args) => self.variant(*en, *i, args),
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms),
            ExprKind::Try(x) => self.try_expr(x),
        }
    }

    fn unary(&mut self, op: UnOp, x: &'a Expr, span: Span) -> Res {
        let v = self.expr(x)?;
        unary(op, v, span)
    }

    /// `&&` and `||` skip the right side when the left decides the result.
    fn binary(&mut self, op: BinOp, a: &'a Expr, b: &'a Expr, span: Span) -> Res {
        if let BinOp::And | BinOp::Or = op {
            let l = self.cond(a)?;
            return if l == (op == BinOp::Or) {
                Ok(Value::Bool(l))
            } else {
                self.expr(b)
            };
        }
        let l = self.expr(a)?;
        let r = self.expr(b)?;
        binary(op, l, r, span)
    }

    fn if_expr(&mut self, cond: &'a Expr, then: &'a Block, els: Option<&'a Expr>) -> Res {
        if self.cond(cond)? {
            self.block(then)
        } else {
            match els {
                Some(x) => self.expr(x),
                None => Ok(Value::Unit),
            }
        }
    }

    #[inline(never)]
    fn list(&mut self, items: &'a [Expr]) -> Res {
        Ok(Value::List(self.values(items)?.into()))
    }

    #[inline(never)]
    fn variant(&mut self, en: EnumRef, i: u32, args: &'a [Expr]) -> Res {
        Ok(Value::Enum(en, i as usize, self.values(args)?))
    }

    fn values(&mut self, exprs: &'a [Expr]) -> Result<Vec<Value>, Box<Stop>> {
        exprs.iter().map(|x| self.expr(x)).collect()
    }

    /// Evaluates the arguments into the callee's frame, above the caller's, after a closure's
    /// `captured` values.
    #[inline(never)]
    fn call(&mut self, f: FuncId, captured: &[Value], args: &'a [Expr], span: Span) -> Res {
        if self.calls == MAX_CALLS {
            return Err(fail(span, "evaluation is too deep"));
        }
        let base = self.stack.len();
        self.stack.extend_from_slice(captured);
        for a in args {
            match self.expr(a) {
                Ok(v) => self.stack.push(v),
                Err(s) => {
                    self.stack.truncate(base);
                    return Err(s);
                }
            }
        }
        self.enter(f, base)
    }

    /// Calls the function value `f` on one argument, for the built-ins that take a function.
    fn apply(&mut self, f: &Value, arg: Value, span: Span) -> Res {
        if self.calls == MAX_CALLS {
            return Err(fail(span, "evaluation is too deep"));
        }
        let base = self.stack.len();
        let f = match f {
            Value::Fn(f) => *f,
            Value::Closure(f, captured) => {
                self.stack.extend_from_slice(captured);
                *f
            }
            _ => unreachable!("checked function value"),
        };
        self.stack.push(arg);
        self.enter(f, base)
    }

    /// Runs `f`, whose arguments are on the stack from `base`.
    fn enter(&mut self, f: FuncId, base: usize) -> Res {
        let func = &self.m.funcs[f.0 as usize];
        self.stack.resize(base + func.locals.len(), Value::Unit);
        let caller = std::mem::replace(&mut self.base, base);
        self.calls += 1;
        let r = self.block(&func.body);
        self.calls -= 1;
        self.base = caller;
        self.stack.truncate(base);
        returned(r)
    }

    #[inline(never)]
    fn cap_call(&mut self, cap: LocalId, method: u32, args: &'a [Expr], span: Span) -> Res {
        let Value::Cap(name) = self.local(cap).clone() else {
            unreachable!("checked capability");
        };
        // A capability is always one of `main`'s parameters, perhaps passed down.
        let main = &self.m.funcs[self.m.main.0 as usize];
        let param = main.locals[..main.params as usize].iter().find(|l| l.name == name);
        let Some(Ty::Cap(c)) = param.map(|l| &l.ty) else {
            unreachable!("Value::Cap names a main parameter");
        };
        let (method, _, ty) = &self.m.caps[c.0 as usize].methods[method as usize];
        let vals = self.values(args)?;
        let v = self.caps.call(&name, method, vals).map_err(|msg| fail(span, &msg))?;
        // The host is trusted code but checked anyway: a value of the wrong type stops the run.
        if !self.fits(&v, ty) {
            return Err(fail(span, &format!("host returned a malformed value for `{method}`")));
        }
        Ok(v)
    }

    /// Whether `v` is a value of type `ty`; used to refuse malformed results from the host.
    fn fits(&self, v: &Value, ty: &Ty) -> bool {
        match (v, ty) {
            (Value::Int(_), Ty::Int) | (Value::Float(_), Ty::Float) => true,
            (Value::Bool(_), Ty::Bool) | (Value::Unit, Ty::Unit) => true,
            (Value::Str(_), Ty::Str) | (Value::Bytes(_), Ty::Bytes) => true,
            (Value::List(vals), Ty::List(t)) => vals.iter().all(|v| self.fits(v, t)),
            (Value::Map(m), Ty::Map(k, t)) => m
                .iter()
                .all(|(key, v)| self.fits(&key.to_value(), k) && self.fits(v, t)),
            (Value::Enum(EnumRef::Result, i, vals), Ty::Result(ok, e)) if vals.len() == 1 => match i {
                0 => self.fits(&vals[0], ok),
                1 => self.fits(&vals[0], e),
                _ => false,
            },
            (Value::Enum(EnumRef::Option, i, vals), Ty::Option(t)) => match (i, vals.as_slice()) {
                (0, []) => true,
                (1, [v]) => self.fits(v, t),
                _ => false,
            },
            (Value::Enum(EnumRef::User(en), i, vals), Ty::Enum(id)) if en == id => {
                let Some((_, payload)) = self.m.enums[id.0 as usize].variants.get(*i) else {
                    return false;
                };
                vals.len() == payload.len() && vals.iter().zip(payload).all(|(v, t)| self.fits(v, t))
            }
            _ => false,
        }
    }

    /// A built-in method. A receiver that is part of a local is read in place, unless the
    /// arguments could change the local, which a copy taken first would not see.
    #[inline(never)]
    fn builtin(&mut self, b: Builtin, recv: &'a Expr, args: &'a [Expr], span: Span) -> Res {
        if takes_fn(b) {
            let recv = self.expr(recv)?;
            let f = self.expr(&args[0])?;
            return self.with_fn(b, recv, &f, span);
        }
        if b == Builtin::StrParseJson {
            let Value::Str(s) = self.expr(recv)? else {
                unreachable!("checked Str");
            };
            return Ok(match crate::json::parse(&s, self.m.json()) {
                Ok(j) => ok(j),
                Err(e) => Value::Enum(EnumRef::Result, 1, vec![Value::Str(e.into())]),
            });
        }
        if in_place(recv) && args.iter().all(pure) {
            let start = self.path.len();
            let r = self.builtin_in_place(start, b, recv, args, span);
            self.path.truncate(start);
            return r;
        }
        let recv = self.expr(recv)?;
        let args = self.arg(args)?;
        builtin(&recv, b, args, span)
    }

    fn builtin_in_place(&mut self, start: usize, b: Builtin, recv: &'a Expr, args: &'a [Expr], span: Span) -> Res {
        let root = self.steps(recv, start)?;
        let args = self.arg(args)?;
        builtin(&self.view(root, &self.path[start..]), b, args, span)
    }

    /// Built-in method `b`, which takes the function value `f`, on `recv`.
    #[inline(never)]
    fn with_fn(&mut self, b: Builtin, recv: Value, f: &Value, span: Span) -> Res {
        use Builtin as B;
        let truth = |v: Value| match v {
            Value::Bool(b) => b,
            _ => unreachable!("checked Bool"),
        };
        Ok(match (b, recv) {
            // `None` is variant 0 of `Option`, `Ok` variant 0 of `Result`.
            (B::OptionMap | B::OptionAndThen, Value::Enum(_, 0, _)) => none(),
            (B::OptionMap, Value::Enum(_, _, mut p)) => some(self.apply(f, p.remove(0), span)?),
            (B::OptionAndThen, Value::Enum(_, _, mut p)) => self.apply(f, p.remove(0), span)?,
            (B::ResultMap, Value::Enum(_, 0, mut p)) => ok(self.apply(f, p.remove(0), span)?),
            (B::ResultMapErr, Value::Enum(_, 1, mut p)) => {
                Value::Enum(EnumRef::Result, 1, vec![self.apply(f, p.remove(0), span)?])
            }
            (B::ResultAndThen, Value::Enum(_, 0, mut p)) => self.apply(f, p.remove(0), span)?,
            (B::ResultMap | B::ResultMapErr | B::ResultAndThen, r @ Value::Enum(..)) => r,
            (B::ListMap, Value::List(xs)) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs.iter() {
                    out.push(self.apply(f, x.clone(), span)?);
                }
                Value::List(Rc::new(out))
            }
            (B::ListFilter, Value::List(xs)) => {
                let mut out = Vec::new();
                for x in xs.iter() {
                    if truth(self.apply(f, x.clone(), span)?) {
                        out.push(x.clone());
                    }
                }
                Value::List(Rc::new(out))
            }
            (B::ListAny | B::ListAll, Value::List(xs)) => {
                // `any` stops at the first true, `all` at the first false, as Rust's.
                let stop = b == B::ListAny;
                for x in xs.iter() {
                    if truth(self.apply(f, x.clone(), span)?) == stop {
                        return Ok(Value::Bool(stop));
                    }
                }
                Value::Bool(!stop)
            }
            _ => unreachable!("checked receiver of {b:?}"),
        })
    }

    /// The arguments a built-in method may take: at most two.
    fn arg(&mut self, args: &'a [Expr]) -> Result<Args, Box<Stop>> {
        let first = args.first().map(|a| self.expr(a)).transpose()?;
        let second = args.get(1).map(|a| self.expr(a)).transpose()?;
        Ok((first, second))
    }

    /// A field or element: read in place when `e` is part of a local, else from its value.
    #[inline(never)]
    fn read(&mut self, e: &'a Expr) -> Res {
        if in_place(e) {
            let start = self.path.len();
            let r = self
                .steps(e, start)
                .map(|root| self.view(root, &self.path[start..]).into_owned());
            self.path.truncate(start);
            return r;
        }
        match &e.kind {
            ExprKind::Field(base, f) => {
                let Value::Struct(_, mut vals) = self.expr(base)? else {
                    unreachable!("checked field");
                };
                Ok(vals.swap_remove(*f as usize))
            }
            ExprKind::Index(base, index) => {
                let base = self.expr(base)?;
                let i = self.int(index)?;
                let len = match &base {
                    Value::List(vals) => vals.len(),
                    Value::Bytes(b) => b.len(),
                    _ => unreachable!("checked index"),
                };
                let Some(at) = usize::try_from(i).ok().filter(|&i| i < len) else {
                    return Err(out_of_bounds(e.span, i, len));
                };
                Ok(match base {
                    Value::List(vals) => vals[at].clone(),
                    Value::Bytes(b) => Value::Int(i64::from(b[at])),
                    _ => unreachable!("checked index"),
                })
            }
            _ => unreachable!("a field or index"),
        }
    }

    /// Pushes the steps from the root local of `e`, which `in_place` accepts, to `path`,
    /// evaluating and bounds-checking each index in order. Returns the root.
    fn steps(&mut self, e: &'a Expr, start: usize) -> Result<LocalId, Box<Stop>> {
        match &e.kind {
            ExprKind::Local(id) => Ok(*id),
            ExprKind::Field(base, f) => {
                let root = self.steps(base, start)?;
                self.path.push(i64::from(*f));
                Ok(root)
            }
            ExprKind::Index(base, index) => {
                let root = self.steps(base, start)?;
                let i = self.int(index)?;
                let len = match &*self.view(root, &self.path[start..]) {
                    Value::List(vals) => vals.len(),
                    Value::Bytes(b) => b.len(),
                    _ => unreachable!("checked index"),
                };
                if usize::try_from(i).map_or(true, |i| i >= len) {
                    return Err(out_of_bounds(e.span, i, len));
                }
                self.path.push(i);
                Ok(root)
            }
            _ => unreachable!("checked place"),
        }
    }

    /// The part of local `root` that `steps`, already bounds-checked, lead to. A byte is not a
    /// `Value`, so an index into `Bytes` gives a new `Int`.
    fn view(&self, root: LocalId, steps: &[i64]) -> Cow<'_, Value> {
        let mut v = &self.stack[self.base + root.0 as usize];
        for &n in steps {
            v = match v {
                Value::Struct(_, vals) => &vals[n as usize],
                Value::List(vals) => &vals[n as usize],
                Value::Bytes(b) => return Cow::Owned(Value::Int(i64::from(b[n as usize]))),
                _ => unreachable!("checked place"),
            };
        }
        Cow::Borrowed(v)
    }

    /// Fields are evaluated in source order and stored in declaration order.
    #[inline(never)]
    fn struct_lit(&mut self, s: StructId, inits: &'a [(u32, Expr)]) -> Res {
        let mut vals = vec![Value::Unit; inits.len()];
        for (f, x) in inits {
            vals[*f as usize] = self.expr(x)?;
        }
        Ok(Value::Struct(s, vals))
    }

    /// `Some` and `Ok` yield their payload; `None` and `Err` return from the function.
    #[inline(never)]
    fn try_expr(&mut self, x: &'a Expr) -> Res {
        let Value::Enum(en, i, mut payload) = self.expr(x)? else {
            unreachable!("checked operand");
        };
        if i == success(en) {
            return Ok(payload.remove(0));
        }
        Err(Box::new(Stop::Return(Value::Enum(en, i, payload))))
    }

    /// The first arm whose pattern matches runs, with its bindings in their slots.
    #[inline(never)]
    fn match_expr(&mut self, scrut: &'a Expr, arms: &'a [Arm]) -> Res {
        let arm = self.pick(scrut, arms)?;
        let r = self.expr(&arm.body);
        // Dropped as at a block's end.
        self.unbind(&arm.pat);
        r
    }

    /// Moves the parts of `v`, which matches `p`, into `p`'s slots, left to right.
    fn bind(&mut self, p: &Pat, v: Value) {
        match (p, v) {
            (Pat::Bind(id), v) => *self.local(*id) = v,
            (Pat::Variant(_, ps), Value::Enum(_, _, vs)) => {
                for (p, v) in ps.iter().zip(vs) {
                    self.bind(p, v);
                }
            }
            _ => {}
        }
    }

    fn unbind(&mut self, p: &Pat) {
        match p {
            Pat::Bind(id) => *self.local(*id) = Value::Unit,
            Pat::Variant(_, ps) => ps.iter().for_each(|p| self.unbind(p)),
            _ => {}
        }
    }

    /// The arm that matches `scrut`'s value, with that value bound to the arm's slots.
    #[inline(never)]
    fn pick(&mut self, scrut: &'a Expr, arms: &'a [Arm]) -> Result<&'a Arm, Box<Stop>> {
        let v = self.expr(scrut)?;
        let arm = arms.iter().find(|a| matches(&a.pat, &v)).expect("checked exhaustive");
        self.bind(&arm.pat, v);
        Ok(arm)
    }
}

/// Whether `v` matches `p`.
fn matches(p: &Pat, v: &Value) -> bool {
    match (p, v) {
        (Pat::Wild | Pat::Bind(_), _) => true,
        (Pat::Int(a), Value::Int(b)) => a == b,
        (Pat::Str(a), Value::Str(b)) => **a == **b,
        (Pat::Bool(a), Value::Bool(b)) => a == b,
        (Pat::Variant(i, ps), Value::Enum(_, j, vs)) => {
            *i as usize == *j && ps.iter().zip(vs).all(|(p, v)| matches(p, v))
        }
        _ => unreachable!("checked pattern"),
    }
}

/// The variant of `Option` or `Result` that `?` and `unwrap_or` unwrap: `Some` or `Ok`.
fn success(en: EnumRef) -> usize {
    match en {
        EnumRef::Option => 1,
        _ => 0,
    }
}

/// Whether `e` is a local, or a part of one reached through fields and indexes that cannot
/// change it, so it can be read without a copy.
fn in_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) => true,
        ExprKind::Field(base, _) => in_place(base),
        ExprKind::Index(base, index) => in_place(base) && pure(index),
        _ => false,
    }
}

/// True when evaluating `e` cannot change a local: only statements, which only blocks hold,
/// and mutating methods do. Calls cannot: a callee has its own locals.
fn pure(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Unit => true,
        ExprKind::Local(_) | ExprKind::MapNew | ExprKind::FnRef(_) | ExprKind::Closure(..) => true,
        ExprKind::CallValue(f, xs) => pure(f) && xs.iter().all(pure),
        ExprKind::Block(_) | ExprKind::If(..) | ExprKind::Match(..) | ExprKind::Mutate(..) => false,
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Try(x) => pure(x),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => pure(a) && pure(b),
        ExprKind::Builtin(_, x, xs) => pure(x) && xs.iter().all(pure),
        ExprKind::Call(_, xs) | ExprKind::CapCall(_, _, xs) | ExprKind::List(xs) | ExprKind::Variant(_, _, xs) => {
            xs.iter().all(pure)
        }
        ExprKind::StructLit(_, inits) => inits.iter().all(|(_, x)| pure(x)),
    }
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

type Args = (Option<Value>, Option<Value>);

fn some(v: Value) -> Value {
    Value::Enum(EnumRef::Option, 1, vec![v])
}

fn ok(v: Value) -> Value {
    Value::Enum(EnumRef::Result, 0, vec![v])
}

/// The built-ins that take a function value, which `Eval::with_fn` runs.
fn takes_fn(b: Builtin) -> bool {
    use Builtin as B;
    matches!(
        b,
        B::OptionMap
            | B::OptionAndThen
            | B::ResultMap
            | B::ResultMapErr
            | B::ResultAndThen
            | B::ListMap
            | B::ListFilter
            | B::ListAny
            | B::ListAll
    )
}

fn none() -> Value {
    Value::Enum(EnumRef::Option, 0, Vec::new())
}

/// The checked offsets of a `slice` call as a range within `len`, or the out-of-bounds error.
fn slice_range(a: Option<Value>, b: Option<Value>, len: usize, span: Span) -> Result<(usize, usize), Box<Stop>> {
    let (Some(Value::Int(a)), Some(Value::Int(b))) = (a, b) else {
        unreachable!("checked arguments");
    };
    match (usize::try_from(a), usize::try_from(b)) {
        (Ok(x), Ok(y)) if x <= y && y <= len => Ok((x, y)),
        _ => Err(fail(span, &format!("slice {a}..{b} out of bounds for length {len}"))),
    }
}

fn str_arg(arg: Option<Value>) -> Rc<str> {
    match arg {
        Some(Value::Str(s)) => s,
        _ => unreachable!("checked argument"),
    }
}

/// Built-in method `b` on `recv`, which may be part of a local, with its argument if it takes one.
fn builtin(recv: &Value, b: Builtin, (arg, arg2): Args, span: Span) -> Res {
    use Builtin as B;
    Ok(match (b, recv) {
        (B::IntToStr, Value::Int(n)) => Value::Str(n.to_string().into()),
        (B::StrLen, Value::Str(s)) => Value::Int(s.len() as i64),
        (B::StrToBytes, Value::Str(s)) => Value::Bytes(Rc::new(s.as_bytes().to_vec())),
        (B::StrSplit, Value::Str(s)) => {
            let sep = str_arg(arg);
            if sep.is_empty() {
                return Err(fail(span, "`split` needs a non-empty separator"));
            }
            Value::List(s.split(&*sep).map(|p| Value::Str(p.into())).collect::<Vec<_>>().into())
        }
        (B::StrLines, Value::Str(s)) => Value::List(s.lines().map(|l| Value::Str(l.into())).collect::<Vec<_>>().into()),
        (B::StrTrim, Value::Str(s)) => Value::Str(s.trim().into()),
        (B::StrContains, Value::Str(s)) => Value::Bool(s.contains(&*str_arg(arg))),
        (B::StrStartsWith, Value::Str(s)) => Value::Bool(s.starts_with(&*str_arg(arg))),
        (B::StrEndsWith, Value::Str(s)) => Value::Bool(s.ends_with(&*str_arg(arg))),
        (B::StrStripPrefix, Value::Str(s)) => match s.strip_prefix(&*str_arg(arg)) {
            Some(r) => some(Value::Str(r.into())),
            None => none(),
        },
        (B::StrStripSuffix, Value::Str(s)) => match s.strip_suffix(&*str_arg(arg)) {
            Some(r) => some(Value::Str(r.into())),
            None => none(),
        },
        (B::StrFind, Value::Str(s)) => match s.find(&*str_arg(arg)) {
            Some(i) => some(Value::Int(i as i64)),
            None => none(),
        },
        (B::StrReplace, Value::Str(s)) => Value::Str(s.replace(&*str_arg(arg), &str_arg(arg2)).into()),
        (B::ListContains, Value::List(v)) => Value::Bool(v.contains(&arg.expect("checked argument"))),
        (B::StrParseInt, Value::Str(s)) => match s.parse::<i64>() {
            Ok(n) => some(Value::Int(n)),
            Err(_) => none(),
        },
        (B::BytesLen, Value::Bytes(b)) => Value::Int(b.len() as i64),
        (B::BytesToStr, Value::Bytes(b)) => match std::str::from_utf8(b) {
            Ok(s) => Value::Str(s.into()),
            Err(_) => return Err(fail(span, "invalid UTF-8")),
        },
        (B::ListLen, Value::List(v)) => Value::Int(v.len() as i64),
        (B::MapLen, Value::Map(m)) => Value::Int(m.len() as i64),
        (B::MapKeys, Value::Map(m)) => Value::List(m.keys().map(Key::to_value).collect::<Vec<_>>().into()),
        (B::MapGet, Value::Map(m)) => match m.get(&Key::from_value(arg.expect("checked argument"))) {
            Some(v) => some(v.clone()),
            None => none(),
        },
        (B::OptionUnwrapOr | B::ResultUnwrapOr, Value::Enum(en, i, payload)) => {
            if *i == success(*en) {
                payload[0].clone()
            } else {
                arg.expect("checked argument")
            }
        }
        (B::OptionIsSome, Value::Enum(_, i, _)) => Value::Bool(*i == 1),
        (B::OptionIsNone, Value::Enum(_, i, _)) => Value::Bool(*i == 0),
        (B::ResultIsOk, Value::Enum(_, i, _)) => Value::Bool(*i == 0),
        (B::ResultIsErr, Value::Enum(_, i, _)) => Value::Bool(*i == 1),
        (B::ResultOk, Value::Enum(_, i, payload)) => {
            if *i == 0 {
                some(payload[0].clone())
            } else {
                none()
            }
        }
        (B::ResultErr, Value::Enum(_, i, payload)) => {
            if *i == 1 {
                some(payload[0].clone())
            } else {
                none()
            }
        }
        (B::IntToFloat, Value::Int(n)) => Value::Float(*n as f64),
        (B::StrParseFloat, Value::Str(s)) => match s.parse::<f64>() {
            Ok(x) => some(Value::Float(x)),
            Err(_) => none(),
        },
        (B::FloatToStr, Value::Float(x)) => Value::Str(x.to_string().into()),
        (B::FloatToInt, Value::Float(x)) => {
            // `i64::MIN` is exactly -2^63; 2^63 is the first value above `i64::MAX`.
            let t = x.trunc();
            if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&t) {
                some(Value::Int(t as i64))
            } else {
                none()
            }
        }
        (B::FloatAbs, Value::Float(x)) => Value::Float(x.abs()),
        (B::FloatFloor, Value::Float(x)) => Value::Float(x.floor()),
        (B::FloatCeil, Value::Float(x)) => Value::Float(x.ceil()),
        (B::FloatRound, Value::Float(x)) => Value::Float(x.round()),
        (B::FloatSqrt, Value::Float(x)) => Value::Float(x.sqrt()),
        (B::FloatIsNan, Value::Float(x)) => Value::Bool(x.is_nan()),
        (B::FloatIsFinite, Value::Float(x)) => Value::Bool(x.is_finite()),
        (B::FloatIsInfinite, Value::Float(x)) => Value::Bool(x.is_infinite()),
        (B::StrSlice, Value::Str(s)) => {
            let (a, b) = slice_range(arg, arg2, s.len(), span)?;
            match s.get(a..b) {
                Some(p) => Value::Str(p.into()),
                None => return Err(fail(span, &format!("slice {a}..{b} is not on a character boundary"))),
            }
        }
        (B::BytesSlice, Value::Bytes(v)) => {
            let (a, b) = slice_range(arg, arg2, v.len(), span)?;
            Value::Bytes(Rc::new(v[a..b].to_vec()))
        }
        (B::OptionOkOr, Value::Enum(_, i, payload)) => {
            if *i == 1 {
                Value::Enum(EnumRef::Result, 0, vec![payload[0].clone()])
            } else {
                Value::Enum(EnumRef::Result, 1, vec![arg.expect("checked argument")])
            }
        }
        (B::JsonToJson, j) => Value::Str(crate::json::encode(j).into()),
        (B::JsonGet, Value::Enum(_, i, p)) => match (*i, &p[..]) {
            (crate::json::OBJ, [Value::Map(m)]) => match m.get(&Key::Str(str_arg(arg).to_string())) {
                Some(v) => some(v.clone()),
                None => none(),
            },
            _ => none(),
        },
        (B::JsonAt, Value::Enum(_, i, p)) => match (*i, &p[..], arg) {
            (crate::json::ARR, [Value::List(xs)], Some(Value::Int(k))) => {
                match usize::try_from(k).ok().and_then(|k| xs.get(k)) {
                    Some(v) => some(v.clone()),
                    None => none(),
                }
            }
            _ => none(),
        },
        (B::JsonAsFloat, Value::Enum(_, crate::json::NUM, p)) => match p[..] {
            [Value::Int(n)] => some(Value::Float(n as f64)),
            _ => unreachable!("checked Json"),
        },
        (
            B::JsonAsStr | B::JsonAsInt | B::JsonAsFloat | B::JsonAsBool | B::JsonAsList | B::JsonAsMap,
            Value::Enum(_, i, p),
        ) => {
            let want = match b {
                B::JsonAsStr => crate::json::STR,
                B::JsonAsInt => crate::json::NUM,
                B::JsonAsFloat => crate::json::FLOAT,
                B::JsonAsBool => crate::json::BOOL,
                B::JsonAsList => crate::json::ARR,
                _ => crate::json::OBJ,
            };
            match *i == want {
                true => some(p[0].clone()),
                false => none(),
            }
        }
        _ => unreachable!("checked method"),
    })
}

fn unary(op: UnOp, v: Value, span: Span) -> Res {
    match (op, v) {
        (UnOp::Neg, Value::Int(n)) => n
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| fail(span, "Int overflow")),
        (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnOp::FNeg, Value::Float(x)) => Ok(Value::Float(-x)),
        _ => unreachable!("checked operand"),
    }
}

fn binary(op: BinOp, l: Value, r: Value, span: Span) -> Res {
    let int = |n: Option<i64>| n.map(Value::Int).ok_or_else(|| fail(span, "Int overflow"));
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => match op {
            BinOp::Add => int(a.checked_add(b)),
            BinOp::Sub => int(a.checked_sub(b)),
            BinOp::Mul => int(a.checked_mul(b)),
            BinOp::Div | BinOp::Rem if b == 0 => Err(fail(span, "division by zero")),
            BinOp::Div => int(a.checked_div(b)),
            BinOp::Rem => int(a.checked_rem(b)),
            BinOp::Lt => Ok(Value::Bool(a < b)),
            BinOp::Le => Ok(Value::Bool(a <= b)),
            BinOp::Gt => Ok(Value::Bool(a > b)),
            BinOp::Ge => Ok(Value::Bool(a >= b)),
            BinOp::Eq => Ok(Value::Bool(a == b)),
            BinOp::Ne => Ok(Value::Bool(a != b)),
            _ => unreachable!("checked operator"),
        },
        (Value::Float(a), Value::Float(b)) => Ok(match op {
            BinOp::FAdd => Value::Float(a + b),
            BinOp::FSub => Value::Float(a - b),
            BinOp::FMul => Value::Float(a * b),
            BinOp::FDiv => Value::Float(a / b),
            BinOp::FRem => Value::Float(a % b),
            BinOp::FLt => Value::Bool(a < b),
            BinOp::FLe => Value::Bool(a <= b),
            BinOp::FGt => Value::Bool(a > b),
            BinOp::FGe => Value::Bool(a >= b),
            BinOp::Eq => Value::Bool(a == b),
            BinOp::Ne => Value::Bool(a != b),
            _ => unreachable!("checked operator"),
        }),
        (Value::Str(a), Value::Str(b)) => Ok(match op {
            BinOp::Concat => Value::Str([&*a, &*b].concat().into()),
            // Rust's `str` order: by bytes, a prefix first.
            BinOp::Lt => Value::Bool(a < b),
            BinOp::Le => Value::Bool(a <= b),
            BinOp::Gt => Value::Bool(a > b),
            BinOp::Ge => Value::Bool(a >= b),
            BinOp::Eq => Value::Bool(a == b),
            BinOp::Ne => Value::Bool(a != b),
            _ => unreachable!("checked operator"),
        }),
        (a, b) => Ok(Value::Bool((a == b) == (op == BinOp::Eq))),
    }
}
