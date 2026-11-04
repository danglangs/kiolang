use lang::ast::*;
use lang::parse::parse;
use lang::Span;

fn show(e: &Expr) -> String {
    let list = |args: &[Expr]| args.iter().fold(String::new(), |s, a| s + " " + &show(a));
    match &e.kind {
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Str(s) => format!("{s:?}"),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Unit => "()".into(),
        ExprKind::Name(n) => n.clone(),
        ExprKind::Unary(op, x) => format!("({op} {})", show(x)),
        ExprKind::Binary(op, a, b) => format!("({op} {} {})", show(a), show(b)),
        ExprKind::If(c, t, None) => format!("(if {} {})", show(c), block(t)),
        ExprKind::If(c, t, Some(e)) => format!("(if {} {} {})", show(c), block(t), show(e)),
        ExprKind::Block(b) => block(b),
        ExprKind::Call(f, args) => format!("(call {}{})", show(f), list(args)),
        ExprKind::Method(r, m, args) => format!("(.{m} {}{})", show(r), list(args)),
        ExprKind::Field(r, f) => format!("(. {} {f})", show(r)),
        ExprKind::Index(r, i) => format!("([] {} {})", show(r), show(i)),
        ExprKind::Path(e, v) => format!("{e}::{v}"),
        ExprKind::Match(s, arms) => {
            let arms = arms.iter().fold(String::new(), |a, arm| {
                a + &format!(" ({} {})", pat(&arm.pat), show(&arm.body))
            });
            format!("(match {}{arms})", show(s))
        }
        ExprKind::StructLit(n, fields) => {
            let fields = fields
                .iter()
                .fold(String::new(), |s, (f, v)| s + &format!(" {f}: {}", show(v)));
            format!("(new {n}{fields})")
        }
    }
}

fn pat(p: &Pat) -> String {
    match &p.kind {
        PatKind::Wild => "_".into(),
        PatKind::Name(n) => n.clone(),
        PatKind::Int(n) => n.to_string(),
        PatKind::Str(s) => format!("{s:?}"),
        PatKind::Bool(b) => b.to_string(),
        PatKind::Variant(e, v, bindings) => {
            let e = e.as_ref().map_or(String::new(), |e| format!("{e}::"));
            let bindings = bindings
                .iter()
                .fold(String::new(), |s, b| s + " " + b.as_deref().unwrap_or("_"));
            format!("[{e}{v}{bindings}]")
        }
    }
}

fn ty(t: &Type) -> String {
    match &t.kind {
        TypeKind::Unit => "()".into(),
        TypeKind::Named(n, args) if args.is_empty() => n.clone(),
        TypeKind::Named(n, args) => format!("{n}<{}>", args.iter().map(ty).collect::<Vec<_>>().join(",")),
    }
}

fn stmt(s: &Stmt) -> String {
    match &s.kind {
        StmtKind::Let {
            name,
            mutable,
            ty: t,
            init,
        } => {
            let m = if *mutable { " mut" } else { "" };
            let t = t.as_ref().map_or(String::new(), |t| format!(" : {}", ty(t)));
            format!("(let{m} {name}{t} {})", show(init))
        }
        StmtKind::Assign { target, op, value } => format!("({op} {} {})", show(target), show(value)),
        StmtKind::Return(None) => "(return)".into(),
        StmtKind::Return(Some(e)) => format!("(return {})", show(e)),
        StmtKind::While { cond, body } => format!("(while {} {})", show(cond), block(body)),
        StmtKind::Expr(e) => show(e),
    }
}

fn block(b: &Block) -> String {
    let mut parts: Vec<String> = b.stmts.iter().map(stmt).collect();
    parts.extend(b.tail.iter().map(|t| format!("tail {}", show(t))));
    format!("{{{}}}", parts.join("; "))
}

