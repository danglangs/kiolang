use crate::ast::*;
use crate::lex::{lex, Kind, Token};
use crate::{Diag, Span};

const MAX_DEPTH: usize = 64;

/// Binding power of a binary operator; 0 for anything else.
fn prec(op: &str) -> u8 {
    match op {
        "||" => 1,
        "&&" => 2,
        "==" | "!=" | "<" | "<=" | ">" | ">=" => 3,
        "+" | "-" => 4,
        "*" | "/" | "%" => 5,
        _ => 0,
    }
}
const COMPARE: u8 = 3;

fn err(span: Span, msg: &str, hint: Option<&str>) -> Diag {
    Diag {
        span,
        msg: msg.to_string(),
        hint: hint.map(str::to_string),
    }
}

/// Bound `b`, written on type parameter `t` at `span`, is neither `Eq` nor `Ord`.
fn unknown_bound(span: Span, t: &str, b: &str) -> Diag {
    let hint = match b {
        "PartialEq" => format!("write `{t}: Eq`"),
        "PartialOrd" => format!("write `{t}: Ord`"),
        "Clone" | "Copy" | "Debug" | "Display" => {
            format!("remove `{b}`: kio values are always copied and displayable")
        }
        _ => format!("write `{t}: Eq` or `{t}: Ord`, or remove the bound"),
    };
    err(span, "only `Eq` and `Ord` bounds exist", Some(&hint))
}

/// `t` as written, for hints.
fn type_text(t: &Type) -> String {
    let list = |ts: &[Type]| ts.iter().map(type_text).collect::<Vec<_>>().join(", ");
    match &t.kind {
        TypeKind::Unit => "()".to_string(),
        TypeKind::Named(n, args) if args.is_empty() => n.clone(),
        TypeKind::Named(n, args) => format!("{n}<{}>", list(args)),
        TypeKind::Module(m, t) => format!("{m}::{}", type_text(t)),
        TypeKind::Fn(ps, r) if r.kind == TypeKind::Unit => format!("fn({})", list(ps)),
        TypeKind::Fn(ps, r) => format!("fn({}) -> {}", list(ps), type_text(r)),
    }
}

fn reserved_hint(word: &str) -> &'static str {
    match word {
        "impl" | "trait" | "dyn" => "kio has no traits or `impl`; use free functions",
        "use" | "crate" | "super" | "self" | "Self" => "write the full path, like `web::text`",
        "static" => "kio has no globals; use `const` for a fixed value, or pass values as arguments",
        "where" => "write the bound on the parameter: `<T: Ord>`",
        "move" => "closures always capture by copy; remove `move`",
        _ => "this Rust keyword is reserved in kio",
    }
}

/// A hint for the expected tokens whose fix is clear.
fn expected_hint(expected: &str) -> Option<&'static str> {
    match expected {
        "`;`" | "`;` or `}`" => Some("add `;`"),
        "`}`" => Some("a `{` is not closed"),
        "`)`" => Some("a `(` is not closed"),
        _ => None,
    }
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// Tree depth so far, capped at `MAX_DEPTH`.
    depth: usize,
    /// Set in `if`, `while` and `for` heads, where `{` starts the body, not a struct literal.
    no_struct: bool,
    /// Index of the source file, stamped on every span.
    file: u32,
}

pub fn parse(src: &str) -> Result<Vec<Item>, Diag> {
    parse_file(src, 0)
}

/// Like `parse`, for source file number `file`.
pub fn parse_file(src: &str, file: u32) -> Result<Vec<Item>, Diag> {
    let at = |mut s: Span| {
        s.file = file;
        s
    };
    let mut toks = lex(src).map_err(|mut d| {
        d.span = at(d.span);
        d
    })?;
    toks.iter_mut().for_each(|t| t.span = at(t.span));
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
        no_struct: false,
        file,
    };
    let mut items = Vec::new();
    while p.pos < p.toks.len() {
        items.push(p.item()?);
    }
    Ok(items)
}

impl Parser {
    fn peek(&self) -> Option<&Kind> {
        self.toks.get(self.pos).map(|t| &t.kind)
    }

