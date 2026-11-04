use crate::ast::*;
use crate::check::Program;
use crate::Span;
use std::collections::HashMap;

/// Bounds Rust recursion, and so stack use, for any program.
const MAX_DEPTH: usize = 3_000;

/// Host capabilities. Empty until task 9.
pub trait Caps {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    Unit,
    /// The struct's name and its field values in declaration order.
    Struct(String, Vec<Value>),
    /// The enum's name, the variant's index in declaration order and its payload.
    Enum(String, usize, Vec<Value>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunErr {
    pub span: Span,
    pub msg: String,
}

/// Why evaluation left the normal path.
enum Stop {
    Return(Value),
    Err(RunErr),
}

type Res = Result<Value, Box<Stop>>;

fn fail(span: Span, msg: &str) -> Box<Stop> {
    Box::new(Stop::Err(RunErr {
        span,
        msg: msg.to_string(),
    }))
}

/// Evaluates `main`. Recursion is capped at `MAX_DEPTH` nested expressions, which needs
/// about 6 MB of stack in debug builds and 3 MB in release. Callers provide that stack.
pub fn run(p: &Program, _caps: &mut dyn Caps) -> Result<Value, RunErr> {
    let mut it = Interp {
        funcs: &p.funcs,
        structs: &p.structs,
        enums: &p.enums,
        locals: Vec::new(),
        base: 0,
        depth: 0,
    };
    let main = &p.funcs["main"];
    match returned(it.block(&main.body)) {
        Ok(v) => Ok(v),
        Err(s) => match *s {
            Stop::Err(e) => Err(e),
            Stop::Return(_) => unreachable!(),
        },
    }
}

struct Interp<'a> {
    funcs: &'a HashMap<String, Fn>,
    structs: &'a HashMap<String, Struct>,
    enums: &'a HashMap<String, Enum>,
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

    fn assign(&mut self, target: &Expr, op: &str, value: &Expr, span: Span) -> Result<(), Box<Stop>> {
        let mut path = Vec::new();
        let mut root = target;
        while let ExprKind::Field(base, field) = &root.kind {
            path.push(field.as_str());
            root = base;
        }
        let ExprKind::Name(name) = &root.kind else {
            unreachable!("checked target");
        };
        let mut v = self.expr(value)?;
        let structs = self.structs;
        let mut slot = self.lookup(name);
        for field in path.into_iter().rev() {
            let Value::Struct(s, vals) = slot else {
                unreachable!("checked field");
            };
            slot = &mut vals[field_index(structs, s, field)];
        }
        if op != "=" {
            v = binary(&op[..1], slot.clone(), v, span)?;
        }
        *slot = v;
        Ok(())
    }

    fn while_stmt(&mut self, cond: &Expr, body: &Block) -> Result<(), Box<Stop>> {
        while self.expr(cond)? == Value::Bool(true) {
            self.block(body)?;
        }
        Ok(())
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
            ExprKind::Name(name) => Ok(self.lookup(name).clone()),
            ExprKind::Int(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Unit => Ok(literal(e)),
            ExprKind::Unary(op, x) => self.unary(op, x, e.span),
            ExprKind::Binary(op @ ("&&" | "||"), a, b) => self.logical(op, a, b),
            ExprKind::Binary(op, a, b) => self.arith(op, a, b, e.span),
            ExprKind::If(cond, then, els) => self.if_expr(cond, then, els.as_deref()),
            ExprKind::Block(b) => self.block(b),
            ExprKind::Call(callee, args) => self.call_expr(callee, args),
            ExprKind::Field(base, field) => self.field(base, field),
            ExprKind::StructLit(name, inits) => self.struct_lit(name, inits),
            ExprKind::Path(en, v) => Ok(self.variant(en, v, Vec::new())),
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms),
            ExprKind::Method(..) | ExprKind::Index(..) => unreachable!("rejected by check"),
        }
    }

    fn field(&mut self, base: &Expr, field: &str) -> Res {
        let Value::Struct(s, mut vals) = self.expr(base)? else {
            unreachable!("checked field");
        };
        Ok(vals.swap_remove(field_index(self.structs, &s, field)))
    }

    /// Fields are evaluated in source order and stored in declaration order.
    fn struct_lit(&mut self, name: &str, inits: &[(String, Expr)]) -> Res {
        let mut vals = vec![Value::Unit; inits.len()];
        for (field, x) in inits {
            vals[field_index(self.structs, name, field)] = self.expr(x)?;
        }
        Ok(Value::Struct(name.to_string(), vals))
    }

    fn variant(&self, en: &str, v: &str, payload: Vec<Value>) -> Value {
        let i = variant_index(self.enums, en, v).expect("checked variant");
        Value::Enum(en.to_string(), i, payload)
    }

    /// The first arm whose pattern matches runs, with its bindings as locals.
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
        let name = match &callee.kind {
            ExprKind::Name(name) => name,
            ExprKind::Path(en, v) => {
                let vals = args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?;
                return Ok(self.variant(en, v, vals));
            }
            _ => unreachable!("checked callee"),
        };
        let f = &self.funcs[name.as_str()];
        let caller = self.enter(f, args)?;
        let result = self.block(&f.body);
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
    enums[name].variants.iter().position(|v| v.name == variant)
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
