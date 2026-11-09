//! L13 `|` in patterns and L14 `let`-else: parsing, checking and running. The programs here also
//! run compiled, through the differential harness.

mod common;

use lang::ast::{Item, PatKind, StmtKind};
use lang::parse::parse;
use lang::{check, run, CapTable, Caps, Value};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

fn eval(src: &str) -> Value {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || run(&p, &mut NoCaps)).unwrap()
}

fn str(src: &str) -> String {
    match eval(src) {
        Value::Str(s) => s.to_string(),
        v => panic!("{v:?}"),
    }
}

/// The single diagnostic `check` gives: message, the source it points at, and hint.
fn reject(src: &str) -> (String, String, String) {
    let diags = check(src, &CapTable::default()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    let hint = d.hint.clone().unwrap_or_default();
    (d.msg.clone(), src[d.span.start..d.span.end].to_string(), hint)
}

/// The parse error: message, the source it points at, and hint.
fn parse_err(src: &str) -> (String, String, String) {
    let d = parse(src).unwrap_err();
    (
        d.msg.clone(),
        src[d.span.start..d.span.end].to_string(),
        d.hint.unwrap_or_default(),
    )
}

/// The statements of the first function's body.
fn body(src: &str) -> Vec<StmtKind> {
    let items = parse(src).unwrap();
    let Item::Fn(f) = &items[0] else { panic!() };
    f.body.stmts.iter().map(|s| s.kind.clone()).collect()
}

#[test]
fn parses_alternatives() {
    let items = parse("fn f() -> Int { match x { | 1 | 2 => 1, Some(3 | 4) => 2, _ => 3 } }").unwrap();
    let Item::Fn(f) = &items[0] else { panic!() };
    let lang::ast::ExprKind::Match(_, arms) = &f.body.tail.as_ref().unwrap().kind else {
        panic!()
    };
    let PatKind::Or(alts) = &arms[0].pat.kind else { panic!() };
    assert_eq!(alts.len(), 2);
    assert!(matches!(alts[1].kind, PatKind::Int(2)));
    let PatKind::Variant(_, _, parts) = &arms[1].pat.kind else {
        panic!()
    };
    assert!(matches!(&parts[0].kind, PatKind::Or(a) if a.len() == 2));
    // `|` in `if let`, and a module path in an alternative.
    parse("fn f() { if let A | B = x { } }").unwrap();
    parse("fn f() { match x { web::E::A | web::E::B => 1, _ => 2 } }").unwrap();
    // Closures are untouched.
    parse("fn f() { let g = |x: Int| x; let h = || 1; }").unwrap();
}

#[test]
fn parses_let_else() {
    let s = body("fn f() { let Some(x) = o else { return; }; let y = o else { return; }; }");
    let StmtKind::LetElse { pat, ty, els, .. } = &s[0] else {
        panic!("{s:?}")
    };
    assert!(matches!(pat.kind, PatKind::Variant(..)) && ty.is_none() && els.stmts.len() == 1);
    let StmtKind::LetElse { pat, .. } = &s[1] else {
        panic!("{s:?}")
    };
    assert_eq!(pat.kind, PatKind::Name("y".into()));
    let s = body("fn f() { let Some(x): Option<Int> = None else { return; }; let 1 | 2 = n else { return; }; }");
    assert!(matches!(&s[0], StmtKind::LetElse { ty: Some(_), .. }));
    assert!(matches!(&s[1], StmtKind::LetElse { pat, .. } if matches!(pat.kind, PatKind::Or(_))));
    // A plain `let` is unchanged.
    assert!(matches!(&body("fn f() { let x = 1; }")[0], StmtKind::Let { .. }));
}

#[test]
fn let_else_parse_errors() {
    assert_eq!(
        parse_err("fn f() { let Some(x) = o; }"),
        (
            "a pattern in `let` needs an `else` block".into(),
            "Some(x)".into(),
            "add `else { return; }` after the value, or use `match`".into()
        )
    );
    assert_eq!(
        parse_err("fn f() { let mut x = o else { return; }; }"),
        ("this pattern always matches".into(), "x".into(), "write `let`".into())
    );
    let (msg, at, _) = parse_err("fn f() { let Some(x) = o else { return; } }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected `;`", "}"));
}

