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
use std::collections::HashMap;

/// Lowers `p`, which `check` built with `caps`.
pub fn lower(p: &Program, caps: &CapTable) -> Module {
    let sorted = |keys: Vec<&String>| {
        let mut keys: Vec<String> = keys.into_iter().cloned().collect();
        keys.sort();
        keys
    };
    let funcs = sorted(p.funcs.keys().collect());
    let structs = sorted(p.structs.keys().collect());
    let mut enums: Vec<String> = caps.enums.iter().map(|e| e.name.clone()).collect();
    enums.extend(sorted(p.enums.keys().filter(|k| !enums.contains(k)).collect()));
    let ids = Ids {
        funcs: index(&funcs),
        structs: index(&structs),
        enums: index(&enums),
        caps: index(&caps.caps.iter().map(|c| c.name.clone()).collect::<Vec<_>>()),
    };
    Module {
        funcs: funcs.iter().map(|k| ids.func(p, caps, k)).collect(),
        structs: (structs.iter())
            .map(|k| StructDef {
                name: k.clone(),
                fields: p.fields[k].iter().map(|(n, t)| (n.clone(), ids.ty(t))).collect(),
            })
            .collect(),
        enums: (enums.iter())
            .map(|k| EnumDef {
                name: k.clone(),
                variants: (p.variants[k].iter())
                    .map(|(n, ts)| (n.clone(), ts.iter().map(|t| ids.ty(t)).collect()))
                    .collect(),
            })
            .collect(),
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

/// The id of every function, struct, enum and capability, by key.
struct Ids {
    funcs: HashMap<String, u32>,
    structs: HashMap<String, u32>,
    enums: HashMap<String, u32>,
    caps: HashMap<String, u32>,
}

impl Ids {
    fn ty(&self, t: &check::Ty) -> Ty {
        let inner = |t| Box::new(self.held(t));
        match t {
            check::Ty::Int => Ty::Int,
            check::Ty::Bool => Ty::Bool,
            check::Ty::Str => Ty::Str,
            check::Ty::Bytes => Ty::Bytes,
            check::Ty::Unit => Ty::Unit,
            check::Ty::Struct(k) => Ty::Struct(StructId(self.structs[k])),
            check::Ty::Enum(k) => Ty::Enum(EnumId(self.enums[k])),
            check::Ty::Option(t) => Ty::Option(inner(t)),
            check::Ty::Result(t, e) => Ty::Result(inner(t), inner(e)),
            check::Ty::List(t) => Ty::List(inner(t)),
            check::Ty::Map(k, v) => Ty::Map(inner(k), inner(v)),
            check::Ty::Cap(c) => Ty::Cap(CapId(self.caps[c])),
            check::Ty::Never => Ty::Never,
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

    fn func(&self, p: &Program, caps: &CapTable, k: &str) -> Func {
        let (f, sig) = (&p.funcs[k], &p.sigs[k]);
        let mut l = Lowerer {
            p,
            caps,
            ids: self,
            module: check::module_of(k),
            locals: Vec::new(),
            scope: Vec::new(),
        };
        for (param, ty) in f.params.iter().zip(&sig.params) {
            l.bind(&param.name, ty, param.span);
        }
        let body = l.block(&f.body);
        Func {
            name: k.to_string(),
            locals: l.locals,
            params: f.params.len() as u32,
            ret: self.ty(&sig.ret),
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
    locals: Vec<Local>,
    /// Names in scope, innermost last.
    scope: Vec<(&'a str, LocalId)>,
}

impl<'a> Lowerer<'a> {
    /// A new slot for `name`, which shadows any earlier one.
    fn bind(&mut self, name: &'a str, ty: &check::Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(Local {
            name: name.to_string(),
            ty: self.ids.held(ty),
            span,
        });
        self.scope.push((name, id));
        id
    }

    fn lookup(&self, name: &str) -> LocalId {
        let found = self.scope.iter().rev().find(|(n, _)| *n == name);
        found.expect("checked name").1
    }

    /// The checker's type of the expression or `let` at `span`.
    fn checked(&self, span: Span) -> &'a check::Ty {
        &self.p.types[&span]
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
                ir::StmtKind::Let(self.bind(name, self.checked(s.span), s.span), init)
            }
            StmtKind::Assign { target, op, value } => {
                let place = self.place(target);
                let op = match *op {
                    "=" => None,
                    "-=" => Some(BinOp::Sub),
                    _ if *self.checked(target.span) == check::Ty::Str => Some(BinOp::Concat),
                    _ => Some(BinOp::Add),
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
                        let x = self.bind(name, t, s.span);
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
        let check::Ty::Struct(s) = self.checked(base) else {
            unreachable!("checked field");
        };
        self.p.fields[s]
            .iter()
            .position(|(n, _)| n == name)
            .expect("checked field") as u32
    }

    /// The enum of a value of type `ty`, and the index of its variant `v`.
    fn variant(&self, ty: &check::Ty, v: &str) -> Option<(EnumRef, u32)> {
        let (i, _) = variant(&shape(&self.p.variants, ty).1, v)?;
        let en = match ty {
            check::Ty::Option(_) => EnumRef::Option,
            check::Ty::Result(..) => EnumRef::Result,
            check::Ty::Enum(k) => EnumRef::User(EnumId(self.ids.enums[k])),
            _ => unreachable!("only enums have variants"),
        };
        Some((en, i as u32))
    }

    fn exprs(&mut self, es: &'a [ast::Expr]) -> Vec<Expr> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    fn expr(&mut self, e: &'a ast::Expr) -> Expr {
        let ty = self.checked(e.span);
        let kind = match &e.kind {
            ExprKind::Int(n) => ir::ExprKind::Int(*n),
            ExprKind::Str(s) => ir::ExprKind::Str(s.as_str().into()),
            ExprKind::Bool(b) => ir::ExprKind::Bool(*b),
            ExprKind::Unit => ir::ExprKind::Unit,
            ExprKind::Name(name) if builtin(name).is_some() => self.variant_value(ty, name, &[]),
            ExprKind::Name(name) => ir::ExprKind::Local(self.lookup(name)),
            ExprKind::Unary(op, x) => {
                let op = if *op == "-" { UnOp::Neg } else { UnOp::Not };
                ir::ExprKind::Unary(op, Box::new(self.expr(x)))
            }
            ExprKind::Binary(op, a, b) => {
                let op = match *op {
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
            ExprKind::Module(_, inner) => match &inner.kind {
                ExprKind::StructLit(_, inits) => self.struct_lit(ty, inits),
                ExprKind::Path(_, v) => self.variant_value(ty, v, &[]),
                _ => unreachable!("checked module path"),
            },
            ExprKind::Match(scrut, arms) => self.match_expr(scrut, arms),
            ExprKind::IfLet(pat, scrut, then, els) => {
                let scrut = self.expr(scrut);
                let mark = self.scope.len();
                let pat = self.pat(pat, self.checked(scrut.span));
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
            ExprKind::Name(name) => key(self.module, name).into_owned(),
            ExprKind::Path(en, _) if en == "Map" => return ir::ExprKind::MapNew,
            ExprKind::Path(_, v) => return self.variant_value(ty, v, args),
            ExprKind::Module(m, inner) => match &inner.kind {
                ExprKind::Name(name) => format!("{m}::{name}"),
                ExprKind::Path(_, v) => return self.variant_value(ty, v, args),
                _ => unreachable!("checked callee"),
            },
            _ => unreachable!("checked callee"),
        };
        ir::ExprKind::Call(FuncId(self.ids.funcs[&k]), self.exprs(args))
    }

    /// `E::V(args)`, `Some(x)` and the like, where `ty` is the value's enum type.
    fn variant_value(&mut self, ty: &check::Ty, v: &str, args: &'a [ast::Expr]) -> ir::ExprKind {
        let (en, i) = self.variant(ty, v).expect("checked variant");
        ir::ExprKind::Variant(en, i, self.exprs(args))
    }

    /// Fields keep source order, which is evaluation order.
    fn struct_lit(&mut self, ty: &check::Ty, inits: &'a [(String, ast::Expr)]) -> ir::ExprKind {
        let check::Ty::Struct(k) = ty else {
            unreachable!("checked struct literal");
        };
        let fields = &self.p.fields[k];
        let inits = (inits.iter())
            .map(|(name, x)| {
                let field = fields.iter().position(|(n, _)| n == name).expect("checked field");
                (field as u32, self.expr(x))
            })
            .collect();
        ir::ExprKind::StructLit(StructId(self.ids.structs[k]), inits)
    }

    fn method(&mut self, recv: &'a ast::Expr, name: &str, args: &'a [ast::Expr]) -> ir::ExprKind {
        use check::Ty as T;
        let mutation = match (self.checked(recv.span), name) {
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
                let pat = self.pat(&arm.pat, st);
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
                let payload = &shape(&self.p.variants, st).1[i as usize].1;
                Pat::Variant(i, parts.iter().zip(payload).map(|(q, ty)| self.pat(q, ty)).collect())
            }
        }
    }
}