/// Parses `src` as the body of `fn f() { .. }` and renders it.
fn body(src: &str) -> String {
    let items = parse(&format!("fn f() {{ {src} }}")).unwrap();
    let Item::Fn(f) = &items[0] else { panic!() };
    block(&f.body)
}

fn expr(src: &str) -> String {
    let b = body(src);
    b[1..b.len() - 1].strip_prefix("tail ").unwrap().to_string()
}

fn fail(src: &str) -> lang::Diag {
    parse(src).unwrap_err()
}

#[test]
fn items() {
    let items = parse("fn add(a: Int, b: List<Map<Str, Int>>,) -> Int { a } fn nop() {}").unwrap();
    assert_eq!(items.len(), 2);
    let (Item::Fn(add), Item::Fn(nop)) = (&items[0], &items[1]) else {
        panic!()
    };
    assert_eq!(add.name, "add");
    assert_eq!(add.params.len(), 2);
    assert_eq!(ty(&add.params[1].ty), "List<Map<Str,Int>>");
    assert_eq!(ty(&add.ret), "Int");
    assert_eq!(add.span, Span { start: 0, end: 52 });
    assert_eq!(ty(&nop.ret), "()");
    assert_eq!(parse("").unwrap(), []);
}

#[test]
fn types() {
    let Item::Fn(f) = &parse("fn f(a: (), b: Option<()>) -> Result<(), IoErr> {}").unwrap()[0] else {
        panic!()
    };
    assert_eq!(ty(&f.params[0].ty), "()");
    assert_eq!(ty(&f.params[1].ty), "Option<()>");
    assert_eq!(ty(&f.ret), "Result<(),IoErr>");
}

#[test]
fn statements() {
    assert_eq!(body("let x = 1;"), "{(let x 1)}");
    assert_eq!(body("let mut x: List<Int> = y;"), "{(let mut x : List<Int> y)}");
    assert_eq!(body("x = 1; x += 2; x -= 3;"), "{(= x 1); (+= x 2); (-= x 3)}");
    assert_eq!(body("a.f = 1; a[0] += 2;"), "{(= (. a f) 1); (+= ([] a 0) 2)}");
    assert_eq!(body("return; return 1;"), "{(return); (return 1)}");
    assert_eq!(body("while x < 3 { x += 1; }"), "{(while (< x 3) {(+= x 1)})}");
    assert_eq!(body("g(1);"), "{(call g 1)}");
}

#[test]
fn blocks_and_tails() {
    assert_eq!(body(""), "{}");
    assert_eq!(body("let x = 1; x"), "{(let x 1); tail x}");
    assert_eq!(body("x;"), "{x}");
    assert_eq!(body("if a { 1 } else { 2 } x"), "{(if a {tail 1} {tail 2}); tail x}");
    assert_eq!(body("if a { b; } c;"), "{(if a {b}); c}");
    assert_eq!(body("{ 1 } 2"), "{{tail 1}; tail 2}");
    assert_eq!(body("while a {} if b {}"), "{(while a {}); tail (if b {})}");
}

#[test]
fn literals_and_names() {
    assert_eq!(expr("1"), "1");
    assert_eq!(expr("\"a\\n\""), "\"a\\n\"");
    assert_eq!(expr("true"), "true");
    assert_eq!(expr("false"), "false");
    assert_eq!(expr("name"), "name");
    assert_eq!(expr("()"), "()");
    assert_eq!(expr("(1)"), "1");
    assert_eq!(expr("{ 1 }"), "{tail 1}");
}

#[test]
fn if_chain() {
    assert_eq!(expr("if a { 1 }"), "(if a {tail 1})");
    assert_eq!(
        expr("if a { 1 } else if b { 2 } else { 3 }"),
        "(if a {tail 1} (if b {tail 2} {tail 3}))"
    );
}

