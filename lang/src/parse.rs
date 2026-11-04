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

fn reserved_hint(word: &str) -> &'static str {
    match word {
        "impl" | "trait" | "dyn" => "kio has no traits or `impl`; use free functions",
        "use" | "mod" | "pub" | "crate" | "super" | "self" | "Self" => {
            "kio is one file of functions: no modules or visibility"
        }
        "const" | "static" => "kio has no globals; pass values as arguments",
        _ => "this Rust keyword is reserved in kio",
    }
}

/// Keywords that kio will support in a later task.
fn later_hint(word: &str) -> Option<&'static str> {
    match word {
        "for" => Some("use `while` for now"),
        "break" | "continue" => Some("use `return` or a `while` condition for now"),
        _ => None,
    }
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// Tree depth so far, capped at `MAX_DEPTH`.
    depth: usize,
    /// Set in `if` and `while` conditions, where `{` starts the body, not a struct literal.
    no_struct: bool,
}

pub fn parse(src: &str) -> Result<Vec<Item>, Diag> {
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
        depth: 0,
        no_struct: false,
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

    /// The current token, or an empty span at the end of the input.
    fn span(&self) -> Span {
        match self.toks.get(self.pos) {
            Some(t) => t.span,
            None => {
                let end = self.toks.last().map_or(0, |t| t.span.end);
                Span { start: end, end }
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
        let rejected = match self.peek() {
            Some(Kind::Reserved(w)) => Some((w, reserved_hint(w))),
            Some(Kind::Keyword(w)) => later_hint(w).map(|h| (w, h)),
            _ => None,
        };
        match rejected {
            Some((w, hint)) => err(span, &format!("`{w}` is not supported in kio"), Some(hint)),
            None => err(span, &format!("expected {expected}"), None),
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
        if self.eat_kw("struct") {
            let name = self.ident()?;
            self.expect("{")?;
            let fields = self.list("}", Self::param)?;
            return Ok(Item::Struct(Struct {
                name,
                fields,
                span: self.since(start),
            }));
        }
        if self.eat_kw("enum") {
            let name = self.ident()?;
            self.expect("{")?;
            let variants = self.list("}", Self::variant)?;
            return Ok(Item::Enum(Enum {
                name,
                variants,
                span: self.since(start),
            }));
        }
        if !self.eat_kw("fn") {
            return Err(self.unexpected("`fn`"));
        }
        let name = self.ident()?;
        self.expect("(")?;
        let params = self.list(")", Self::param)?;
        let ret = if self.eat("->") {
            self.ty()?
        } else {
            let end = self.prev_end();
            Type {
                kind: TypeKind::Unit,
                span: Span { start: end, end },
            }
        };
        let body = self.block()?;
        Ok(Item::Fn(Fn {
            name,
            params,
            ret,
            body,
            span: self.since(start),
        }))
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

    fn ty(&mut self) -> Result<Type, Diag> {
        let depth = self.depth;
        self.enter()?;
        let start = self.span().start;
        let kind = if self.eat("(") {
            if !self.at(")") {
                return Err(err(self.span(), "kio has no tuple types", Some("use a struct")));
            }
            self.pos += 1;
            TypeKind::Unit
        } else {
            let name = self.ident()?;
            let args = if self.eat("<") {
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
                self.return_stmt()?
            } else if self.at_kw("while") {
                self.while_stmt()?
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
        let name = self.ident()?;
        let ty = if self.eat(":") { Some(self.ty()?) } else { None };
        self.expect("=")?;
        let init = self.expr()?;
        self.expect(";")?;
        Ok(StmtKind::Let {
            name,
            mutable,
            ty,
            init,
        })
    }

    fn return_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.pos += 1;
        let value = if self.at(";") { None } else { Some(self.expr()?) };
        self.expect(";")?;
        Ok(StmtKind::Return(value))
    }

    fn while_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.pos += 1;
        let cond = self.cond()?;
        let body = self.block()?;
        Ok(StmtKind::While { cond, body })
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
        let kind = match self.peek() {
            Some(Kind::Int(n)) => ExprKind::Int(*n),
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
        let fields = self.list("}", Self::field_init)?;
        Ok(Expr {
            kind: ExprKind::StructLit(name, fields),
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
        let cond = self.cond()?;
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
        Ok(Expr {
            kind: ExprKind::If(Box::new(cond), then, els),
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
            let body = if block_body { self.primary()? } else { self.expr()? };
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

    fn pat(&mut self) -> Result<Pat, Diag> {
        let start = self.span().start;
        let neg = self.eat("-");
        let kind = match self.peek() {
            Some(Kind::Int(n)) => {
                let n = if neg { -*n } else { *n };
                self.pos += 1;
                PatKind::Int(n)
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
                    PatKind::Variant(Some(name), variant, self.bindings()?)
                } else if self.at("(") {
                    PatKind::Variant(None, name, self.bindings()?)
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
    fn bindings(&mut self) -> Result<Vec<Option<String>>, Diag> {
        if !self.eat("(") {
            return Ok(Vec::new());
        }
        self.list(")", Self::binding)
    }

    fn binding(&mut self) -> Result<Option<String>, Diag> {
        if self.at("..") {
            return Err(self.dotdot());
        }
        let span = self.span();
        let name = self.ident()?;
        if self.at("(") || self.at("::") {
            let hint = "match the inner value in a separate `match`";
            return Err(err(span, "nested patterns are not supported", Some(hint)));
        }
        Ok((name != "_").then_some(name))
    }

    fn dotdot(&self) -> Diag {
        let hint = "list every payload field, using `_` to ignore one";
        err(self.span(), "`..` is not supported in patterns", Some(hint))
    }
}