#[test]
fn alternatives_must_bind_the_same_names() {
    let (msg, at, hint) =
        reject("fn main() -> Int { let r: Result<Int, Int> = Ok(1); match r { Ok(x) | Err(_) => x } }");
    assert_eq!(msg, "`x` is bound in one alternative but not the other");
    assert_eq!(at, "Err(_)");
    assert_eq!(hint, "bind `x` in every alternative, or in none");
    let (msg, at, _) = reject("fn main() -> Int { let r: Result<Int, Int> = Ok(1); match r { Ok(_) | Err(y) => y } }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`y` is bound in one alternative but not the other", "Ok(_)")
    );
    let (msg, at, hint) = reject(
        "enum E { A(Int), B(Str) }
         fn main() -> Int { match E::A(1) { E::A(x) | E::B(x) => 1 } }",
    );
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`x` is Int in one alternative and Str in another", "E::B(x)")
    );
    assert_eq!(hint, "bind values of the same type in every alternative");
    let (msg, _, _) = reject(
        "enum E { P(Int, Int) }
         fn main() -> Int { match E::P(1, 2) { E::P(x, x | x) => x } }",
    );
    assert_eq!(msg, "`x` is bound twice in this pattern");
}

#[test]
fn alternatives_and_exhaustiveness() {
    let ok = |src: &str| {
        check(src, &CapTable::default())
            .map(|_| ())
            .unwrap_or_else(|d| panic!("{d:?}"))
    };
    ok("fn main() -> Int { match true { true | false => 1 } }");
    ok("enum E { A, B, C } fn main() -> Int { match E::A { E::A | E::B => 1, E::C => 2 } }");
    ok("fn main() -> Int { let o: Option<Int> = None; match o { Some(1 | 2) => 1, Some(_) | None => 0 } }");
    let (msg, at, hint) =
        reject("fn main() -> Int { let o: Option<Option<Int>> = None; match o { Some(Some(_) | None) => 1 } }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("non-exhaustive `match`: missing Option::None", "o")
    );
    assert_eq!(hint, "add arms for the missing cases, or `_ => ..`");
    let (msg, at, _) = reject("fn main() -> Int { let n = 3; match n { 1 | 2 => 1 } }");
    assert_eq!((msg.as_str(), at.as_str()), ("non-exhaustive `match`: missing _", "n"));
    // One alternative that earlier arms cover, and a whole arm.
    let (msg, at, hint) = reject("fn main() -> Int { let n = 3; match n { 1 => 0, 2 | 1 => 1, _ => 2 } }");
    assert_eq!((msg.as_str(), at.as_str()), ("unreachable alternative", "1"));
    assert_eq!(hint, "earlier patterns match every value it matches; remove it");
    let (msg, at, _) = reject("fn main() -> Int { let n = 3; match n { 2 | 2 => 1, _ => 2 } }");
    assert_eq!((msg.as_str(), at.as_str()), ("unreachable alternative", "2"));
    let (msg, at, _) = reject("fn main() -> Int { let n = 3; match n { 1 | 2 => 0, 2 | 1 => 1, _ => 2 } }");
    assert_eq!((msg.as_str(), at.as_str()), ("unreachable arm", "2 | 1"));
    let (msg, at, _) = reject("fn main() -> Int { match true { true | false => 1, _ => 2 } }");
    assert_eq!((msg.as_str(), at.as_str()), ("unreachable arm", "_"));
}