#[test]
fn postfix() {
    assert_eq!(expr("f(1, g(2),)"), "(call f 1 (call g 2))");
    assert_eq!(expr("f()"), "(call f)");
    assert_eq!(expr("a.m(1).n()"), "(.n (.m a 1))");
    assert_eq!(expr("a.f.g"), "(. (. a f) g)");
    assert_eq!(expr("a[i][j]"), "([] ([] a i) j)");
    assert_eq!(expr("a.f(x)[0].g"), "(. ([] (.f a x) 0) g)");
}

#[test]
fn precedence() {
    assert_eq!(expr("1 + 2 * 3 == 7 && !b"), "(&& (== (+ 1 (* 2 3)) 7) (! b))");
    assert_eq!(expr("a || b && c"), "(|| a (&& b c))");
    assert_eq!(expr("1 - 2 - 3"), "(- (- 1 2) 3)");
    assert_eq!(expr("8 / 4 % 3"), "(% (/ 8 4) 3)");
    assert_eq!(expr("-a * b"), "(* (- a) b)");
    assert_eq!(expr("-f(x).y"), "(- (. (call f x) y))");
    assert_eq!(expr("!!a"), "(! (! a))");
    assert_eq!(expr("(1 + 2) * 3"), "(* (+ 1 2) 3)");
}

#[test]
fn spans() {
    let src = "fn f() { let x = 1 + 2; }";
    let Item::Fn(f) = &parse(src).unwrap()[0] else { panic!() };
    let StmtKind::Let { init, .. } = &f.body.stmts[0].kind else {
        panic!()
    };
    assert_eq!(init.span, Span { start: 17, end: 22 });
    assert_eq!(f.body.stmts[0].span, Span { start: 9, end: 23 });
    assert_eq!(f.body.span, Span { start: 7, end: 25 });
}

fn rejected(src: &str, text: &str, msg: &str, hint: bool) {
    rejected_with(src, text, msg, hint.then_some(""));
}

/// Like `rejected`, but a `Some` hint must equal the given text; `Some("")` accepts any hint.
fn rejected_with(src: &str, text: &str, msg: &str, hint: Option<&str>) {
    let d = fail(src);
    let start = src.find(text).unwrap();
    assert_eq!(
        d.span,
        Span {
            start,
            end: start + text.len()
        },
        "{src}"
    );
    assert_eq!(d.msg, msg, "{src}");
    match hint {
        Some("") => assert!(d.hint.is_some(), "{src}"),
        _ => assert_eq!(d.hint.as_deref(), hint, "{src}"),
    }
}

#[test]
fn rejects_reserved_words() {
    rejected("impl Foo {}", "impl", "`impl` is not supported in kio", true);
    rejected("use a;", "use", "`use` is not supported in kio", true);
    rejected("pub fn f() {}", "pub", "`pub` is not supported in kio", true);
    rejected(
        "fn f() { let x = self; }",
        "self",
        "`self` is not supported in kio",
        true,
    );
    rejected("fn f() { loop {} }", "loop", "`loop` is not supported in kio", true);
}

#[test]
fn rejects_tuples() {
    rejected("fn f() { (1, 2) }", ",", "kio has no tuples", true);
    rejected("fn f(a: (Int, Int)) {}", "Int", "kio has no tuple types", true);
}

#[test]
fn rejects_later_features() {
    rejected("fn f() { for x in y {} }", "for", "`for` is not supported in kio", true);
    rejected("fn f() { break; }", "break", "`break` is not supported in kio", true);
}

#[test]
fn rejects_trailing_tokens() {
    rejected("fn f() {} 1", "1", "expected `fn`", false);
    rejected("fn f() {} ;", ";", "expected `fn`", false);
}

#[test]
fn rejects_chained_comparison() {
    let d = fail("fn f() { a < b < c }");
    assert_eq!(d.span, Span { start: 15, end: 16 });
    assert_eq!(d.msg, "comparisons cannot be chained");
    assert!(d.hint.is_some());
    assert!(parse("fn f() { a < b && b < c }").is_ok());
}

