//! Lowering: a checked `Program` to an `ir::Module`.
//!
//! Every type comes from the checker, which records the type of each expression by its span
//! (`Program::types`); lowering only resolves names to ids and indexes. It assumes the program
//! checked, so an unexpected shape is a bug in lowering, not in the program.

// `ExprKind`, `PatKind` and `StmtKind` are the AST's; the IR's are written `ir::`.
use crate::ast::{self, ExprKind, PatKind, StmtKind};
use crate::check::{self, builtin, key, shape, variant, CapTable, Program};
use crate::ir::{self, *};
use crate::Span;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Lowers `p`, which `check` built with `caps`.
pub fn lower(p: &Program, caps: &CapTable) -> Module {
    let sorted = |keys: Vec<&String>| {
        let mut keys: Vec<String> = keys.into_iter().cloned().collect();
        keys.sort();
        keys
    };
    // A generic function becomes one function per instance, made when a use is lowered.
    let funcs = sorted(p.funcs.keys().filter(|k| p.sigs[*k].tparams.is_empty()).collect());
    // So is a generic type: one struct or enum per instance, made when its type is lowered.
    let plain = |k: &&String| p.tparams[*k].is_empty();
    let structs = sorted(p.structs.keys().filter(plain).collect());
    // The capability error enums, then `Json`, at fixed ids; then the program's.
    let mut enums: Vec<String> = caps.enums.iter().map(|e| e.name.clone()).collect();
    enums.push("Json".to_string());
    enums.extend(sorted(
        p.enums.keys().filter(|k| !enums.contains(k) && plain(k)).collect(),
    ));
    let structs: Vec<check::Ty> = structs.into_iter().map(|k| check::Ty::Struct(k, Vec::new())).collect();
    let enums: Vec<check::Ty> = enums.into_iter().map(|k| check::Ty::Enum(k, Vec::new())).collect();
    let ids = Ids {
        funcs: index(&funcs),
        structs: Types::new(structs),
        enums: Types::new(enums),
        caps: index(&caps.caps.iter().map(|c| c.name.clone()).collect::<Vec<_>>()),
        lifted: RefCell::new(Vec::new()),
        instances: RefCell::new(HashMap::new()),
        pending: RefCell::new(Vec::new()),
        closures: Cell::new(0),
    };
    let mut lowered: Vec<Func> = funcs.iter().map(|k| ids.func(p, caps, k, Vec::new())).collect();
    // Lowering an instance can find more; `check` proved there are finitely many.
    for next in 0.. {
        let item = ids.pending.borrow().get(next).cloned();
        let Some((index, k, args)) = item else { break };
        let binds = p.sigs[&k].tparams.iter().cloned().zip(args).collect();
        let f = ids.func(p, caps, &k, binds);
        ids.lifted.borrow_mut()[index] = Some(f);
    }
    lowered.extend(
        ids.lifted
            .take()
            .into_iter()
            .map(|f| f.expect("lifted closure or instance")),
    );
    // A type's parts can name more instances; `check` proved there are finitely many. Display
    // shows an instance's name without its type arguments, as Rust's derived `Debug` does.
    let (mut structs, mut enums) = (Vec::new(), Vec::new());
    loop {
        let s = ids.structs.list.borrow().get(structs.len()).cloned();
        let e = ids.enums.list.borrow().get(enums.len()).cloned();
        match (s, e) {
            (Some(ref ty @ check::Ty::Struct(ref k, _)), _) => structs.push(StructDef {
                fields: (check::fields_of(&p.fields, &p.tparams, ty).iter())
                    .map(|(n, t)| (n.clone(), ids.held(t)))
                    .collect(),
                name: k.clone(),
            }),
            (_, Some(ref ty @ check::Ty::Enum(ref k, _))) => enums.push(EnumDef {
                variants: (shape(&p.variants, &p.tparams, ty).1.iter())
                    .map(|(n, ts)| (n.clone(), ts.iter().map(|t| ids.held(t)).collect()))
                    .collect(),
                name: k.clone(),
            }),
            (None, None) => break,
            _ => unreachable!("structs and enums"),
        }
    }
    Module {
        funcs: lowered,
        structs,
        enums,
        caps: (caps.caps.iter())
            .map(|c| CapDef {
                name: c.name.clone(),
                methods: (c.methods.iter())
                    .map(|(n, ps, r)| (n.clone(), ps.iter().map(|t| ids.ty(t)).collect(), ids.ty(r)))
                    .collect(),
            })
            .collect(),
        main: FuncId(ids.funcs["main"]),
    }
}