    fn at(&self, sym: &str) -> bool {
        matches!(self.peek(), Some(Kind::Sym(s)) if *s == sym)
    }

    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Kind::Keyword(k)) if k == kw)
    }

    fn eat(&mut self, sym: &str) -> bool {
        let hit = self.at(sym);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        let hit = self.at_kw(kw);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn expect(&mut self, sym: &str) -> Result<(), Diag> {
        if self.eat(sym) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("`{sym}`")))
        }
    }

    fn ident(&mut self) -> Result<String, Diag> {
        let Some(Kind::Ident(name)) = self.peek() else {
            return Err(self.unexpected("a name"));
        };
        let name = name.clone();
        self.pos += 1;
        Ok(name)
    }

    /// Consumes a leading `web::`. A lowercase first segment is a module; enums and structs are capitalized.
    fn module(&mut self) -> Result<Option<String>, Diag> {
        let Some(Kind::Ident(name)) = self.peek() else {
            return Ok(None);
        };
        let lower = name.starts_with(|c: char| c.is_lowercase());
        let at = |i: usize, sym| self.toks.get(self.pos + i).is_some_and(|t| t.kind == Kind::Sym(sym));
        // `first::<Int>` is a function with type arguments, not a module.
        if !lower || !at(1, "::") || at(2, "<") {
            return Ok(None);
        }
        let name = self.ident()?;
        self.pos += 1;
        if !matches!(self.peek(), Some(Kind::Ident(_))) {
            return Err(self.unexpected("a name"));
        }
        Ok(Some(name))
    }

    /// The current token, or an empty span at the end of the input.
    fn span(&self) -> Span {
        match self.toks.get(self.pos) {
            Some(t) => t.span,
            None => {
                let end = self.toks.last().map_or(0, |t| t.span.end);
                Span {
                    start: end,
                    end,
                    file: self.file,
                }
            }
        }
    }

    fn prev_end(&self) -> usize {
        self.toks[self.pos - 1].span.end
    }

    /// Spans from `start` to the end of the last consumed token.
    fn since(&self, start: usize) -> Span {
        Span {
            start,
            end: self.prev_end(),
            file: self.file,
        }
    }

    fn enter(&mut self) -> Result<(), Diag> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(err(
                self.span(),
                "nesting is too deep",
                Some("the limit is 64; split the code into functions"),
            ));
        }
        Ok(())
    }

    /// The error for a token that cannot start or continue the current rule.
    fn unexpected(&self, expected: &str) -> Diag {
        let span = self.span();
        match self.peek() {
            Some(Kind::Reserved(w)) => err(span, &format!("`{w}` is not supported in kio"), Some(reserved_hint(w))),
            Some(Kind::Sym("..")) => err(span, "unexpected `..`", Some("ranges are only for `for` loops")),
            Some(Kind::Keyword(k)) if k == "const" => err(
                span,
                "a constant is declared at the top level of a file",
                Some("move it out of the function, or use `let`"),
            ),
            _ => err(span, &format!("expected {expected}"), expected_hint(expected)),
        }
    }

    /// Items up to the closing symbol, separated by commas, allowing a trailing comma.
    fn list<T>(&mut self, close: &str, item: fn(&mut Self) -> Result<T, Diag>) -> Result<Vec<T>, Diag> {
        let outer = std::mem::replace(&mut self.no_struct, false);
        let mut out = Vec::new();
        while !self.at(close) {
            out.push(item(self)?);
            if !self.eat(",") {
                break;
            }
        }
        self.expect(close)?;
        self.no_struct = outer;
        Ok(out)
    }

    fn item(&mut self) -> Result<Item, Diag> {
        let start = self.span().start;
        if self.eat_kw("mod") {
            let name = self.ident()?;
            self.expect(";")?;
            return Ok(Item::Mod(name, self.since(start)));
        }
        let public = self.eat_kw("pub");
        if self.eat_kw("const") {
            let name = self.ident()?;
            if !self.eat(":") {
                let hint = format!("write the type: `const {name}: Int = ..`");
                return Err(err(self.span(), "a constant needs a type", Some(&hint)));
            }
            let ty = self.ty()?;
            self.expect("=")?;
            let value = self.expr()?;
            self.expect(";")?;
            return Ok(Item::Const(Const {
                public,
                name,
                ty,
                value,
                span: self.since(start),
            }));
        }
        if self.eat_kw("struct") {
            let name = self.ident()?;
            let tparams = self.tparams(&format!("struct {name} {{ .. }}"))?;
            self.expect("{")?;
            let fields = self.list("}", Self::param)?;
            return Ok(Item::Struct(Struct {
                public,
                name,
                tparams,
                fields,
                span: self.since(start),
            }));
        }
        if self.eat_kw("enum") {
            let name = self.ident()?;
            let tparams = self.tparams(&format!("enum {name} {{ .. }}"))?;
            self.expect("{")?;
            let variants = self.list("}", Self::variant)?;
            return Ok(Item::Enum(Enum {
                public,
                name,
                tparams,
                variants,
                span: self.since(start),
            }));
        }
        if !self.eat_kw("fn") {
            return Err(self.unexpected("`fn`"));
        }
        let name = self.ident()?;
        let tparams = self.tparams(&format!("fn {name}(..)"))?;
        self.expect("(")?;
        let params = self.list(")", Self::param)?;
        let ret = self.ret()?;
        let body = self.block()?;
        Ok(Item::Fn(Fn {
            public,
            name,
            tparams,
            params,
            ret,
            body,
            span: self.since(start),
        }))
    }

    /// `<T, U>` after an item's name; `item` is how it is written without them.
    fn tparams(&mut self, item: &str) -> Result<Vec<TParam>, Diag> {
        if !self.eat("<") {
            return Ok(Vec::new());
        }
        if self.at(">") {
            let hint = format!("write `{item}` without `<>`");
            return Err(err(self.span(), "empty type parameters", Some(&hint)));
        }
        self.list(">", Self::tparam)
    }

    /// A type parameter `T`, with bounds `T: Eq`, `T: Ord` or `T: Eq + Ord`; its span is the name's.
    fn tparam(&mut self) -> Result<TParam, Diag> {
        let start = self.span().start;
        let name = self.ident()?;
        let span = self.since(start);
        let mut bound = None;
        if self.eat(":") {
            loop {
                let at = self.span();
                let b = match self.ident()?.as_str() {
                    "Eq" => TBound::Eq,
                    "Ord" => TBound::Ord,
                    other => return Err(unknown_bound(at, &name, other)),
                };
                bound = bound.max(Some(b));
                if !self.eat("+") {
                    break;
                }
            }
        }
        Ok(TParam { name, bound, span })
    }

    fn variant(&mut self) -> Result<Variant, Diag> {
        let start = self.span().start;
        let name = self.ident()?;
        let fields = if self.eat("(") {
            self.list(")", Self::ty)?
        } else {
            Vec::new()
        };
        if self.at("{") {
            let hint = "use `A(T)` or a struct payload";
            return Err(err(self.span(), "struct-like variants are not supported", Some(hint)));
        }
        Ok(Variant {
            name,
            fields,
            span: self.since(start),
        })
    }

    fn param(&mut self) -> Result<Param, Diag> {
        let start = self.span().start;
        let name = self.ident()?;
        self.expect(":")?;
        let ty = self.ty()?;
        Ok(Param {
            name,
            ty,
            span: self.since(start),
        })
    }

    /// `-> T`, or `()` at the end of the last token when there is no `->`.
    fn ret(&mut self) -> Result<Type, Diag> {
        if self.eat("->") {
            return self.ty();
        }
        let end = self.prev_end();
        Ok(Type {
            kind: TypeKind::Unit,
            span: Span {
                start: end,
                end,
                file: self.file,
            },
        })
    }

    fn ty(&mut self) -> Result<Type, Diag> {
        let depth = self.depth;
        self.enter()?;
        let start = self.span().start;
        let kind = if let Some(module) = self.module()? {
            TypeKind::Module(module, Box::new(self.ty()?))
        } else if self.eat_kw("fn") {
            self.expect("(")?;
            let params = self.list(")", Self::ty)?;
            TypeKind::Fn(params, Box::new(self.ret()?))
        } else if self.eat("(") {
            if !self.at(")") {
                return Err(err(self.span(), "kio has no tuple types", Some("use a struct")));
            }
            self.pos += 1;
            TypeKind::Unit
        } else {
            let name = self.ident()?;
            let args = if self.eat("<") {
                if self.at(">") {
                    return Err(err(
                        self.span(),
                        "empty type arguments",
                        Some(&format!("write `{name}` without `<>`")),
                    ));
                }
                self.list(">", Self::ty)?
            } else {
                Vec::new()
            };
            TypeKind::Named(name, args)
        };
        self.depth = depth;
        Ok(Type {
            kind,
            span: self.since(start),
        })
    }

    fn block(&mut self) -> Result<Block, Diag> {
        let depth = self.depth;
        self.enter()?;
        let outer = std::mem::replace(&mut self.no_struct, false);
        let start = self.span().start;
        self.expect("{")?;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.eat("}") {
            if self.peek().is_none() {
                return Err(self.unexpected("`}`"));
            }
            let start = self.span().start;
            let kind = if self.at_kw("let") {
                self.let_stmt()?
            } else if self.at_kw("return") {
                let k = self.return_stmt()?;
                self.expect(";")?;
                k
            } else if self.at_kw("while") {
                self.while_stmt()?
            } else if self.at_kw("loop") {
                self.loop_stmt()?
            } else if self.at_kw("for") {
                self.for_stmt()?
            } else if self.at_kw("break") || self.at_kw("continue") {
                let k = self.jump_stmt()?;
                self.expect(";")?;
                k
            } else {
                let block_like = self.at_kw("if") || self.at_kw("match") || self.at("{");
                let e = if block_like { self.primary()? } else { self.expr()? };
                if let Some(&Kind::Sym(op @ ("=" | "+=" | "-="))) = self.peek() {
                    if !matches!(e.kind, ExprKind::Name(_) | ExprKind::Field(..) | ExprKind::Index(..)) {
                        let hint = "assign to a name, field or index";
                        return Err(err(e.span, "cannot assign to this expression", Some(hint)));
                    }
                    self.pos += 1;
                    let value = self.expr()?;
                    self.expect(";")?;
                    StmtKind::Assign { target: e, op, value }
                } else if self.eat(";") {
                    StmtKind::Expr(e)
                } else if self.at("}") {
                    tail = Some(Box::new(e));
                    continue;
                } else if block_like {
                    StmtKind::Expr(e)
                } else {
                    return Err(self.unexpected("`;` or `}`"));
                }
            };
            stmts.push(Stmt {
                kind,
                span: self.since(start),
            });
        }
        self.depth = depth;
        self.no_struct = outer;
        Ok(Block {
            stmts,
            tail,
            span: self.since(start),
        })
    }

    fn let_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.pos += 1;
        let mutable = self.eat_kw("mut");
        let name_start = self.span().start;
        let simple = matches!(self.peek(), Some(Kind::Ident(_)))
            && !matches!(
                self.toks.get(self.pos + 1).map(|t| &t.kind),
                Some(Kind::Sym("(" | "::" | "|"))
            );
        if !simple && !mutable {
            // `let Some(x) = e else { .. };`
            let pat = self.pat()?;
            let ty = if self.eat(":") { Some(self.ty()?) } else { None };
            self.expect("=")?;
            let init = self.expr()?;
            if !self.eat_kw("else") {
                let hint = "add `else { return; }` after the value, or use `match`";
                return Err(err(pat.span, "a pattern in `let` needs an `else` block", Some(hint)));
            }
            let els = self.block()?;
            self.expect(";")?;
            return Ok(StmtKind::LetElse { pat, ty, init, els });
        }
        let name = self.ident()?;
        let name_span = self.since(name_start);
        let ty = if self.eat(":") { Some(self.ty()?) } else { None };
        self.expect("=")?;
        let init = self.expr()?;
        if self.eat_kw("else") {
            if mutable {
                // `let mut x` binds whatever the value is, as in Rust.
                return Err(err(name_span, "this pattern always matches", Some("write `let`")));
            }
            let els = self.block()?;
            self.expect(";")?;
            let pat = Pat {
                kind: PatKind::Name(name),
                span: name_span,
            };
            return Ok(StmtKind::LetElse { pat, ty, init, els });
        }
        self.expect(";")?;
        Ok(StmtKind::Let {
            name,
            mutable,
            ty,
            init,
        })
    }

    /// `return` and an optional value, up to its `;`, or `,` or `}` as a match arm.
    fn return_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.pos += 1;
        let end = self.at(";") || self.at(",") || self.at("}");
        let value = if end { None } else { Some(self.expr()?) };
        Ok(StmtKind::Return(value))
    }

    fn while_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.pos += 1;
        let cond = self.cond()?;
        let body = self.block()?;
        Ok(StmtKind::While { cond, body })
    }

    fn loop_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.pos += 1;
        Ok(StmtKind::Loop { body: self.block()? })
    }

    fn for_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.pos += 1;
        if self.at("(") {
            let start = self.span().start;
            while !self.at(")") && self.peek().is_some() {
                self.pos += 1;
            }
            self.pos += 1;
            let hint = "loop over `m.keys()` and `get`";
            return Err(err(self.since(start), "kio has no tuples", Some(hint)));
        }
        let name = self.ident()?;
        if !self.eat_kw("in") {
            return Err(self.unexpected("`in`"));
        }
        let outer = std::mem::replace(&mut self.no_struct, true);
        let first = self.expr()?;
        let iter = if self.eat("..") {
            Iter::Range(first, self.expr()?)
        } else {
            Iter::List(first)
        };
        self.no_struct = outer;
        let body = self.block()?;
        Ok(StmtKind::For { name, iter, body })
    }

    fn jump_stmt(&mut self) -> Result<StmtKind, Diag> {
        let kind = if self.at_kw("break") {
            StmtKind::Break
        } else {
            StmtKind::Continue
        };
        self.pos += 1;
        let end = self.at(";") || self.at(",") || self.at("}") || self.peek().is_none();
        if matches!(kind, StmtKind::Break) && !end {
            let span = self.span();
            return Err(err(span, "`break` with a value", Some("use a `let mut` and `break`")));
        }
        Ok(kind)
    }

    fn cond(&mut self) -> Result<Expr, Diag> {
        let outer = std::mem::replace(&mut self.no_struct, true);
        let e = self.expr()?;
        self.no_struct = outer;
        Ok(e)
    }

    fn expr(&mut self) -> Result<Expr, Diag> {
        let depth = self.depth;
        self.enter()?;
        let e = self.binary(1)?;
        self.depth = depth;
        Ok(e)
    }

    /// Precedence climbing: parse operators binding at least as tightly as `min`.
    fn binary(&mut self, min: u8) -> Result<Expr, Diag> {
        let depth = self.depth;
        let mut lhs = self.unary()?;
        while let Some(&Kind::Sym(op)) = self.peek() {
            if op == "|" {
                let hint = "use `||` for logical or; `|x| ..` starts a closure only where a value starts";
                return Err(err(self.span(), "`|` is not an operator in kio", Some(hint)));
            }
            let p = prec(op);
            if p < min || p == 0 {
                break;
            }
            // Each left-nested operator deepens the tree.
            self.enter()?;
            self.pos += 1;
            let rhs = self.binary(p + 1)?;
            lhs = Expr {
                span: Span {
                    start: lhs.span.start,
                    end: rhs.span.end,
                    file: self.file,
                },
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
            };
            if p == COMPARE && matches!(self.peek(), Some(&Kind::Sym(next)) if prec(next) == COMPARE) {
                return Err(err(
                    self.span(),
                    "comparisons cannot be chained",
                    Some("join them with `&&`: `a < b && b < c`"),
                ));
            }
        }
        self.depth = depth;
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, Diag> {
        let Some(&Kind::Sym(op @ ("-" | "!"))) = self.peek() else {
            return self.postfix();
        };
        let depth = self.depth;
        self.enter()?;
        let start = self.span().start;
        self.pos += 1;
        let operand = self.unary()?;
        self.depth = depth;
        Ok(Expr {
            span: Span {
                start,
                end: operand.span.end,
                file: self.file,
            },
            kind: ExprKind::Unary(op, Box::new(operand)),
        })
    }

    fn postfix(&mut self) -> Result<Expr, Diag> {
        let depth = self.depth;
        let mut e = self.primary()?;
        loop {
            let start = e.span.start;
            let kind = if self.eat("(") {
                ExprKind::Call(Box::new(e), self.list(")", Self::expr)?)
            } else if self.eat(".") {
                let name = self.ident()?;
                if self.eat("(") {
                    ExprKind::Method(Box::new(e), name, self.list(")", Self::expr)?)
                } else {
                    ExprKind::Field(Box::new(e), name)
                }
            } else if self.eat("[") {
                let outer = std::mem::replace(&mut self.no_struct, false);
                let index = self.expr()?;
                self.expect("]")?;
                self.no_struct = outer;
                ExprKind::Index(Box::new(e), Box::new(index))
            } else if self.eat("?") {
                ExprKind::Try(Box::new(e))
            } else {
                break;
            };
            self.enter()?;
            e = Expr {
                kind,
                span: self.since(start),
            };
        }
        self.depth = depth;
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, Diag> {
        let span = self.span();
        if let Some(module) = self.module()? {
            let inner = self.primary()?;
            return Ok(Expr {
                kind: ExprKind::Module(module, Box::new(inner)),
                span: self.since(span.start),
            });
        }
        let kind = match self.peek() {
            Some(Kind::Ident(name))
                if self.toks.get(self.pos + 1).is_some_and(|t| t.kind == Kind::Sym("!"))
                    && self
                        .toks
                        .get(self.pos + 2)
                        .is_some_and(|t| matches!(t.kind, Kind::Sym("(" | "[" | "{"))) =>
            {
                let hint = match name.as_str() {
                    "format" => "build text with `+` and `to_str()`",
                    "matches" => "use `match` or `if let`",
                    "vec" => "write a list literal: `[..]`",
                    "println" | "print" | "eprintln" | "dbg" => "return the value from `main`",
                    _ => "kio has no macros; write a function call",
                };
                self.pos += 2;
                return Err(err(self.since(span.start), "macros are not supported", Some(hint)));
            }
            Some(Kind::Int(n)) => ExprKind::Int(*n),
            Some(Kind::Float(x)) => ExprKind::Float(*x),
            Some(Kind::Str(s)) => ExprKind::Str(s.clone()),
            Some(Kind::Ident(_))
                if !self.no_struct && self.toks.get(self.pos + 1).is_some_and(|t| t.kind == Kind::Sym("{")) =>
            {
                return self.struct_lit();
            }
            Some(Kind::Ident(_)) if self.toks.get(self.pos + 1).is_some_and(|t| t.kind == Kind::Sym("::")) => {
                let start = span.start;
                let enum_name = self.ident()?;
                self.pos += 1;
                if self.eat("<") {
                    if self.at(">") {
                        let hint = format!("write `{enum_name}` without `::<>`");
                        return Err(err(self.span(), "empty type arguments", Some(&hint)));
                    }
                    let tys = self.list(">", Self::ty)?;
                    // `Box::<Int> { .. }`, `Tree::<Int>::Leaf`: type arguments go on the `let`.
                    let what = match self.peek() {
                        _ if !enum_name.starts_with(|c: char| c.is_uppercase()) => None,
                        Some(Kind::Sym("{")) if !self.no_struct => Some("a struct literal"),
                        Some(Kind::Sym("::")) => Some("a variant"),
                        _ => None,
                    };
                    if let Some(what) = what {
                        let args: Vec<String> = tys.iter().map(type_text).collect();
                        let local = enum_name.to_lowercase().chars().next().unwrap_or('x');
                        let hint = format!(
                            "write the type on the `let`: `let {local}: {enum_name}<{}> = ..`",
                            args.join(", ")
                        );
                        let msg = format!("{what} takes no type arguments");
                        return Err(err(self.since(start), &msg, Some(&hint)));
                    }
                    return Ok(Expr {
                        kind: ExprKind::Turbofish(enum_name, tys),
                        span: self.since(start),
                    });
                }
                let variant = self.ident()?;
                return Ok(Expr {
                    kind: ExprKind::Path(enum_name, variant),
                    span: self.since(start),
                });
            }
            Some(Kind::Ident(name)) => ExprKind::Name(name.clone()),
            Some(Kind::Keyword(k)) if k == "true" => ExprKind::Bool(true),
            Some(Kind::Keyword(k)) if k == "false" => ExprKind::Bool(false),
            Some(Kind::Keyword(k)) if k == "if" => return self.if_expr(),
            Some(Kind::Keyword(k)) if k == "match" => return self.match_expr(),
            Some(Kind::Sym("(")) => return self.paren(),
            Some(Kind::Sym("[")) => return self.list_lit(),
            Some(Kind::Sym("|" | "||")) => return self.closure(),
            Some(Kind::Sym("{")) => {
                let b = self.block()?;
                return Ok(Expr {
                    span: b.span,
                    kind: ExprKind::Block(b),
                });
            }
            _ => return Err(self.unexpected("an expression")),
        };
        self.pos += 1;
        Ok(Expr { kind, span })
    }

    fn struct_lit(&mut self) -> Result<Expr, Diag> {
        let start = self.span().start;
        let name = self.ident()?;
        self.pos += 1;
        let outer = std::mem::replace(&mut self.no_struct, false);
        let (mut fields, mut base) = (Vec::new(), None);
        while !self.at("}") {
            if self.at("..") {
                let from = self.span().start;
                self.pos += 1;
                base = Some(Box::new(self.expr()?));
                if self.eat(",") {
                    let hint = match self.at("}") {
                        true => "the base struct must be last: remove this comma",
                        false => "the base struct must be last: move `..base` after the fields",
                    };
                    return Err(err(
                        self.since(from),
                        "cannot use a comma after the base struct",
                        Some(hint),
                    ));
                }
                break;
            }
            fields.push(self.field_init()?);
            if !self.eat(",") {
                break;
            }
        }
        self.expect("}")?;
        self.no_struct = outer;
        Ok(Expr {
            kind: ExprKind::StructLit(name, fields, base),
            span: self.since(start),
        })
    }

    /// `|a: Int, b| body` or `|| body`; the body extends as far as an expression can.
    fn closure(&mut self) -> Result<Expr, Diag> {
        let depth = self.depth;
        self.enter()?;
        let start = self.span().start;
        let params = if self.eat("||") {
            Vec::new()
        } else {
            self.pos += 1;
            self.list("|", Self::closure_param)?
        };
        if self.at("->") {
            let hint = "remove `-> T`: a closure returns its body's type";
            return Err(err(self.span(), "a closure has no return type annotation", Some(hint)));
        }
        let body = self.expr()?;
        self.depth = depth;
        Ok(Expr {
            kind: ExprKind::Closure(params, Box::new(body)),
            span: self.since(start),
        })
    }

    fn closure_param(&mut self) -> Result<ClosureParam, Diag> {
        let start = self.span().start;
        let name = self.ident()?;
        let ty = if self.eat(":") { Some(self.ty()?) } else { None };
        Ok(ClosureParam {
            name,
            ty,
            span: self.since(start),
        })
    }

    fn field_init(&mut self) -> Result<(String, Expr), Diag> {
        let span = self.span();
        let name = self.ident()?;
        if !self.at(":") {
            return Err(err(
                span,
                "field shorthand is not supported",
                Some(&format!("write `{name}: {name}`")),
            ));
        }
        self.pos += 1;
        Ok((name, self.expr()?))
    }

    fn list_lit(&mut self) -> Result<Expr, Diag> {
        let start = self.span().start;
        self.pos += 1;
        let items = self.list("]", Self::expr)?;
        Ok(Expr {
            kind: ExprKind::List(items),
            span: self.since(start),
        })
    }

    /// `()` or `( e )`; the parentheses leave no node.
    fn paren(&mut self) -> Result<Expr, Diag> {
        let start = self.span().start;
        self.pos += 1;
        if self.eat(")") {
            return Ok(Expr {
                kind: ExprKind::Unit,
                span: self.since(start),
            });
        }
        let outer = std::mem::replace(&mut self.no_struct, false);
        let inner = self.expr()?;
        self.no_struct = outer;
        if self.at(",") {
            return Err(err(self.span(), "kio has no tuples", Some("use a struct")));
        }
        self.expect(")")?;
        Ok(inner)
    }

    fn if_expr(&mut self) -> Result<Expr, Diag> {
        let depth = self.depth;
        self.enter()?;
        let start = self.span().start;
        self.pos += 1;
        let pat = if self.eat_kw("let") {
            let pat = self.pat()?;
            self.expect("=")?;
            Some(pat)
        } else {
            None
        };
        let cond = self.cond()?;
        if let (Some(pat), ExprKind::Binary("&&", ..)) = (&pat, &cond.kind) {
            // Only a Bool pattern can match `a && b`; anything else is a let-chain.
            if !matches!(pat.kind, PatKind::Bool(_) | PatKind::Name(_) | PatKind::Wild) {
                return Err(err(cond.span, "let-chains are not supported", Some("nest an `if`")));
            }
        }
        let then = self.block()?;
        let els = if !self.eat_kw("else") {
            None
        } else if self.at_kw("if") {
            Some(Box::new(self.if_expr()?))
        } else {
            let b = self.block()?;
            Some(Box::new(Expr {
                span: b.span,
                kind: ExprKind::Block(b),
            }))
        };
        self.depth = depth;
        let kind = match pat {
            Some(pat) => ExprKind::IfLet(Box::new(pat), Box::new(cond), then, els),
            None => ExprKind::If(Box::new(cond), then, els),
        };
        Ok(Expr {
            kind,
            span: self.since(start),
        })
    }

    fn match_expr(&mut self) -> Result<Expr, Diag> {
        let depth = self.depth;
        self.enter()?;
        let start = self.span().start;
        self.pos += 1;
        let scrutinee = self.cond()?;
        self.expect("{")?;
        let outer = std::mem::replace(&mut self.no_struct, false);
        let mut arms = Vec::new();
        while !self.at("}") {
            let arm_start = self.span().start;
            let pat = self.pat()?;
            if self.at_kw("if") {
                let hint = "test the condition inside the arm with `if`";
                return Err(err(self.span(), "match guards are not supported", Some(hint)));
            }
            self.expect("=>")?;
            let block_body = self.at("{");
            let body = if block_body {
                self.primary()?
            } else if self.at_kw("return") || self.at_kw("break") || self.at_kw("continue") {
                self.arm_jump()?
            } else {
                let e = self.expr()?;
                if let Some(&Kind::Sym("=" | "+=" | "-=")) = self.peek() {
                    self.pos += 1;
                    self.expr()?;
                    let hint = "wrap it in `{ }`";
                    return Err(err(
                        self.since(e.span.start),
                        "an assignment cannot be a match arm",
                        Some(hint),
                    ));
                }
                e
            };
            arms.push(Arm {
                pat,
                body,
                span: self.since(arm_start),
            });
            if !self.eat(",") && !block_body {
                break;
            }
        }
        self.expect("}")?;
        self.no_struct = outer;
        self.depth = depth;
        Ok(Expr {
            kind: ExprKind::Match(Box::new(scrutinee), arms),
            span: self.since(start),
        })
    }

    /// `return`, `break` or `continue` as an arm body, ended by `,` or `}`: the block `{ stmt; }`.
    fn arm_jump(&mut self) -> Result<Expr, Diag> {
        let start = self.span().start;
        let kind = if self.at_kw("return") {
            self.return_stmt()?
        } else {
            self.jump_stmt()?
        };
        let span = self.since(start);
        Ok(Expr {
            kind: ExprKind::Block(Block {
                stmts: vec![Stmt { kind, span }],
                tail: None,
                span,
            }),
            span,
        })
    }

    /// A pattern: one alternative, or several `P1 | P2 | ..`, with an optional leading `|`, as in Rust.
    fn pat(&mut self) -> Result<Pat, Diag> {
        let start = self.span().start;
        self.eat("|");
        let first = self.alt()?;
        if !self.at("|") {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat("|") {
            alts.push(self.alt()?);
        }
        Ok(Pat {
            kind: PatKind::Or(alts),
            span: self.since(start),
        })
    }

    /// One alternative of a pattern.
    fn alt(&mut self) -> Result<Pat, Diag> {
        let depth = self.depth;
        self.enter()?;
        let p = self.pat_inner()?;
        self.depth = depth;
        Ok(p)
    }

    fn pat_inner(&mut self) -> Result<Pat, Diag> {
        let start = self.span().start;
        if let Some(module) = self.module()? {
            let inner = self.alt()?;
            return Ok(Pat {
                kind: PatKind::Module(module, Box::new(inner)),
                span: self.since(start),
            });
        }
        let neg = self.eat("-");
        let kind = match self.peek() {
            Some(Kind::Int(n)) => {
                let n = if neg { -*n } else { *n };
                self.pos += 1;
                PatKind::Int(n)
            }
            Some(Kind::Float(_)) => {
                let hint = Some("compare with `<` or `==`");
                return Err(err(self.span(), "a `Float` cannot be a pattern", hint));
            }
            _ if neg => return Err(self.unexpected("an integer")),
            Some(Kind::Str(s)) => {
                let s = s.clone();
                self.pos += 1;
                PatKind::Str(s)
            }
            Some(Kind::Keyword(k)) if k == "true" || k == "false" => {
                let b = k == "true";
                self.pos += 1;
                PatKind::Bool(b)
            }
            Some(Kind::Ident(_)) => {
                let name = self.ident()?;
                if self.eat("::") {
                    let variant = self.ident()?;
                    PatKind::Variant(Some(name), variant, self.payload()?)
                } else if self.at("(") {
                    PatKind::Variant(None, name, self.payload()?)
                } else if name == "_" {
                    PatKind::Wild
                } else {
                    PatKind::Name(name)
                }
            }
            Some(Kind::Sym("..")) => return Err(self.dotdot()),
            _ => return Err(self.unexpected("a pattern")),
        };
        Ok(Pat {
            kind,
            span: self.since(start),
        })
    }

    /// The optional `(a, _)` after a variant pattern.
    fn payload(&mut self) -> Result<Vec<Pat>, Diag> {
        if !self.eat("(") {
            return Ok(Vec::new());
        }
        self.list(")", Self::pat)
    }

    fn dotdot(&self) -> Diag {
        let hint = "list every payload field, using `_` to ignore one";
        err(self.span(), "`..` is not supported in patterns", Some(hint))
    }
}