#[test]
fn rejects_syntax_errors() {
    rejected_with("fn f() { let x = 1 }", "}", "expected `;`", Some("add `;`"));
    rejected("fn f() { 1 = 2; }", "1", "cannot assign to this expression", true);
    rejected_with("fn f() { a b }", "b", "expected `;` or `}`", Some("add `;`"));
    let d = fail("fn f() {");
    assert_eq!(d.msg, "expected `}`");
    assert_eq!(d.hint.as_deref(), Some("a `{` is not closed"));
    assert_eq!(fail("fn f() { 1 + }").msg, "expected an expression");
    assert_eq!(fail("fn f() { \"x }").msg, "unterminated string");
}

#[test]
fn rejects_deep_nesting() {
    let n = 65;
    let parens = format!("fn f() {{ {}1{} }}", "(".repeat(n), ")".repeat(n));
    assert_eq!(fail(&parens).msg, "nesting is too deep");
    let blocks = format!("fn f() {}{}", "{".repeat(n), "}".repeat(n));
    assert_eq!(fail(&blocks).msg, "nesting is too deep");
    let ok = format!("fn f() {{ {}1{} }}", "(".repeat(60), ")".repeat(60));
    assert!(parse(&ok).is_ok());
}

#[test]
fn struct_items() {
    let src = "struct P { x: Int, q: List<Int>, } struct E {} fn f() {}";
    let items = parse(src).unwrap();
    assert_eq!(items.len(), 3);
    let (Item::Struct(p), Item::Struct(e)) = (&items[0], &items[1]) else {
        panic!()
    };
    assert_eq!(p.name, "P");
    assert_eq!(p.fields.len(), 2);
    assert_eq!(p.fields[0].name, "x");
    assert_eq!(ty(&p.fields[1].ty), "List<Int>");
    assert_eq!(p.span, Span { start: 0, end: 34 });
    assert!(e.fields.is_empty());
}

#[test]
fn struct_literals() {
    assert_eq!(expr("P { a: 1, b: Q { c: x }, }"), "(new P a: 1 b: (new Q c: x))");
    assert_eq!(expr("P {}"), "(new P)");
    assert_eq!(expr("a.b.c"), "(. (. a b) c)");
    assert_eq!(expr("P { a: 1 }.a"), "(. (new P a: 1) a)");
    assert_eq!(body("p = P { a: 1 };"), "{(= p (new P a: 1))}");
}

#[test]
fn struct_literals_in_conditions() {
    assert_eq!(expr("if x == P { 2 }"), "(if (== x P) {tail 2})");
    assert_eq!(expr("if x == (P { a: 1 }) { 2 }"), "(if (== x (new P a: 1)) {tail 2})");
    assert_eq!(body("while f(P { a: 1 }) {}"), "{(while (call f (new P a: 1)) {})}");
    assert_eq!(
        expr("if a { P { a: 1 } } else { Q {} }"),
        "(if a {tail (new P a: 1)} {tail (new Q)})"
    );
    // A nested `if` in a condition must not lift the restriction for the rest of it.
    assert_eq!(
        expr("if if b { c } else { d } == P { 2 }"),
        "(if (== (if b {tail c} {tail d}) P) {tail 2})"
    );
}

#[test]
fn rejects_struct_shorthand() {
    let d = fail("fn f() { P { a } }");
    assert_eq!(d.span, Span { start: 13, end: 14 });
    assert_eq!(d.msg, "field shorthand is not supported");
    assert_eq!(d.hint.as_deref(), Some("write `a: a`"));
}

#[test]
fn enum_items() {
    let src = "enum S { A, B(Int, List<Str>), C(),} enum E {} fn f() {}";
    let items = parse(src).unwrap();
    let Item::Enum(e) = &items[0] else { panic!() };
    assert_eq!(e.name, "S");
    let shape: Vec<_> = e.variants.iter().map(|v| (v.name.as_str(), v.fields.len())).collect();
    assert_eq!(shape, [("A", 0), ("B", 2), ("C", 0)]);
    assert_eq!(ty(&e.variants[1].fields[1]), "List<Str>");
    assert_eq!(e.variants[1].span, Span { start: 12, end: 29 });
    assert_eq!(e.span, Span { start: 0, end: 36 });
    let Item::Enum(e) = &items[1] else { panic!() };
    assert!(e.variants.is_empty());
}