fn index(keys: &[String]) -> HashMap<String, u32> {
    keys.iter().enumerate().map(|(i, k)| (k.clone(), i as u32)).collect()
}

/// The structs or enums of the module: each type, generic ones by instance, and its id.
struct Types {
    list: RefCell<Vec<check::Ty>>,
    ids: RefCell<HashMap<check::Ty, u32>>,
}

impl Types {
    fn new(list: Vec<check::Ty>) -> Types {
        let ids = list.iter().enumerate().map(|(i, t)| (t.clone(), i as u32)).collect();
        Types {
            list: RefCell::new(list),
            ids: RefCell::new(ids),
        }
    }

    /// The id of `ty`, which holds no type parameter; a new one if it is a new instance.
    fn id(&self, ty: &check::Ty) -> u32 {
        if let Some(&i) = self.ids.borrow().get(ty) {
            return i;
        }
        let mut list = self.list.borrow_mut();
        list.push(ty.clone());
        let i = list.len() as u32 - 1;
        self.ids.borrow_mut().insert(ty.clone(), i);
        i
    }
}

/// The id of every function, struct, enum and capability, by key.
struct Ids {
    funcs: HashMap<String, u32>,
    structs: Types,
    enums: Types,
    caps: HashMap<String, u32>,
    /// Closures lifted to functions and instances of generic functions, which follow the
    /// named ones in the order they are found; `None` until lowered.
    lifted: RefCell<Vec<Option<Func>>>,
    /// The id of each instance, by generic function key and type arguments.
    instances: RefCell<HashMap<(String, Vec<check::Ty>), FuncId>>,
    /// Every instance: its index in `lifted`, key and type arguments.
    pending: RefCell<Vec<(usize, String, Vec<check::Ty>)>>,
    /// Closures lifted so far from the function being lowered, for their names.
    closures: Cell<u32>,
}

impl Ids {
    /// A slot after the named functions, filled once its function is lowered.
    fn reserve(&self) -> (usize, FuncId) {
        let mut lifted = self.lifted.borrow_mut();
        lifted.push(None);
        let index = lifted.len() - 1;
        (index, FuncId((self.funcs.len() + index) as u32))
    }

    /// The instance of generic function `k` for type arguments `args`, which hold no type
    /// parameter; lowered later if new.
    fn instance(&self, k: &str, args: Vec<check::Ty>) -> FuncId {
        let id = (k.to_string(), args);
        if let Some(f) = self.instances.borrow().get(&id) {
            return *f;
        }
        let (index, f) = self.reserve();
        self.pending.borrow_mut().push((index, id.0.clone(), id.1.clone()));
        self.instances.borrow_mut().insert(id, f);
        f
    }

    fn ty(&self, t: &check::Ty) -> Ty {
        let inner = |t| Box::new(self.held(t));
        match t {
            check::Ty::Int => Ty::Int,
            check::Ty::Float => Ty::Float,
            check::Ty::Bool => Ty::Bool,
            check::Ty::Str => Ty::Str,
            check::Ty::Bytes => Ty::Bytes,
            check::Ty::Unit => Ty::Unit,
            check::Ty::Struct(..) => Ty::Struct(StructId(self.structs.id(t))),
            check::Ty::Enum(..) => Ty::Enum(EnumId(self.enums.id(t))),
            check::Ty::Option(t) => Ty::Option(inner(t)),
            check::Ty::Result(t, e) => Ty::Result(inner(t), inner(e)),
            check::Ty::List(t) => Ty::List(inner(t)),
            check::Ty::Map(k, v) => Ty::Map(inner(k), inner(v)),
            check::Ty::Cap(c) => Ty::Cap(CapId(self.caps[c])),
            check::Ty::Fn(ps, r) => Ty::Fn(ps.iter().map(|t| self.held(t)).collect(), inner(r)),
            check::Ty::Never => Ty::Never,
            check::Ty::Param(t) => unreachable!("type parameter `{t}` outside an instance"),
        }
    }