#[test]
fn let_else_rules() {
    let (msg, at, hint) = reject("fn main() -> Int { let o: Option<Int> = None; let Some(x) = o else { 1; }; x }");
    assert_eq!(msg, "the `else` block of `let`-else must leave the function or loop");
    assert_eq!(at, "{ 1; }");
    assert_eq!(
        hint,
        "end it with `return ..`: its last statement must be `return`, `break` or `continue`"
    );
    // The pinned rule is syntactic: an `if` whose branches both return is not enough.
    let (msg, _, _) = reject(
        "fn main() -> Int { let o: Option<Int> = None; let Some(x) = o else { if true { return 1; } else { return 2; } }; x }",
    );
    assert_eq!(msg, "the `else` block of `let`-else must leave the function or loop");
    let (msg, at, hint) = reject("fn main() -> Int { let o: Option<Int> = None; let x = o else { return 0; }; 1 }");
    assert_eq!(
        (msg.as_str(), at.as_str(), hint.as_str()),
        ("this pattern always matches", "x", "write `let`")
    );
    let (msg, at, _) = reject("enum W { A(Int) } fn main() -> Int { let W::A(n) = W::A(1) else { return 0; }; n }");
    assert_eq!((msg.as_str(), at.as_str()), ("this pattern always matches", "W::A(n)"));
    let (msg, at, _) = reject("fn main() -> Int { let _ = 1 else { return 0; }; 1 }");
    assert_eq!((msg.as_str(), at.as_str()), ("this pattern always matches", "_"));
    // `break` only in a loop; the pattern's names are not in scope in the else block.
    let (msg, _, _) = reject("fn main() { let o: Option<Int> = None; let Some(x) = o else { break; }; }");
    assert_eq!(msg, "`break` outside a loop");
    let (msg, _, _) = reject("fn main() -> Int { let o: Option<Int> = None; let Some(x) = o else { return x; }; x }");
    assert_eq!(msg, "unknown name `x`");
    let (msg, at, _) = reject("fn main() { let Some(x) = 5 else { return; }; }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("expected Int, found variant `Some`", "Some(x)")
    );
    let (msg, _, _) = reject("fn main() { let Some(x): Option<Str> = Some(1) else { return; }; }");
    assert_eq!(msg, "expected Str, found Int");
    // The bindings are not `mut`.
    let (msg, _, _) = reject("fn main() { let o: Option<Int> = None; let Some(x) = o else { return; }; x = 2; }");
    assert!(msg.contains("`x`"), "{msg}");
}

const SHAPES: &str = "
enum Shape<T> { Dot(T), Line(T, T), Box(T, T, T), Empty }

fn first<T>(s: Shape<T>, d: T) -> T {
    match s {
        Shape::Dot(x) | Shape::Line(x, _) | Shape::Box(_, _, x) => x,
        Shape::Empty => d,
    }
}