#[test]
fn paths() {
    assert_eq!(expr("Status::Due"), "Status::Due");
    assert_eq!(expr("Status::Due(5)"), "(call Status::Due 5)");
    assert_eq!(expr("Status::Due(Status::Idle)"), "(call Status::Due Status::Idle)");
}

#[test]
fn match_patterns() {
    assert_eq!(
        expr("match x { _ => 0, n => 1, 7 => 2, -7 => 3, \"s\" => 4, true => 5, false => 6, E::A => 7, E::B(a, _) => 8, B(_, b,) => 9, None => 10 }"),
        "(match x (_ 0) (n 1) (7 2) (-7 3) (\"s\" 4) (true 5) (false 6) ([E::A] 7) ([E::B a _] 8) ([B _ b] 9) (None 10))"
    );
}

#[test]
fn match_arms() {
    assert_eq!(expr("match x {}"), "(match x)");
    assert_eq!(expr("match x { a => 1, }"), "(match x (a 1))");
    assert_eq!(
        expr("match x { a => { 1 } b => { 2 }, c => 3 }"),
        "(match x (a {tail 1}) (b {tail 2}) (c 3))"
    );
    assert_eq!(
        expr("match x { a => match y { _ => 1 }, _ => 2 }"),
        "(match x (a (match y (_ 1))) (_ 2))"
    );
}

#[test]
fn match_is_block_like() {
    assert_eq!(body("match x { _ => {} } y"), "{(match x (_ {})); tail y}");
    assert_eq!(body("match x { _ => 1 }"), "{tail (match x (_ 1))}");
    assert_eq!(expr("1 + match x { _ => 1 }"), "(+ 1 (match x (_ 1)))");
}

#[test]
fn match_scrutinee_is_a_condition() {
    assert_eq!(
        expr("match a == b { _ => P { x: 1 } }"),
        "(match (== a b) (_ (new P x: 1)))"
    );
    assert_eq!(expr("match (P { x: 1 }) { _ => 1 }"), "(match (new P x: 1) (_ 1))");
}

#[test]
fn rejects_enum_syntax() {
    let d = fail("enum E { A { f: Int } }");
    assert_eq!(d.span, Span { start: 11, end: 12 });
    assert_eq!(d.msg, "struct-like variants are not supported");
    assert!(d.hint.is_some());
    rejected_with("enum E { A(Int }", "}", "expected `)`", Some("a `(` is not closed"));
}

#[test]
fn rejects_match_syntax() {
    rejected("fn f() { match x { -y => 1 } }", "y", "expected an integer", false);
    rejected(
        "fn f() { match x { n if n > 1 => 1 } }",
        "if",
        "match guards are not supported",
        true,
    );
    rejected(
        "fn f() { match x { Some(Inner(y)) => 1 } }",
        "Inner",
        "nested patterns are not supported",
        true,
    );
    rejected(
        "fn f() { match x { E::A(F::B) => 1 } }",
        "F",
        "nested patterns are not supported",
        true,
    );
    rejected(
        "fn f() { match x { E::A(..) => 1 } }",
        "..",
        "`..` is not supported in patterns",
        true,
    );
    rejected(
        "fn f() { match x { .. => 1 } }",
        "..",
        "`..` is not supported in patterns",
        true,
    );
    rejected("fn f() { match x { 1 + 2 => 1 } }", "+", "expected `=>`", false);
    rejected_with(
        "fn f() { match x { a => 1 b => 2 } }",
        "b",
        "expected `}`",
        Some("a `{` is not closed"),
    );
}