    /// The type of a part of a value, or of a local. Only a diverging expression, such as
    /// `Some({ return x; })`, gives `!` to one; that value never exists, so any type will do.
    fn held(&self, t: &check::Ty) -> Ty {
        match t {
            check::Ty::Never => Ty::Unit,
            t => self.ty(t),
        }
    }

    /// Function `k`, or its instance for `binds` (each type parameter and its type), named
    /// like `first<Int>`.
    fn func(&self, p: &Program, caps: &CapTable, k: &str, binds: Vec<(String, check::Ty)>) -> Func {
        let (f, sig) = (&p.funcs[k], &p.sigs[k]);
        let name = match binds.is_empty() {
            true => k.to_string(),
            false => {
                let args: Vec<String> = binds.iter().map(|(_, t)| t.to_string()).collect();
                format!("{k}<{}>", args.join(", "))
            }
        };
        self.closures.set(0);
        let mut l = Lowerer {
            p,
            caps,
            ids: self,
            module: check::module_of(k),
            owner: name.clone(),
            binds,
            locals: Vec::new(),
            scope: Vec::new(),
        };
        for (param, ty) in f.params.iter().zip(&sig.params) {
            l.bind(&param.name, &l.subst(ty), param.span);
        }
        let body = l.block(&f.body);
        let ret = self.ty(&l.subst(&sig.ret));
        Func {
            name,
            locals: l.locals,
            params: f.params.len() as u32,
            ret,
            body,
            span: f.span,
        }
    }
}

/// Lowers one function.
struct Lowerer<'a> {
    p: &'a Program,
    caps: &'a CapTable,
    ids: &'a Ids,
    /// The module of the function.
    module: &'a str,
    /// The name of the function, or of the one a closure is in, for closure names.
    owner: String,
    /// The type arguments of the instance being lowered; empty for a function not generic.
    binds: Vec<(String, check::Ty)>,
    locals: Vec<Local>,
    /// Names in scope, innermost last.
    scope: Vec<(&'a str, LocalId)>,
}

impl<'a> Lowerer<'a> {
    /// A new slot for `name`, which shadows any earlier one.
    fn bind(&mut self, name: &'a str, ty: &check::Ty, span: Span) -> LocalId {
        self.bind_ty(name, self.ids.held(ty), span)
    }