fn main() -> Str {
    let a = first(Shape::Line(\"l\", \"m\"), \"d\");
    let b = first(Shape::Box(\"x\", \"y\", \"z\"), \"d\");
    let c = first(Shape::Empty, \"d\");
    let d = first(Shape::Dot(7), 0).to_str();
    a + b + c + d
}
";

#[test]
fn alternatives_bind_in_different_places() {
    assert_eq!(str(SHAPES), "lzd7");
}

#[test]
fn nested_alternatives_run() {
    let src = "
enum Op { Add(Int, Int), Neg(Int), Nop }

fn kind(r: Result<Option<Int>, Int>) -> Int {
    match r {
        Ok(Some(1 | 2)) => 10,
        Ok(Some(x)) | Err(x) => x,
        Ok(None) => -1,
    }
}

fn op(o: Op) -> Int {
    match o {
        Op::Add(0 | 1, x) | Op::Neg(x) => x,
        Op::Add(a, b) => a + b,
        | Op::Nop => 0,
    }
}

fn verb(v: Str) -> Int {
    match v {
        \"GET\" | \"HEAD\" => 1,
        \"POST\" | \"PUT\" | \"PATCH\" => 2,
        _ => 3,
    }
}

fn main() -> Int {
    let mut n = kind(Ok(Some(2))) + kind(Ok(Some(5))) * 100 + kind(Err(3)) * 1000 + kind(Ok(None));
    n = n * 10 + op(Op::Add(1, 4)) + op(Op::Neg(2)) + op(Op::Add(3, 4)) + op(Op::Nop);
    n = n * 10 + verb(\"HEAD\") + verb(\"PATCH\") * 10 + verb(\"DELETE\") * 100;
    if let Some(1 | 3) = Some(3) {
        n = n + 1;
    }
    let r: Result<Int, Int> = Err(5);
    if let Ok(x) | Err(x) = r {
        n = n + x;
    }
    n
}
";
    // kind: 10 + 500 + 3000 - 1 = 3509; op: 4 + 2 + 7 + 0 = 13; verb: 1 + 20 + 300 = 321.
    assert_eq!(eval(src), Value::Int((3509 * 10 + 13) * 10 + 321 + 1 + 5));
}

#[test]
fn let_else_runs() {
    let src = "
enum Cmd { Say(Str), Quit }

fn word(c: Cmd) -> Str {
    let Cmd::Say(w) = c else { return \"quit\"; };
    w + \"!\"
}

fn pair(a: Option<Str>, b: Result<Int, Str>) -> Str {
    let Some(x) = a else { return \"no a\"; };
    let Ok(n) = b else { return x + \": no b\"; };
    x + n.to_str()
}

fn sum(xs: List<Option<Int>>) -> Int {
    let mut total = 0;
    for o in xs {
        let Some(n) = o else { continue; };
        let 1 | 2 | 3 = n else { break; };
        total += n;
    }
    total
}

fn main() -> Str {
    let Some(k): Option<Int> = None else {
        let mut s = word(Cmd::Say(\"hi\")) + word(Cmd::Quit);
        s = s + pair(Some(\"a\"), Ok(1)) + pair(None, Ok(1)) + pair(Some(\"c\"), Err(\"e\"));
        return s + sum([Some(1), None, Some(3), Some(9), Some(2)]).to_str();
    };
    k.to_str()
}
";
    assert_eq!(str(src), "hi!quita1no ac: no b4");
}

#[test]
fn let_else_in_blocks_and_loops() {
    let src = "
fn first(xs: List<Str>) -> Str {
    let mut xs = xs;
    let mut out = \"\";
    while true {
        let Some(x) = xs.pop() else { break; };
        out = x;
    }
    out
}

fn main() -> Int {
    let v = {
        let r: Result<Int, Str> = Ok(4);
        let Ok(n) = r else { return 0; };
        n * 2
    };
    let mut seen = 0;
    loop {
        let o: Option<Int> = None;
        let Some(_) = o else { break; };
        seen += 1;
    }
    v + seen + first([\"bb\", \"a\"]).len() * 100
}
";
    assert_eq!(eval(src), Value::Int(208));
}

#[test]
fn let_else_in_value_positions() {
    let src = "
fn f(o: Option<Int>, c: Bool) -> Int {
    let y = if c { let Some(x) = o else { return 0; }; return x + 1; } else { 5 };
    y
}

fn g(o: Option<Int>) -> Int {
    let Some(x) = o else { return 0; };
    return x * 2;
}

fn h(o: Option<Int>, n: Int) -> Int {
    match n {
        0 => { let Some(x) = o else { return -1; }; return x; }
        _ => n,
    }
}

fn k(o: Option<Str>) -> Str {
    let s = { let Some(x) = o else { return \"none\"; }; x + \"?\" };
    s
}

fn main() -> Int {
    f(Some(3), true) + f(None, true) * 10 + f(None, false) * 100 + g(Some(4)) * 1000
        + h(Some(7), 0) + h(None, 0) + h(None, 9) + k(Some(\"ab\")).len() * 10000 + k(None).len() * 100000
}
";
    assert_eq!(eval(src), Value::Int(438519));
}