    fn bind_ty(&mut self, name: &'a str, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(Local {
            name: name.to_string(),
            ty,
            span,
        });
        self.scope.push((name, id));
        id
    }

    fn lookup(&self, name: &str) -> LocalId {
        self.local(name).expect("checked name")
    }

    /// The local `name`, if one is in scope; else `name` is a function.
    fn local(&self, name: &str) -> Option<LocalId> {
        self.scope.iter().rev().find(|(n, _)| *n == name).map(|(_, id)| *id)
    }

    /// Function `k` named by the expression at `span`: the instance the checker recorded
    /// there, for a generic function.
    fn func(&self, span: Span, k: &str) -> FuncId {
        match self.p.instances.get(&span) {
            Some((g, args)) => self.ids.instance(g, args.iter().map(|t| self.subst(t)).collect()),
            None => FuncId(self.ids.funcs[k]),
        }
    }

    /// `ty` in the instance being lowered.
    fn subst(&self, ty: &check::Ty) -> check::Ty {
        check::subst(ty, &self.binds)
    }

    /// The checker's type of the expression or `let` at `span`, in this instance.
    fn checked(&self, span: Span) -> check::Ty {
        self.subst(&self.p.types[&span])
    }

    fn block(&mut self, b: &'a ast::Block) -> Block {
        let mark = self.scope.len();
        let stmts = b.stmts.iter().map(|s| self.stmt(s)).collect();
        let tail = b.tail.as_ref().map(|t| Box::new(self.expr(t)));
        self.scope.truncate(mark);
        Block { stmts, tail }
    }

    fn stmt(&mut self, s: &'a ast::Stmt) -> Stmt {
        let kind = match &s.kind {
            StmtKind::Let { name, init, .. } => {
                let init = self.expr(init);
                ir::StmtKind::Let(self.bind(name, &self.checked(s.span), s.span), init)
            }
            StmtKind::Assign { target, op, value } => {
                let place = self.place(target);
                let op = match *op {
                    "=" => None,
                    _ => Some(match (*op, self.checked(target.span)) {
                        ("-=", check::Ty::Float) => BinOp::FSub,
                        ("-=", _) => BinOp::Sub,
                        (_, check::Ty::Str) => BinOp::Concat,
                        (_, check::Ty::Float) => BinOp::FAdd,
                        _ => BinOp::Add,
                    }),
                };
                ir::StmtKind::Assign(place, op, self.expr(value))
            }
            StmtKind::Return(value) => ir::StmtKind::Return(value.as_ref().map(|e| self.expr(e))),
            StmtKind::While { cond, body } => ir::StmtKind::While(self.expr(cond), self.block(body)),
            StmtKind::Loop { body } => ir::StmtKind::Loop(self.block(body)),
            StmtKind::For { name, iter, body } => {
                let mark = self.scope.len();
                let kind = match iter {
                    ast::Iter::List(xs) => {
                        let xs = self.expr(xs);
                        let check::Ty::List(t) = self.checked(xs.span) else {
                            unreachable!("checked list");
                        };
                        let x = self.bind(name, &t, s.span);
                        ir::StmtKind::ForList(x, xs, self.block(body))
                    }
                    ast::Iter::Range(a, b) => {
                        let (a, b) = (self.expr(a), self.expr(b));
                        let i = self.bind(name, &check::Ty::Int, s.span);
                        ir::StmtKind::ForRange(i, a, b, self.block(body))
                    }
                };
                self.scope.truncate(mark);
                kind
            }
            StmtKind::Break => ir::StmtKind::Break,
            StmtKind::Continue => ir::StmtKind::Continue,
            StmtKind::Expr(e) => ir::StmtKind::Expr(self.expr(e)),
        };
        Stmt { kind, span: s.span }
    }

    /// An assignment target or a mutating method's receiver: a local, then fields and indexes.
    fn place(&mut self, e: &'a ast::Expr) -> Place {
        match &e.kind {
            ExprKind::Name(name) => Place {
                root: self.lookup(name),
                steps: Vec::new(),
            },
            ExprKind::Field(base, name) => {
                let mut place = self.place(base);
                let field = self.field(base.span, name);
                place.steps.push(PlaceStep::Field(field));
                place
            }
            ExprKind::Index(base, index) => {
                let mut place = self.place(base);
                place.steps.push(PlaceStep::Index(self.expr(index), e.span));
                place
            }
            _ => unreachable!("checked place"),
        }
    }

    /// The index of field `name` in the struct that the expression at `base` has.
    fn field(&self, base: Span, name: &str) -> u32 {
        let check::Ty::Struct(s, _) = self.checked(base) else {
            unreachable!("checked field");
        };
        self.p.fields[&s]
            .iter()
            .position(|(n, _)| n == name)
            .expect("checked field") as u32
    }

    /// The enum of a value of type `ty`, and the index of its variant `v`.
    fn variant(&self, ty: &check::Ty, v: &str) -> Option<(EnumRef, u32)> {
        let (i, _) = variant(&shape(&self.p.variants, &self.p.tparams, ty).1, v)?;
        let en = match ty {
            check::Ty::Option(_) => EnumRef::Option,
            check::Ty::Result(..) => EnumRef::Result,
            check::Ty::Enum(..) => EnumRef::User(EnumId(self.ids.enums.id(ty))),
            _ => unreachable!("only enums have variants"),
        };
        Some((en, i as u32))
    }

    fn exprs(&mut self, es: &'a [ast::Expr]) -> Vec<Expr> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    fn expr(&mut self, e: &'a ast::Expr) -> Expr {
        let ty = &self.checked(e.span);
        let kind = match &e.kind {
            ExprKind::Int(n) => ir::ExprKind::Int(*n),
            ExprKind::Float(x) => ir::ExprKind::Float(f64::from_bits(*x)),
            ExprKind::Str(s) => ir::ExprKind::Str(s.as_str().into()),
            ExprKind::Bool(b) => ir::ExprKind::Bool(*b),
            ExprKind::Unit => ir::ExprKind::Unit,
            ExprKind::Name(name) if builtin(name).is_some() => self.variant_value(ty, name, &[]),
            ExprKind::Name(name) => match self.local(name) {
                Some(x) => ir::ExprKind::Local(x),
                None => ir::ExprKind::FnRef(self.func(e.span, &key(self.module, name))),
            },
            ExprKind::Turbofish(name, _) => ir::ExprKind::FnRef(self.func(e.span, &key(self.module, name))),
            ExprKind::Unary(op, x) => {
                let op = match *op {
                    "-" if *ty == check::Ty::Float => UnOp::FNeg,
                    "-" => UnOp::Neg,
                    _ => UnOp::Not,
                };
                ir::ExprKind::Unary(op, Box::new(self.expr(x)))
            }
            ExprKind::Binary(op, a, b) => {
                let float = self.checked(a.span) == check::Ty::Float;
                let op = match *op {
                    "+" if float => BinOp::FAdd,
                    "-" if float => BinOp::FSub,
                    "*" if float => BinOp::FMul,
                    "/" if float => BinOp::FDiv,
                    "%" if float => BinOp::FRem,
                    "<" if float => BinOp::FLt,
                    "<=" if float => BinOp::FLe,
                    ">" if float => BinOp::FGt,
                    ">=" if float => BinOp::FGe,
                    "+" if *ty == check::Ty::Str => BinOp::Concat,
                    "+" => BinOp::Add,
                    "-" => BinOp::Sub,
                    "*" => BinOp::Mul,
                    "/" => BinOp::Div,
                    "%" => BinOp::Rem,
                    "<" => BinOp::Lt,
                    "<=" => BinOp::Le,
                    ">" => BinOp::Gt,
                    ">=" => BinOp::Ge,
                    "==" => BinOp::Eq,
                    "!=" => BinOp::Ne,
                    "&&" => BinOp::And,
                    "||" => BinOp::Or,
                    _ => unreachable!("checked operator"),
                };
                ir::ExprKind::Binary(op, Box::new(self.expr(a)), Box::new(self.expr(b)))
            }
            ExprKind::If(cond, then, els) => ir::ExprKind::If(
                Box::new(self.expr(cond)),
                self.block(then),
                els.as_ref().map(|x| Box::new(self.expr(x))),
            ),
            ExprKind::Block(b) => ir::ExprKind::Block(self.block(b)),
            ExprKind::Call(callee, args) => self.call(ty, callee, args),
            ExprKind::Method(recv, name, args) => self.method(recv, name, args),
            ExprKind::Field(base, name) => {
                let field = self.field(base.span, name);
                ir::ExprKind::Field(Box::new(self.expr(base)), field)
            }
            ExprKind::Index(base, index) => ir::ExprKind::Index(Box::new(self.expr(base)), Box::new(self.expr(index))),
            ExprKind::List(items) => ir::ExprKind::List(self.exprs(items)),
            ExprKind::StructLit(_, inits) => self.struct_lit(ty, inits),
            ExprKind::Path(_, v) => self.variant_value(ty, v, &[]),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::StructLit(_, inits) => self.struct_lit(ty, inits),
                ExprKind::Path(_, v) => self.variant_value(ty, v, &[]),
                ExprKind::Name(name) | ExprKind::Turbofish(name, _) => {
                    ir::ExprKind::FnRef(self.func(e.span, &format!("{m}::{name}")))
                }
                _ => unreachable!("checked module path"),
            },
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms),
            ExprKind::IfLet(pat, scrut, then, els) => {
                let scrut = self.expr(scrut);
                let mark = self.scope.len();
                let pat = self.pat(pat, &self.checked(scrut.span));
                let then = Expr {
                    kind: ir::ExprKind::Block(self.block(then)),
                    ty: self.ids.ty(ty),
                    span: then.span,
                };
                self.scope.truncate(mark);
                let els = match els {
                    Some(x) => self.expr(x),
                    None => Expr {
                        kind: ir::ExprKind::Unit,
                        ty: Ty::Unit,
                        span: e.span,
                    },
                };
                let arms = vec![
                    Arm { pat, body: then },
                    Arm {
                        pat: Pat::Wild,
                        body: els,
                    },
                ];
                ir::ExprKind::Match(Box::new(scrut), arms)
            }
            ExprKind::Try(x) => ir::ExprKind::Try(Box::new(self.expr(x))),
            ExprKind::Closure(params, body) => self.closure(e.span, ty, params, body),
        };
        Expr {
            kind,
            ty: self.ids.ty(ty),
            span: e.span,
        }
    }

    fn call(&mut self, ty: &check::Ty, callee: &'a ast::Expr, args: &'a [ast::Expr]) -> ir::ExprKind {
        let k = match &callee.kind {
            ExprKind::Name(name) if builtin(name).is_some() => return self.variant_value(ty, name, args),
            ExprKind::Name(name) if self.local(name).is_none() => key(self.module, name).into_owned(),
            ExprKind::Turbofish(name, _) => key(self.module, name).into_owned(),
            ExprKind::Path(en, _) if en == "Map" => return ir::ExprKind::MapNew,
            ExprKind::Path(_, v) => return self.variant_value(ty, v, args),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::Name(name) | ExprKind::Turbofish(name, _) => format!("{m}::{name}"),
                ExprKind::Path(_, v) => return self.variant_value(ty, v, args),
                _ => unreachable!("checked callee"),
            },
            _ => return ir::ExprKind::CallValue(Box::new(self.expr(callee)), self.exprs(args)),
        };
        ir::ExprKind::Call(self.func(callee.span, &k), self.exprs(args))
    }

    /// Lifts the closure at `span`, of type `ty`, to a new function whose parameters are its
    /// captures, then its own; the closure value copies the captured locals.
    fn closure(
        &mut self,
        span: Span,
        ty: &check::Ty,
        params: &'a [ast::ClosureParam],
        body: &'a ast::Expr,
    ) -> ir::ExprKind {
        let check::Ty::Fn(tys, ret) = ty else {
            unreachable!("checked closure");
        };
        let mut inner = Lowerer {
            p: self.p,
            caps: self.caps,
            ids: self.ids,
            module: self.module,
            owner: self.owner.clone(),
            binds: self.binds.clone(),
            locals: Vec::new(),
            scope: Vec::new(),
        };
        let names = &self.p.captures[&span];
        let mut captured = Vec::new();
        for name in names {
            let x = self.lookup(name);
            let local = &self.locals[x.0 as usize];
            // The checker refuses this; a capability in a closure would carry authority.
            assert!(!matches!(local.ty, Ty::Cap(_)), "closure captures capability `{name}`");
            inner.bind_ty(name, local.ty.clone(), local.span);
            captured.push(x);
        }
        for (p, t) in params.iter().zip(tys) {
            inner.bind(&p.name, t, p.span);
        }
        let n = self.ids.closures.get();
        self.ids.closures.set(n + 1);
        let (index, id) = self.ids.reserve();
        let body = inner.expr(body);
        let f = Func {
            name: format!("{}::{{closure#{n}}}", self.owner),
            locals: inner.locals,
            params: (names.len() + params.len()) as u32,
            ret: self.ids.ty(ret),
            body: Block {
                stmts: Vec::new(),
                tail: Some(Box::new(body)),
            },
            span,
        };
        self.ids.lifted.borrow_mut()[index] = Some(f);
        ir::ExprKind::Closure(id, captured)
    }

    /// `E::V(args)`, `Some(x)` and the like, where `ty` is the value's enum type.
    fn variant_value(&mut self, ty: &check::Ty, v: &str, args: &'a [ast::Expr]) -> ir::ExprKind {
        let (en, i) = self.variant(ty, v).expect("checked variant");
        ir::ExprKind::Variant(en, i, self.exprs(args))
    }

    /// Fields keep source order, which is evaluation order.
    fn struct_lit(&mut self, ty: &check::Ty, inits: &'a [(String, ast::Expr)]) -> ir::ExprKind {
        let check::Ty::Struct(k, _) = ty else {
            unreachable!("checked struct literal");
        };
        let fields = &self.p.fields[k];
        let inits = (inits.iter())
            .map(|(name, x)| {
                let field = fields.iter().position(|(n, _)| n == name).expect("checked field");
                (field as u32, self.expr(x))
            })
            .collect();
        ir::ExprKind::StructLit(StructId(self.ids.structs.id(ty)), inits)
    }

    fn method(&mut self, recv: &'a ast::Expr, name: &str, args: &'a [ast::Expr]) -> ir::ExprKind {
        use check::Ty as T;
        let mutation = match (&self.checked(recv.span), name) {
            (T::Cap(c), _) => {
                let ExprKind::Name(cap) = &recv.kind else {
                    unreachable!("checked capability receiver");
                };
                let methods = &self.caps.caps[self.ids.caps[c] as usize].methods;
                let m = methods.iter().position(|(n, ..)| n == name).expect("checked method");
                return ir::ExprKind::CapCall(self.lookup(cap), m as u32, self.exprs(args));
            }
            (T::List(_), "push") => Mut::ListPush,
            (T::List(_), "sort") => Mut::ListSort,
            (T::List(_), "pop") => Mut::ListPop,
            (T::Bytes, "push") => Mut::BytesPush,
            (T::Map(..), "insert") => Mut::MapInsert,
            (T::Map(..), "remove") => Mut::MapRemove,
            (recv_ty, _) => {
                let b = match (recv_ty, name) {
                    (T::Int, "to_str") => Builtin::IntToStr,
                    (T::Str, "len") => Builtin::StrLen,
                    (T::Str, "to_bytes") => Builtin::StrToBytes,
                    (T::Str, "split") => Builtin::StrSplit,
                    (T::Str, "lines") => Builtin::StrLines,
                    (T::Str, "trim") => Builtin::StrTrim,
                    (T::Str, "contains") => Builtin::StrContains,
                    (T::Str, "starts_with") => Builtin::StrStartsWith,
                    (T::Str, "parse_int") => Builtin::StrParseInt,
                    (T::Str, "ends_with") => Builtin::StrEndsWith,
                    (T::Str, "strip_prefix") => Builtin::StrStripPrefix,
                    (T::Str, "strip_suffix") => Builtin::StrStripSuffix,
                    (T::Str, "find") => Builtin::StrFind,
                    (T::Str, "replace") => Builtin::StrReplace,
                    (T::List(_), "contains") => Builtin::ListContains,
                    (T::Bytes, "len") => Builtin::BytesLen,
                    (T::Bytes, "to_str") => Builtin::BytesToStr,
                    (T::List(_), "len") => Builtin::ListLen,
                    (T::Map(..), "len") => Builtin::MapLen,
                    (T::Map(..), "keys") => Builtin::MapKeys,
                    (T::Map(..), "get") => Builtin::MapGet,
                    (T::Option(_), "unwrap_or") => Builtin::OptionUnwrapOr,
                    (T::Option(_), "is_some") => Builtin::OptionIsSome,
                    (T::Option(_), "is_none") => Builtin::OptionIsNone,
                    (T::Result(..), "unwrap_or") => Builtin::ResultUnwrapOr,
                    (T::Result(..), "is_ok") => Builtin::ResultIsOk,
                    (T::Result(..), "is_err") => Builtin::ResultIsErr,
                    (T::Result(..), "ok") => Builtin::ResultOk,
                    (T::Result(..), "err") => Builtin::ResultErr,
                    (T::Option(_), "ok_or") => Builtin::OptionOkOr,
                    (T::Int, "to_float") => Builtin::IntToFloat,
                    (T::Str, "parse_float") => Builtin::StrParseFloat,
                    (T::Float, "to_str") => Builtin::FloatToStr,
                    (T::Float, "to_int") => Builtin::FloatToInt,
                    (T::Float, "abs") => Builtin::FloatAbs,
                    (T::Float, "floor") => Builtin::FloatFloor,
                    (T::Float, "ceil") => Builtin::FloatCeil,
                    (T::Float, "round") => Builtin::FloatRound,
                    (T::Float, "sqrt") => Builtin::FloatSqrt,
                    (T::Float, "is_nan") => Builtin::FloatIsNan,
                    (T::Float, "is_finite") => Builtin::FloatIsFinite,
                    (T::Float, "is_infinite") => Builtin::FloatIsInfinite,
                    (T::Str, "slice") => Builtin::StrSlice,
                    (T::Bytes, "slice") => Builtin::BytesSlice,
                    (T::Option(_), "map") => Builtin::OptionMap,
                    (T::Option(_), "and_then") => Builtin::OptionAndThen,
                    (T::Result(..), "map") => Builtin::ResultMap,
                    (T::Result(..), "map_err") => Builtin::ResultMapErr,
                    (T::Result(..), "and_then") => Builtin::ResultAndThen,
                    (T::List(_), "map") => Builtin::ListMap,
                    (T::List(_), "filter") => Builtin::ListFilter,
                    (T::List(_), "any") => Builtin::ListAny,
                    (T::List(_), "all") => Builtin::ListAll,
                    (T::Str, "parse_json") => Builtin::StrParseJson,
                    (T::Enum(..), "to_json") => Builtin::JsonToJson,
                    (T::Enum(..), "get") => Builtin::JsonGet,
                    (T::Enum(..), "at") => Builtin::JsonAt,
                    (T::Enum(..), "as_str") => Builtin::JsonAsStr,
                    (T::Enum(..), "as_int") => Builtin::JsonAsInt,
                    (T::Enum(..), "as_float") => Builtin::JsonAsFloat,
                    (T::Enum(..), "as_bool") => Builtin::JsonAsBool,
                    (T::Enum(..), "as_list") => Builtin::JsonAsList,
                    (T::Enum(..), "as_map") => Builtin::JsonAsMap,
                    _ => unreachable!("checked method"),
                };
                return ir::ExprKind::Builtin(b, Box::new(self.expr(recv)), self.exprs(args));
            }
        };
        // The place's indexes come before the arguments.
        let place = self.place(recv);
        ir::ExprKind::Mutate(place, mutation, self.exprs(args))
    }

    fn match_expr(&mut self, scrut: &'a ast::Expr, arms: &'a [ast::Arm]) -> ir::ExprKind {
        let scrut = self.expr(scrut);
        let st = self.checked(scrut.span);
        let arms = (arms.iter())
            .map(|arm| {
                let mark = self.scope.len();
                let pat = self.pat(&arm.pat, &st);
                let body = self.expr(&arm.body);
                self.scope.truncate(mark);
                Arm { pat, body }
            })
            .collect();
        ir::ExprKind::Match(Box::new(scrut), arms)
    }

    /// Pattern `p` on a scrutinee of type `st`, binding its names in new slots.
    fn pat(&mut self, p: &'a ast::Pat, st: &check::Ty) -> Pat {
        match &p.kind {
            PatKind::Wild => Pat::Wild,
            PatKind::Int(n) => Pat::Int(*n),
            PatKind::Str(s) => Pat::Str(s.clone()),
            PatKind::Bool(b) => Pat::Bool(*b),
            PatKind::Name(n) => match self.variant(st, n) {
                Some((_, i)) => Pat::Variant(i, Vec::new()),
                None => Pat::Bind(self.bind(n, st, p.span)),
            },
            PatKind::Module(_, inner) => self.pat(inner, st),
            PatKind::Variant(_, v, parts) => {
                let (_, i) = self.variant(st, v).expect("checked variant");
                let payload = &shape(&self.p.variants, &self.p.tparams, st).1[i as usize].1;
                Pat::Variant(i, parts.iter().zip(payload).map(|(q, ty)| self.pat(q, ty)).collect())
            }
        }
    }
}
