use common::caps;
use lang::check::closest;
use lang::{check, CapTable};

mod common;

/// Checks `src` and returns the single diagnostic's text and the source it points at.
fn reject(src: &str) -> (String, String) {
    let diags = check(src, &CapTable::default()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    (d.msg.clone(), src[d.span.start..d.span.end].to_string())
}

fn ok(src: &str) {
    if let Err(d) = check(src, &CapTable::default()) {
        panic!("{d:?}");
    }
}

#[test]
fn accepts_valid_program() {
    ok("fn main() -> Int { let mut x = 1; x += 2; if x > 2 { x } else { 0 } }");
    ok("fn main() { helper(); } fn helper() {}");
}

#[test]
fn unknown_name() {
    let (msg, at) = reject("fn main() -> Int { y }");
    assert_eq!((msg.as_str(), at.as_str()), ("unknown name `y`", "y"));
}

#[test]
fn unknown_function() {
    let (msg, at) = reject("fn main() { g(1); }");
    assert_eq!((msg.as_str(), at.as_str()), ("unknown function `g`", "g"));
}

#[test]
fn wrong_arity() {
    let (msg, at) = reject("fn f(a: Int) {} fn main() { f(1, 2); }");
    assert_eq!(msg, "`f` takes 1 arguments, found 2");
    assert_eq!(at, "f(1, 2)");
}

#[test]
fn type_mismatch() {
    let (msg, at) = reject("fn main() { let x: Int = \"a\"; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    let (msg, at) = reject("fn f(a: Int) {} fn main() { f(true); }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Bool", "true"));
    let (msg, _) = reject("fn main() -> Int { 1 + \"a\" }");
    assert_eq!(msg, "`+` cannot be applied to Int and Str");
    let (msg, _) = reject("fn main() -> Bool { 1 == true }");
    assert_eq!(msg, "`==` cannot be applied to Int and Bool");
}

#[test]
fn non_bool_condition() {
    let (msg, at) = reject("fn main() { if 1 { } }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Bool, found Int", "1"));
    let (msg, at) = reject("fn main() { while \"a\" { } }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Bool, found Str", "\"a\""));
}

#[test]
fn loops() {
    ok("fn main() { for x in [1, 2] { if x == 1 { continue; } break; } for i in 0..3 { let _j: Int = i; } }");
    ok("fn main() -> Int { let mut n = 0; while true { n += 1; if n > 2 { break; } } n }");
    let (msg, at) = reject("fn main() { break; }");
    assert_eq!((msg.as_str(), at.as_str()), ("`break` outside a loop", "break;"));
    let diags = check("fn main() { continue; }", &CapTable::default()).unwrap_err();
    assert_eq!(diags[0].hint.as_deref(), Some("use `return` to leave a function"));
    reject("fn main() { for x in [1] { } break; }");
    reject("fn main() { for x in [1] { f(); } } fn f() { break; }");
    let (msg, at) = reject("fn main() { for x in 5 { } }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot iterate over Int", "5"));
    let (msg, at) = reject("fn main() { for i in 0..\"a\" { } }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    let (msg, _) = reject("fn main() { for x in [1] { x = 2; } }");
    assert!(msg.contains("mut"), "{msg}");
    reject("fn main() { for x in [1] { } let y = x; }");
}

#[test]
fn missing_return() {
    let (msg, at) = reject("fn main() -> Int { let x = 1; }");
    assert_eq!(msg, "missing return: `main` must return Int");
    assert_eq!(at, "{ let x = 1; }");
    reject("fn main() -> Int { if true { return 1; } }");
}

#[test]
fn returns_never_fall_through() {
    ok("fn main() -> Int { return 1; }");
    ok("fn main() -> Int { if true { return 1; } else { return 2; } }");
    ok("fn main() -> Int { if true { return 1; } else { 2 } }");
}

#[test]
fn return_type_mismatch() {
    let (msg, at) = reject("fn main() -> Int { return true; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Bool", "true"));
    reject("fn main() -> Int { return; }");
}

#[test]
fn if_branches_differ() {
    let (msg, _) = reject("fn main() { let x = if true { 1 } else { \"a\" }; }");
    assert_eq!(msg, "`if` branches have different types: Int and Str");
    reject("fn main() { let x = if true { 1 }; }");
}

#[test]
fn assign_to_immutable() {
    let (msg, at) = reject("fn main() { let x = 1; x = 2; }");
    assert_eq!(msg, "cannot assign to `x`, it is not `mut`");
    assert_eq!(at, "x");
    reject("fn f(a: Int) { a += 1; } fn main() {}");
}

#[test]
fn assign_type_mismatch() {
    reject("fn main() { let mut x = 1; x = \"a\"; }");
    reject("fn main() { let mut x = true; x += 1; }");
}

#[test]
fn duplicates() {
    let (msg, _) = reject("fn f() {} fn f() {} fn main() {}");
    assert_eq!(msg, "duplicate function `f`");
    let (msg, at) = reject("fn f(a: Int, a: Int) {} fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("duplicate parameter `a`", "a: Int"));
}

/// Wasm allows 1,000 parameters, and a function used as a value takes itself too: 999 at most,
/// a closure's captures included.
#[test]
fn too_many_parameters() {
    let params = |n: usize| (0..n).map(|i| format!("p{i}: Int")).collect::<Vec<_>>().join(", ");
    ok(&format!("fn f({}) {{}} fn main() {{}}", params(999)));
    let src = format!("fn f({}) {{}} fn main() {{}}", params(1001));
    let diags = check(&src, &CapTable::default()).expect_err("too many");
    let d = &diags[0];
    assert_eq!(d.msg, "`f` takes 1001 parameters; at most 999");
    assert_eq!(&src[d.span.start..d.span.end], "p999: Int");
    assert_eq!(d.hint.as_deref(), Some("pass a struct instead"));

    // 500 captures and 500 parameters: too many once lifted.
    let lets: String = (0..500).map(|i| format!("let c{i} = {i}; ")).collect();
    let uses: Vec<String> = (0..500).map(|i| format!("c{i}")).collect();
    let ps: Vec<String> = (0..500).map(|i| format!("q{i}: Int")).collect();
    let closure = format!("|{}| [{}]", ps.join(", "), uses.join(", "));
    let src = format!("fn main() {{ {lets}let f = {closure}; }}");
    let diags = check(&src, &CapTable::default()).expect_err("too many");
    let d = &diags[0];
    assert_eq!(
        d.msg,
        "this closure takes 500 parameters and captures 500 locals; at most 999"
    );
    assert_eq!(&src[d.span.start..d.span.end], closure);
    assert_eq!(d.hint.as_deref(), Some("pass a struct instead"));
    ok(&format!(
        "fn main() {{ {lets}let f = |{}| [{}]; }}",
        ps[..499].join(", "),
        uses.join(", ")
    ));
}

#[test]
fn main_rules() {
    let (msg, _) = reject("fn f() {}");
    assert_eq!(msg, "no `main` function");
    reject("fn main(a: Int) {}");
}

#[test]
fn unavailable_types_and_features() {
    let (_, at) = reject("fn main() { let x: Foo = 1; }");
    assert_eq!(at, "Foo");
    reject("fn f(x: Vec<Int>) {} fn main() {}");
    reject("fn main() { let x = 1; x.f; }");
    reject("fn main() { let x = 1; x[0]; }");
    reject("fn main() { let x = 1; x.m(); }");
    reject("fn main() { let mut x = 1; x.f = 2; }");
}

#[test]
fn shadowing_and_block_scope() {
    ok("fn main() -> Str { let x = 1; let x = \"a\"; x }");
    reject("fn main() { { let x = 1; } x; }");
}

#[test]
fn one_diag_per_function_in_source_order() {
    let src = "fn a() { x; y; } fn main() { z; } fn b() { w; }";
    let diags = check(src, &CapTable::default()).unwrap_err();
    let at: Vec<_> = diags.iter().map(|d| &src[d.span.start..d.span.end]).collect();
    assert_eq!(at, ["x", "z", "w"]);
}

#[test]
fn parse_errors_surface() {
    reject("fn main( {");
}

const P: &str = "struct P { a: Int, b: Q } struct Q { c: Int } ";

fn reject_p(body: &str) -> (String, String) {
    reject(&format!("{P}{body}"))
}

#[test]
fn accepts_structs() {
    ok(&format!(
        "{P}fn f(p: P) -> Q {{ p.b }}
         fn main() -> Int {{
             let mut p = P {{ b: Q {{ c: 1 }}, a: 2 }};
             p.b.c += p.a; p.a = f(p).c; p.a
         }}"
    ));
    ok("struct E {} fn main() { let e = E {}; }");
}

#[test]
fn struct_declaration_errors() {
    let (msg, at) = reject("struct A {} struct A {} fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("duplicate type `A`", "struct A {}"));
    let (msg, at) = reject("struct A { x: Int, x: Str } fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("duplicate field `x`", "x: Str"));
    let (_, at) = reject("struct A { x: B } fn main() {}");
    assert_eq!(at, "B");
}

#[test]
fn recursive_structs() {
    let (msg, at) = reject("struct A { a: A } fn main() {}");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("struct `A` contains itself", "struct A { a: A }")
    );
    // Each struct on the cycle is reported.
    let src = "struct A { b: B } struct B { a: A } fn main() {}";
    assert_eq!(check(src, &CapTable::default()).unwrap_err().len(), 2);
    // Sharing a struct is not recursion.
    ok("struct A { x: C, y: C } struct C {} fn main() {}");
    // `Option` and `Result` store inline; `List` and `Map` use the heap.
    let (msg, _) = reject("struct Node { next: Option<Node> } fn main() {}");
    assert_eq!(msg, "struct `Node` contains itself");
    assert_eq!(
        hint("struct A { r: Result<Int, A> } fn main() {}"),
        "use indexes into a `List` instead"
    );
    ok("struct Tree { kids: List<Tree> } fn main() {}");
    ok("struct Tree { kids: Map<Str, Tree>, up: List<Option<Tree>> } fn main() {}");
}

#[test]
fn struct_literal_errors() {
    let (msg, at) = reject_p("fn main() { Z { a: 1 }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("unknown struct `Z`", "Z { a: 1 }"));
    let (msg, at) = reject_p("fn main() { Q { d: 1, c: 1 }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("no field `d` on Q", "1"));
    let (msg, at) = reject_p("fn main() { P { a: 1 }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("missing field `b` in `P`", "P { a: 1 }"));
    let (msg, at) = reject_p("fn main() { Q { c: 1, c: 2 }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("duplicate field `c`", "2"));
    let (msg, at) = reject_p("fn main() { Q { c: true }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Bool", "true"));
    reject_p("fn main() { P { a: 1, b: 2 }; }");
}

#[test]
fn field_access_errors() {
    let (msg, at) = reject_p("fn main() { let p = P { a: 1, b: Q { c: 1 } }; p.z; }");
    assert_eq!((msg.as_str(), at.as_str()), ("no field `z` on P", "p.z"));
    let diags = check(
        &format!("{P}fn main() {{ let q = Q {{ c: 1 }}; q.z; }}"),
        &CapTable::default(),
    )
    .unwrap_err();
    assert_eq!(diags[0].hint.as_deref(), Some("did you mean `c`?"));
    let (msg, _) = reject_p("fn main() { let x = 1; x.f; }");
    assert_eq!(msg, "no field `f` on Int");
}

#[test]
fn field_assignment_errors() {
    let (msg, at) = reject_p("fn main() { let q = Q { c: 1 }; q.c = 2; }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("cannot assign to `q`, it is not `mut`", "q")
    );
    reject_p("fn f(q: Q) { q.c = 1; } fn main() {}");
    let (msg, _) = reject_p("fn main() { let mut q = Q { c: 1 }; q.c = true; }");
    assert_eq!(msg, "expected Int, found Bool");
    reject_p("fn main() { let mut q = Q { c: 1 }; q.z = 1; }");
    reject_p("fn main() { let mut q = Q { c: 1 }; q.c += \"a\"; }");
    reject_p("fn main() { let mut p = P { a: 1, b: Q { c: 1 } }; p.b += 1; }");
}

#[test]
fn struct_equality_is_accepted() {
    ok(&format!("{P}fn main() -> Bool {{ Q {{ c: 1 }} == Q {{ c: 1 }} }}"));
    let (msg, _) = reject_p("fn main() -> Bool { Q { c: 1 } == 1 }");
    assert_eq!(msg, "`==` cannot be applied to Q and Int");
}

#[test]
fn struct_type_mismatch() {
    let (msg, _) = reject_p("fn f(p: P) {} fn main() { f(Q { c: 1 }); }");
    assert_eq!(msg, "expected P, found Q");
}

const S: &str = "enum S { Done, Due(Int), Pair(Int, Str) } ";

fn reject_s(body: &str) -> (String, String) {
    reject(&format!("{S}{body}"))
}

#[test]
fn accepts_enums() {
    ok(&format!(
        "{S}struct W {{ s: S }}
         fn f(s: S) -> Int {{
             match s {{ S::Done => 0, Due(n) => n, S::Pair(a, _) => a }}
         }}
         fn g(s: S) -> Int {{ match s {{ Done => 1, other => f(other) }} }}
         fn h(n: Int, b: Bool, t: Str) -> Int {{
             let a = match n {{ 0 => 1, -1 => 2, _ => match b {{ true => 3, false => 4 }} }};
             a + match t {{ \"a\" => 1, _ => {{ return 0; }} }}
         }}
         fn main() {{ let w = W {{ s: S::Due(1) }}; f(w.s); g(S::Done); }}"
    ));
}

#[test]
fn enum_declaration_errors() {
    let (msg, at) = reject("enum A { X } struct A {} fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("duplicate type `A`", "struct A {}"));
    let (msg, _) = reject("enum A { X } enum A { Y } fn main() {}");
    assert_eq!(msg, "duplicate type `A`");
    let (msg, at) = reject("enum A { X, X(Int) } fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("duplicate variant `X`", "X(Int)"));
    let (_, at) = reject("enum A { X(B) } fn main() {}");
    assert_eq!(at, "B");
}

#[test]
fn recursive_enums() {
    let (msg, _) = reject("enum A { X(A) } fn main() {}");
    assert_eq!(msg, "enum `A` contains itself");
    let src = "enum A { X(S) } struct S { a: A } fn main() {}";
    assert_eq!(check(src, &CapTable::default()).unwrap_err().len(), 2);
    ok("enum A { X(B, B) } enum B { Y } fn main() {}");
}

#[test]
fn variant_expression_errors() {
    let (msg, at) = reject_s("fn main() { S::Nope; }");
    assert_eq!((msg.as_str(), at.as_str()), ("unknown variant `S::Nope`", "S::Nope"));
    reject_s("fn main() { T::Done; }");
    let (msg, _) = reject_s("fn main() { S::Due; }");
    assert_eq!(msg, "`S::Due` takes 1 arguments, found 0");
    let (msg, _) = reject_s("fn main() { S::Done(1); }");
    assert_eq!(msg, "`S::Done` takes 0 arguments, found 1");
    let (msg, at) = reject_s("fn main() { S::Due(\"a\"); }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    let (msg, _) = reject_s("fn f(s: S) {} fn main() { f(1); }");
    assert_eq!(msg, "expected S, found Int");
}

#[test]
fn enum_equality_is_accepted() {
    ok(&format!("{S}fn main() -> Bool {{ S::Done != S::Done }}"));
}

#[test]
fn match_scrutinee_and_arm_types() {
    let (msg, at) = reject_s("fn main() { match () { _ => 1 } }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot match on ()", "()"));
    let (msg, _) = reject_s("fn main() { match 1 { _ => 1, } match 1 { 1 => 1, _ => \"a\" }; }");
    assert_eq!(msg, "`match` arms have different types: Int and Str");
    ok("fn main() -> Int { match 1 { 1 => { return 1; } _ => 2 } }");
}

#[test]
fn pattern_errors() {
    let (msg, at) = reject_s("fn main() { match 1 { \"a\" => 1, _ => 1 }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    reject_s("fn main() { match true { 1 => 1, _ => 1 }; }");
    reject_s("fn main() { match 1 { S::Done => 1, _ => 1 }; }");
    reject_s("fn main() { match S::Done { T::Done => 1, _ => 1 }; }");
    let (msg, at) = reject_s("fn main() { match S::Done { S::Nope => 1, _ => 1 }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("unknown variant `S::Nope`", "S::Nope"));
    reject_s("fn main() { match S::Done { Nope(x) => 1, _ => 1 }; }");
    let (msg, at) = reject_s("fn main() { match S::Done { Pair(a) => 1, _ => 1 }; }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`S::Pair` has 2 values, found 1 bindings", "Pair(a)")
    );
    reject_s("fn main() { match S::Done { Done(x) => 1, _ => 1 }; }");
    let (msg, _) = reject_s("fn main() { match S::Done { Due(n) => n + \"a\", _ => 1 }; }");
    assert_eq!(msg, "`+` cannot be applied to Int and Str");
    reject_s("fn main() { match S::Done { Due(n) => 1, _ => n }; }");
}

#[test]
fn exhaustiveness() {
    let (msg, at) = reject_s("fn main() { match S::Done { Done => 1, Due(_) => 2 }; }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("non-exhaustive `match`: missing S::Pair", "S::Done")
    );
    let (msg, _) = reject_s("fn main() { match S::Done { Done => 1 }; }");
    assert_eq!(msg, "non-exhaustive `match`: missing S::Due, S::Pair");
    let (msg, _) = reject_s("fn main() { match true { true => 1 }; }");
    assert_eq!(msg, "non-exhaustive `match`: missing false");
    reject_s("fn main() { match 1 { 1 => 1 }; }");
    reject_s("fn main() { match \"a\" { \"a\" => 1 }; }");
    ok(&format!(
        "{S}fn main() {{ match 1 {{ x => x }}; match S::Done {{ _ => 1 }}; }}"
    ));
}

#[test]
fn unreachable_arm() {
    let (msg, at) = reject_s("fn main() { match 1 { _ => 1, 2 => 2 }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("unreachable arm", "2"));
    reject_s("fn main() { match 1 { x => 1, _ => 2 }; }");
    reject_s("fn main() { match S::Done { x => 1, Done => 2 }; }");
}

// Nested patterns and `if let` (L4a).

const MISSING: &str = "add arms for the missing cases, or `_ => ..`";

#[test]
fn nested_exhaustiveness() {
    let src = "fn f(r: Result<Int, IoErr>) -> Int { match r { Ok(n) => n, Err(IoErr::NotFound) => 0, Err(IoErr::Exists) => 1 } } fn main() {}";
    assert_rejected(
        src,
        "non-exhaustive `match`: missing Result::Err(IoErr::Denied), Result::Err(IoErr::TooLarge), Result::Err(IoErr::InvalidPath)",
        MISSING,
        "r",
    );
    let (msg, at) =
        reject("fn f(o: Option<Option<Int>>) -> Int { match o { Some(Some(x)) => x, None => 0 } } fn main() {}");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("non-exhaustive `match`: missing Option::Some(Option::None)", "o")
    );
    let (msg, _) = reject("fn f(o: Option<Bool>) -> Int { match o { Some(true) => 1, None => 0 } } fn main() {}");
    assert_eq!(msg, "non-exhaustive `match`: missing Option::Some(false)");
    let (msg, _) = reject_s(
        "fn f(s: S) -> Int { match s { Due(0) => 1, Done => 2, Pair(_, \"a\") => 3, Pair(1, _) => 4 } } fn main() {}",
    );
    assert_eq!(msg, "non-exhaustive `match`: missing S::Due");
    let p = "enum P { Two(Bool, Bool) } ";
    let (msg, _) = reject(&format!(
        "{p}fn f(p: P) -> Int {{ match p {{ Two(true, _) => 1, Two(false, true) => 2 }} }} fn main() {{}}"
    ));
    assert_eq!(msg, "non-exhaustive `match`: missing P::Two(false, false)");
    assert_eq!(
        hint(&format!(
            "{p}fn f(p: P) -> Int {{ match p {{ Two(_, true) => 1 }} }} fn main() {{}}"
        )),
        MISSING
    );
    ok(&format!("{p}fn f(p: P) -> Int {{ match p {{ Two(true, _) => 1, Two(_, true) => 2, Two(false, false) => 3 }} }} fn main() {{}}"));
    ok("fn f(o: Option<Option<Int>>) -> Int { match o { Some(Some(x)) => x, Some(None) => 1, None => 0 } } fn main() {}");
    ok("fn f(r: Result<Bool, Str>) -> Int { match r { Ok(true) => 1, Ok(false) => 2, Err(\"a\") => 3, Err(_) => 4 } } fn main() {}");
}

#[test]
fn nested_unreachable_arm() {
    let src =
        "fn f(o: Option<Option<Int>>) -> Int { match o { Some(_) => 1, Some(Some(x)) => x, None => 0 } } fn main() {}";
    assert_eq!(
        reject(src),
        ("unreachable arm".to_string(), "Some(Some(x))".to_string())
    );
    assert_eq!(hint(src), "earlier arms match every value it matches; remove it");
    let p = "enum P { Two(Bool, Bool) } ";
    let src = format!("{p}fn f(p: P) -> Int {{ match p {{ Two(true, _) => 1, Two(_, true) => 2, Two(true, true) => 3, _ => 4 }} }} fn main() {{}}");
    assert_eq!(
        reject(&src),
        ("unreachable arm".to_string(), "Two(true, true)".to_string())
    );
    let (msg, _) = reject_s("fn main() { match S::Due(1) { Due(1) => 1, Due(1) => 2, _ => 3 }; }");
    assert_eq!(msg, "unreachable arm");
    let (msg, _) = reject(
        "fn f(r: Result<Int, Str>) -> Int { match r { Ok(_) => 1, Err(_) => 2, Err(\"a\") => 3 } } fn main() {}",
    );
    assert_eq!(msg, "unreachable arm");
}

#[test]
fn nested_pattern_bindings() {
    ok("fn f(r: Result<Option<Str>, Int>) -> Int { match r { Ok(Some(s)) => s.len(), Ok(None) => 0, Err(n) => n } } fn main() {}");
    let (msg, _) =
        reject("fn f(o: Option<Option<Int>>) -> Int { match o { Some(Some(x)) => x + \"a\", _ => 0 } } fn main() {}");
    assert_eq!(msg, "`+` cannot be applied to Int and Str");
    let (msg, at) = reject("fn f(o: Option<Int>) -> Int { match o { Some(\"a\") => 1, _ => 0 } } fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    let (msg, at) = reject_s("fn f(o: Option<S>) -> Int { match o { Some(Pair(x, x)) => 1, _ => 0 } } fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("`x` is bound twice in this pattern", "x"));
    assert_eq!(
        hint(&format!(
            "{S}fn f(o: Option<S>) -> Int {{ match o {{ Some(Pair(x, x)) => 1, _ => 0 }} }} fn main() {{}}"
        )),
        "rename one of them"
    );
    let (msg, _) = reject_s("fn f(o: Option<S>) -> Int { match o { Some(Pair(a)) => 1, _ => 0 } } fn main() {}");
    assert_eq!(msg, "`S::Pair` has 2 values, found 1 bindings");
    let src = "fn f(o: Option<Int>) -> Int { match o { Some(Some(x)) => x, _ => 0 } } fn main() {}";
    assert_eq!(
        reject(src),
        ("expected Int, found variant `Some`".to_string(), "Some(x)".to_string())
    );
    assert_eq!(hint(src), "match Int with a name or `_`");
}

#[test]
fn if_let_checks() {
    ok("fn f(o: Option<Int>) -> Int { if let Some(x) = o { x } else { 0 } } fn main() {}");
    ok("fn g(x: Int) {} fn f(o: Option<Int>) { if let Some(x) = o { g(x); } } fn main() {}");
    ok("fn f(o: Option<Int>, r: Result<Int, Str>, b: Bool) -> Int { if let Some(x) = o { x } else if let Ok(y) = r { y } else if b { 1 } else { 2 } } fn main() {}");
    ok("fn f() -> Int { if let x = 1 { x } else { 0 } } fn main() {}");
    let (msg, _) = reject("fn f(o: Option<Int>) { if let Some(x) = o { x } } fn main() {}");
    assert_eq!(msg, "expected (), found Int");
    let (msg, at) = reject("fn f(o: Option<Int>) -> Int { if let Some(x) = o { x } else { x } } fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("unknown name `x`", "x"));
    let (msg, _) = reject("fn f(o: Option<Int>) -> Int { if let Some(x) = o { x } else { \"a\" } } fn main() {}");
    assert_eq!(msg, "`if` branches have different types: Int and Str");
    let (msg, at) = reject("fn main() { if let x = [1] {} }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot match on List<Int>", "[1]"));
    let (msg, _) = reject("fn f(o: Option<Int>) { if let Ok(x) = o {} } fn main() {}");
    assert_eq!(msg, "unknown variant `Option::Ok`");
}

#[test]
fn too_complex_match() {
    let arms: String = (0..3000).map(|i| format!("{i} => 0, ")).collect();
    let src = format!("fn f(n: Int) -> Int {{ match n {{ {arms}_ => 1 }} }} fn main() {{}}");
    let diags = check(&src, &CapTable::default()).expect_err("should be rejected");
    assert_eq!(diags[0].msg, "this `match` is too complex to check");
    assert_eq!(diags[0].hint.as_deref(), Some("split it into smaller matches"));
    let arms: String = (0..500).map(|i| format!("{i} => 0, ")).collect();
    ok(&format!(
        "fn f(n: Int) -> Int {{ match n {{ {arms}_ => 1 }} }} fn main() {{}}"
    ));
    // A payload too wide for the depth limit is refused, not a stack overflow.
    let n = 2000;
    let tys = vec!["Int"; n].join(", ");
    let ones = vec!["1"; n].join(", ");
    let src = format!("enum W {{ V({tys}) }} fn f(w: W) -> Int {{ match w {{ V({ones}) => 1, V({ones}) => 2, _ => 3 }} }} fn main() {{}}");
    assert_eq!(
        check(&src, &CapTable::default()).unwrap_err()[0].msg,
        "this `match` is too complex to check"
    );
}

/// The exhaustiveness budget is per program: many matches each under the per-match cap stop at
/// the program's, quickly, rather than costing the host minutes (C5 review).
#[test]
fn many_large_matches_spend_the_program_budget() {
    let arms: String = (0..1800).map(|i| format!("{i} => 0, ")).collect();
    let one = format!("fn f(n: Int) -> Int {{ match n {{ {arms}_ => 1 }} }} fn main() {{}}");
    ok(&one);
    let fns: String = (0..40)
        .map(|k| format!("fn f{k}(n: Int) -> Int {{ match n {{ {arms}_ => 1 }} }} "))
        .collect();
    let start = std::time::Instant::now();
    let diags = check(&format!("{fns}fn main() {{}}"), &CapTable::default()).expect_err("should be rejected");
    assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
    assert_eq!(diags[0].msg, "this program's matches are too complex to check");
    assert_eq!(
        diags[0].hint.as_deref(),
        Some("simplify the largest matches; together they exceed what the checker will do")
    );
}

/// The hint of the single diagnostic for `src`.
#[track_caller]
fn hint(src: &str) -> String {
    let diags = check(src, &CapTable::default()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    diags[0].hint.clone().unwrap_or_else(|| panic!("no hint for {src}"))
}

#[track_caller]
fn no_hint(src: &str) {
    let diags = check(src, &CapTable::default()).expect_err("should be rejected");
    assert_eq!(diags[0].hint, None, "{src}");
}

#[test]
fn closest_picks_near_names() {
    assert_eq!(closest("lenght", &["length", "width"]), Some("length"));
    assert_eq!(closest("x", &["y"]), Some("y"));
    assert_eq!(closest("count", &["total", "sum"]), None);
    assert_eq!(closest("abcdef", &["abcxyz"]), None);
    assert_eq!(closest("abcdefghi", &["abcdefxyz"]), Some("abcdefxyz"));
    assert_eq!(closest("a", &[]), None);
    assert_eq!(closest("é", &["e"]), Some("e"));
    assert_eq!(closest("cuont", &["count", "total"]), Some("count"));
}

#[test]
fn hint_for_trailing_semicolon() {
    let src = "fn total() -> Int { let a = 1; a + 2; } fn main() {}";
    assert_eq!(hint(src), "remove the trailing `;` to return the value");
}

#[test]
fn hints_for_unknown_things() {
    assert_eq!(
        hint("fn main() -> Int { let total = 1; totl }"),
        "did you mean `total`?"
    );
    assert_eq!(hint("fn main() -> Int { zzzzzz }"), "declare it with `let`");
    assert_eq!(hint("fn main() { let x = 1; xx = 2; }"), "did you mean `x`?");
    assert_eq!(hint("fn helper() {} fn main() { helpr(); }"), "did you mean `helper`?");
    assert_eq!(hint("fn alpha() {} fn main() { zzzzzz(); }"), "functions: alpha, main");
    let p = "struct Point { x: Int } ";
    assert_eq!(
        hint(&format!("{p}fn main() {{ Pint {{ x: 1 }}; }}")),
        "did you mean `Point`?"
    );
    assert_eq!(
        hint(&format!("{p}fn main() {{ Zzzzzz {{ x: 1 }}; }}")),
        "structs: Point"
    );
    assert_eq!(hint(&format!("{p}fn main() {{ Point {{ yyyyyy: 1 }}; }}")), "fields: x");
    assert_eq!(
        hint(&format!("{p}fn main() {{ Point {{ xx: 1 }}; }}")),
        "did you mean `x`?"
    );
    assert_eq!(hint("fn main() { 1.f; }"), "only structs have fields");
    let e = "enum Color { Red, Green } ";
    assert_eq!(
        hint(&format!("{e}fn main() {{ Color::Gren; }}")),
        "did you mean `Green`?"
    );
    assert_eq!(
        hint(&format!("{e}fn main() {{ Color::Blue; }}")),
        "variants: Red, Green"
    );
    assert_eq!(hint(&format!("{e}fn main() {{ Colr::Red; }}")), "did you mean `Color`?");
    let m = "fn main() { match Color::Red { Blue(x) => 1, _ => 2 }; }";
    assert_eq!(hint(&format!("{e}{m}")), "variants: Red, Green");
}

#[test]
fn hints_for_duplicates() {
    for src in [
        "fn f() {} fn f() {} fn main() {}",
        "fn f(a: Int, a: Int) {} fn main() {}",
        "struct A { x: Int, x: Int } fn main() {}",
        "enum A { X, X } fn main() {}",
        "struct A {} enum A {} fn main() {}",
    ] {
        assert_eq!(hint(src), "rename one of them", "{src}");
    }
    let src = "struct Q { c: Int } fn main() { Q { c: 1, c: 2 }; }";
    assert_eq!(hint(src), "remove one of them");
}

#[test]
fn hint_for_stray_semicolon() {
    let stray = "remove the trailing `;` to return the value";
    assert_eq!(hint("fn main() { let x: Int = { 1; }; }"), stray);
    assert_eq!(hint("fn f(a: Int) {} fn main() { f({ 1; }); }"), stray);
    assert_eq!(
        hint("fn main() -> Int { if true { 1; } else { 2 } }"),
        "every branch must have the same type"
    );
    no_hint("fn main() { let x: Int = { let y = 1; }; }");
    no_hint("fn main() { let x: Int = { }; }");
    no_hint("fn main() { let x: Int = true; }");
    no_hint("fn f() {} fn main() { let x: Int = f(); }");
}

#[test]
fn hints_for_calls_and_operators() {
    let f = "fn f(a: Int, b: Str) {} ";
    assert_eq!(
        hint(&format!("{f}fn main() {{ f(1); }}")),
        "`f` is declared as `fn f(a: Int, b: Str)`"
    );
    assert_eq!(hint("fn g() {} fn main() { g(1); }"), "`g` is declared as `fn g()`");
    let s = "enum S { Done, Pair(Int, Str) } ";
    assert_eq!(
        hint(&format!("{s}fn main() {{ S::Pair(1); }}")),
        "`S::Pair` holds (Int, Str)"
    );
    assert_eq!(hint(&format!("{s}fn main() {{ S::Done(1); }}")), "`S::Done` holds ()");
    let pat = "fn main() { match S::Done { Pair(a) => 1, _ => 2 }; }";
    assert_eq!(hint(&format!("{s}{pat}")), "`S::Pair` holds (Int, Str)");
    assert_eq!(
        hint("fn main() -> Int { 1 + true }"),
        "`+` works on two Int, two Float or two Str"
    );
    assert_eq!(
        hint("fn main() -> Int { 1 - \"a\" }"),
        "`-` works on two Int or two Float"
    );
    assert_eq!(
        hint("fn main() -> Bool { \"a\" < 1 }"),
        "`<` works on two Int, two Float or two Str"
    );
    assert_eq!(
        hint("fn main() -> Bool { [\"a\"] < [\"b\"] }"),
        "`<` works on two Int, two Float or two Str"
    );
    assert_eq!(hint("fn main() -> Bool { 1 && 2 }"), "`&&` works on two Bool");
    assert_eq!(
        hint("fn main() -> Bool { 1 == true }"),
        "`==` works on two values of one type that holds no capability"
    );
    assert_eq!(
        hint("fn main() { let mut x = true; x += 1; }"),
        "`+` works on two Int, two Float or two Str"
    );
    assert_eq!(
        hint("fn main() { 1(2); }"),
        "only functions and values of `fn` type can be called"
    );
}

#[test]
fn hints_for_declarations_and_match() {
    assert_eq!(hint("fn f() {}"), "add `fn main() { .. }`");
    assert_eq!(
        hint("fn main(a: Int) {}"),
        "`main` parameters must be capabilities, like `input: Dir`"
    );
    let q = "struct P { a: Int, b: Int } ";
    assert_eq!(hint(&format!("{q}fn main() {{ P {{ a: 1 }}; }}")), "add `b: ..`");
    assert_eq!(
        hint("fn main() { match () { _ => 1 }; }"),
        "match works on Int, Bool, Str and enums"
    );
    let cases = "add arms for the missing cases, or `_ => ..`";
    assert_eq!(hint("fn main() { match 1 { 1 => 1 }; }"), cases);
    let diff = "fn main() { match 1 { 1 => 1, _ => \"a\" }; }";
    assert_eq!(hint(diff), "every branch must have the same type");
    assert_eq!(
        hint("fn main() { if true { 1 } else { \"a\" }; }"),
        "every branch must have the same type"
    );
}

#[test]
fn option_and_result_types() {
    ok("fn f(x: Option<Int>, y: Result<(), Str>) -> Option<Option<Int>> { Some(x) } fn main() {}");
    ok("fn main() { let x: Option<Int> = None; let y: Result<Int, Str> = Err(\"a\"); let z = Some(1); }");
    ok("fn f() -> Option<Int> { return None; } fn g() -> Result<(), Str> { Ok(()) } fn main() {}");
    ok("fn f(b: Bool) -> Option<Int> { if b { None } else { Some(1) } } fn main() {}");
    ok("struct S { o: Option<Int> } fn main() { let s = S { o: None }; }");
    ok("enum E { A(Option<Int>) } fn main() { let e = E::A(None); }");
    ok("fn f(o: Option<Int>) {} fn main() { f(None); f(Some(1)); }");
    let (msg, at) = reject("fn f(x: Option<Int, Int>) {} fn main() {}");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`Option` takes 1 type arguments, found 2", "Option<Int, Int>")
    );
    assert_eq!(hint("fn f(x: Option<Int, Int>) {} fn main() {}"), "write `Option<T>`");
    assert_eq!(hint("fn f(x: Result<Int>) {} fn main() {}"), "write `Result<T, E>`");
    let (msg, _) = reject("fn f(x: Option) {} fn main() {}");
    assert_eq!(msg, "`Option` takes 1 type arguments, found 0");
    let (msg, _) = reject("struct Option { a: Int } fn main() {}");
    assert_eq!(msg, "`Option` is a built-in type");
    assert_eq!(hint("enum Result { A } fn main() {}"), "rename your type");
    let (msg, _) = reject("fn main() { let x: Option<Int> = Some(\"a\"); }");
    assert_eq!(msg, "expected Int, found Str");
    let (msg, _) = reject("fn main() { let x: Option<Int> = Some(1) == Some(1); }");
    assert_eq!(msg, "expected Option<Int>, found Bool");
}

#[test]
fn constructors_need_a_type() {
    for (src, tip) in [
        (
            "fn main() { let x = None; }",
            "add a type: `let x: Option<Int> = None;`",
        ),
        (
            "fn main() { let x = Ok(1); }",
            "add a type: `let x: Result<Int, Str> = Ok(1);`",
        ),
        (
            "fn main() { let x = Err(1); }",
            "add a type: `let x: Result<Int, Str> = Err(\"no\");`",
        ),
    ] {
        assert_eq!(hint(src), tip, "{src}");
    }
    let (msg, _) = reject("fn main() { let x: Result<Int, Int> = None; }");
    assert_eq!(msg, "expected Result<Int, Int>, found an Option");
    let (msg, at) = reject("fn main() { let x = None; }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot tell the type of `None`", "None"));
    assert_eq!(
        hint("fn main() { let x: Option<Int> = Some(); }"),
        "write `Some(value)`"
    );
    assert_eq!(
        hint("fn main() { let x: Option<Int> = None(1); }"),
        "write `None` without arguments"
    );
    assert_eq!(
        hint("fn main() { Option::Some(1); }"),
        "write `Some` without `Option::`"
    );
    assert_eq!(hint("fn main() { Option::Soem(1); }"), "did you mean `Some`?");
}

#[test]
fn try_operator() {
    ok("fn a() -> Option<Int> { Some(1) } fn b() -> Option<Int> { let x = a()?; Some(x + 1) } fn main() {}");
    ok("fn a() -> Result<Int, Str> { Ok(1) } fn b() -> Result<(), Str> { a()?; Ok(()) } fn main() {}");
    let src = "fn a() -> Result<Int, Str> { Ok(1) } fn b() -> Option<Int> { Some(a()?) } fn main() {}";
    let (msg, at) = reject(src);
    assert_eq!(msg, "`?` on Result<Int, Str> in a function returning Option<Int>");
    assert_eq!(at, "a()?");
    assert_eq!(hint(src), "change the return type or `match` on the result");
    let src = "fn a() -> Result<Int, Str> { Ok(1) } fn b() -> Result<Int, Int> { Ok(a()?) } fn main() {}";
    let (msg, _) = reject(src);
    assert_eq!(msg, "`?` on Result<Int, Str> in a function returning Result<Int, Int>");
    assert_eq!(
        hint(src),
        "change the return type, convert the error with `.map_err(f)?`, or `match` on the result"
    );
    let (msg, at) = reject("fn f() -> Option<Int> { 1? } fn main() {}");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`?` needs an Option or Result, found Int", "1")
    );
    assert_eq!(
        hint("fn f() -> Option<Int> { 1? } fn main() {}"),
        "`?` works on Option and Result values"
    );
    let (msg, _) = reject("fn f() -> Int { Some(1)? } fn main() {}");
    assert_eq!(msg, "`?` on Option<Int> in a function returning Int");
}

#[test]
fn match_on_option_and_result() {
    ok("fn f(o: Option<Int>) -> Int { match o { Some(x) => x, None => 0 } } fn main() {}");
    ok("fn f(o: Option<Int>) -> Int { match o { Option::Some(x) => x, Option::None => 0 } } fn main() {}");
    ok("fn f(r: Result<Int, Str>) -> Int { match r { Ok(x) => x, Err(_) => 0 } } fn main() {}");
    ok("fn f(r: Result<Int, Str>) -> Int { match r { Result::Ok(x) => x, _ => 0 } } fn main() {}");
    let (msg, _) = reject("fn f(o: Option<Int>) -> Int { match o { Some(x) => x } } fn main() {}");
    assert_eq!(msg, "non-exhaustive `match`: missing Option::None");
    let src = "fn f(o: Option<Int>) -> Int { match o { Some(x) => \"a\", None => 0 } } fn main() {}";
    assert_eq!(reject(src).0, "`match` arms have different types: Str and Int");
    let (msg, _) = reject("fn f(o: Option<Int>) -> Int { match o { Ok(x) => x, _ => 0 } } fn main() {}");
    assert_eq!(msg, "unknown variant `Option::Ok`");
    assert_eq!(
        hint("fn f(o: Option<Int>) -> Int { match o { Ok(x) => x, _ => 0 } }  fn main() {}"),
        "variants: None, Some"
    );
    let (msg, _) = reject("fn f(o: Option<Int>) -> Int { match o { Some(a, b) => a, _ => 0 } } fn main() {}");
    assert_eq!(msg, "`Option::Some` has 1 values, found 2 bindings");
    assert_eq!(
        hint("fn f(o: Option<Int>) -> Int { match o { Some(a, b) => a, _ => 0 } } fn main() {}"),
        "`Option::Some` holds (Int)"
    );
    let (msg, _) = reject("fn f(o: Option<Int>) -> Int { match o { Result::Ok(x) => x, _ => 0 } } fn main() {}");
    assert_eq!(msg, "expected Option<Int>, found Result");
}

#[test]
fn constructor_against_wrong_expected_type() {
    let (msg, _) = reject("fn f() -> Option<Int> { Ok(1) } fn main() {}");
    assert_eq!(msg, "expected Option<Int>, found a Result");
    let (msg, _) = reject("fn main() -> Int { let x: Int = None; x }");
    assert_eq!(msg, "expected Int, found an Option");
    assert_eq!(
        hint("fn main() { let x = Ok(1); }"),
        "add a type: `let x: Result<Int, Str> = Ok(1);`"
    );
}

#[test]
fn methods() {
    ok("fn main() -> Bool { 1.to_str().to_bytes().to_str().len() == 1 && \"\".to_bytes() == \"\".to_bytes() }");
    let (msg, at) = reject("fn main() { \"a\".size(); }");
    assert_eq!((msg.as_str(), at.as_str()), ("no method `size` on Str", "\"a\".size()"));
    assert_eq!(hint("fn main() { \"a\".lenn(); }"), "did you mean `len`?");
    assert_eq!(
        hint("fn main() { \"a\".zzzzzz(); }"),
        "Str has: contains, ct_eq, ends_with, find, len, lines, parse_float, parse_int, parse_json, replace, sha256, slice, split, starts_with, strip_prefix, strip_suffix, to_bytes, to_lowercase, to_uppercase, trim"
    );
    ok("fn main() { let a: List<Str> = \"a\".split(\",\"); let b: List<Str> = \"a\".lines(); let c: Str = \"a\".trim(); let d: Bool = \"a\".contains(\"a\") && \"a\".starts_with(\"a\"); let e: Option<Int> = \"1\".parse_int(); }");
    assert_eq!(reject("fn main() { \"a\".split(1); }").0, "expected Str, found Int");
    assert_eq!(hint("fn main() { 1.zzzzzz(); }"), "Int has: max, min, to_float, to_str");
    assert_eq!(hint("fn main() { true.len(); }"), "Bool has no methods");
    let src = "fn main() { \"a\".len(1); }";
    assert_eq!(reject(src).0, "`len` takes 0 arguments, found 1");
    assert_eq!(hint(src), "`Str.len` is declared as `len() -> Int`");
    let (msg, _) = reject("fn main() -> Str { \"a\".len() }");
    assert_eq!(msg, "expected Str, found Int");
    reject("fn main() { let b = \"a\".to_bytes(); match b { _ => {} } }");
    assert_eq!(
        hint("fn main() { let b = \"a\".to_bytes(); match b { _ => {} } }"),
        "match works on Int, Bool, Str and enums"
    );
}

// Capabilities (task 8).

/// Rejects `src` with the capability table and returns message, hint and the source it points at.
fn reject_cap(src: &str) -> (String, String, String) {
    let diags = check(src, &caps()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    (
        d.msg.clone(),
        d.hint.clone().unwrap_or_default(),
        src[d.span.start..d.span.end].to_string(),
    )
}

fn assert_rejected(src: &str, msg: &str, hint: &str, at: &str) {
    assert_eq!(
        reject_cap(src),
        (msg.to_string(), hint.to_string(), at.to_string()),
        "{src}"
    );
}

const ONLY_PARAMS: &str = "capabilities are parameters only";
const AS_VALUE: &str = "call methods on the parameter itself";

#[test]
fn accepts_capabilities_passed_to_helpers() {
    let src = "fn load(d: Dir, name: Str) -> Result<Bytes, IoErr> { d.read(name) }
        fn save(d: Dir, b: Bytes) -> Result<(), IoErr> { d.create(\"o\", b) }
        fn main(input: Dir, out: Dir) -> Result<(), IoErr> {
            let data = load(input, \"in\")?;
            save(out, data)
        }";
    check(src, &caps()).unwrap_or_else(|d| panic!("{d:?}"));
    let src = "fn leaf(d: Dir) -> Result<Bytes, IoErr> { d.read(\"x\") }
        fn mid(d: Dir) -> Result<Bytes, IoErr> { leaf(d) }
        fn main(input: Dir, net: Http) -> Result<(), HttpErr> {
            let a = mid(input);
            net.get(\"u\")?;
            Ok(())
        }";
    check(src, &caps()).unwrap_or_else(|d| panic!("{d:?}"));
}

#[test]
fn rule1_local() {
    assert_rejected(
        "fn main(input: Dir) { let d = input; }",
        "capability `input` bound to local `d`",
        "pass `input` directly instead",
        "input",
    );
}

#[test]
fn rule1_field_and_payload() {
    assert_rejected(
        "struct Job { dir: Dir } fn main() {}",
        "capability type in a field",
        ONLY_PARAMS,
        "Dir",
    );
    assert_rejected(
        "enum E { A(Dir) } fn main() {}",
        "capability type in a payload",
        ONLY_PARAMS,
        "Dir",
    );
}

#[test]
fn rule1_return() {
    assert_rejected(
        "fn pick(a: Dir) -> Dir { a } fn main() {}",
        "cannot return `Dir` from `pick`",
        ONLY_PARAMS,
        "Dir",
    );
}

#[test]
fn rule1_nested_type() {
    assert_rejected(
        "fn f(x: Option<Dir>) {} fn main() {}",
        "capability nested in `Option<Dir>`",
        "take `Dir` as a parameter",
        "Option<Dir>",
    );
}

#[test]
fn rule1_expression() {
    let value = "capability used as a value";
    assert_rejected(
        "fn main(a: Dir, b: Dir) { (if true { a } else { b }).read(\"x\"); }",
        value,
        AS_VALUE,
        "a",
    );
    assert_rejected("fn main(d: Dir) { Some(d); }", value, AS_VALUE, "d");
    assert_rejected("fn main(d: Dir) { let x = Some(d); }", value, AS_VALUE, "d");
}

#[test]
fn rule1_adversarial() {
    let value = "capability used as a value";
    // Struct literal field, match arm, block tail, `?`, return, statement, scrutinee, method argument.
    for body in [
        "let j = Job { n: d };",
        "let x = match 1 { _ => d };",
        "let x = { d };",
        "let x = d?;",
        "return d;",
        "d;",
        "match d { _ => {} }",
        "d.create(\"p\", d);",
        "let x = -d;",
        "if d { }",
        "let x: Dir = d;",
    ] {
        let src = format!("struct Job {{ n: Int }} fn main(d: Dir) {{ {body} }}");
        let (msg, hint, _) = reject_cap(&src);
        let local = msg.starts_with("capability `d` bound");
        assert!(
            local || msg == value || msg.starts_with("cannot return"),
            "{src}: {msg} / {hint}"
        );
    }
    // A local cannot be declared with a capability type, even from a non-capability expression.
    assert_rejected(
        "fn main() { let x: Option<Dir> = None; }",
        "capability nested in `Option<Dir>`",
        "take `Dir` as a parameter",
        "Option<Dir>",
    );
    assert_rejected(
        "fn main() { let x: Dir = 1; }",
        "capability type in a local",
        ONLY_PARAMS,
        "Dir",
    );
    // A capability parameter is never `mut`.
    reject_cap("fn f(d: Dir, e: Dir) { d = e; } fn main() {}");
    // Passing a capability to the wrong parameter type.
    assert_rejected(
        "fn f(h: Http) {} fn main(d: Dir) { f(d); }",
        "expected Http, found Dir",
        "",
        "d",
    );
}

#[test]
fn rule2_no_construction() {
    let hint = "capabilities come only as parameters of `main`";
    assert_rejected("fn main() { let d = Dir; }", "unknown name `Dir`", hint, "Dir");
    assert_rejected(
        "fn main() { let d = Dir::new(); }",
        "no ambient Dir access: `Dir::new` does not exist",
        hint,
        "Dir::new()",
    );
    assert_rejected("fn main() { let d = Dir {}; }", "unknown struct `Dir`", hint, "Dir {}");
}

#[test]
fn rule3_main_parameters() {
    assert_rejected(
        "fn main(n: Int) {}",
        "`main` parameter `n` is not a capability",
        "`main` parameters must be capabilities, like `input: Dir`",
        "n: Int",
    );
    assert_rejected(
        "fn main(o: Option<Dir>) {}",
        "capability nested in `Option<Dir>`",
        "take `Dir` as a parameter",
        "Option<Dir>",
    );
    check("fn main(a: Dir, b: Http) {}", &caps()).unwrap_or_else(|d| panic!("{d:?}"));
}

#[test]
fn capability_methods_are_typed() {
    let (msg, hint, _) = reject_cap("fn main(d: Dir) { d.delete(\"x\"); }");
    assert_eq!(
        (msg.as_str(), hint.as_str()),
        (
            "no method `delete` on Dir",
            "Dir has: read, create, list, write, remove, exists"
        )
    );
    let (msg, hint, _) = reject_cap("fn main(d: Dir) { d.read(); }");
    assert_eq!(msg, "`read` takes 1 arguments, found 0");
    assert_eq!(hint, "`Dir.read` is declared as `read(Str) -> Result<Bytes, IoErr>`");
    assert_rejected("fn main(d: Dir) { d.read(1); }", "expected Str, found Int", "", "1");
}

#[test]
fn builtin_error_enums_are_reserved_and_usable() {
    let (msg, hint, _) = reject_cap("enum IoErr { A } fn main() {}");
    assert_eq!(
        (msg.as_str(), hint.as_str()),
        ("`IoErr` is a built-in type", "rename your type")
    );
    reject_cap("struct Dir {} fn main() {}");
    let src = "fn main(d: Dir) { match d.read(\"x\") { Ok(b) => {}, Err(e) => {} } }";
    check(src, &caps()).unwrap_or_else(|d| panic!("{d:?}"));
}

#[test]
fn builtin_constructor_names_cannot_be_bound() {
    let (msg, _) = reject("fn f(Some: Int) {} fn main() {}");
    assert_eq!(msg, "`Some` is a built-in constructor");
    assert_eq!(hint("fn main() { let None = 1; }"), "choose another name");
}

#[test]
fn capabilities_cannot_hide_in_payloads() {
    for src in [
        "enum E { A(Int) } fn f(d: Dir) { let x = E::A(d); } fn main() {}",
        "fn f(d: Dir) -> Result<Int, Int> { Ok(d) } fn main() {}",
        "fn f(d: Dir) -> Result<Int, Int> { Err(d) } fn main() {}",
    ] {
        let (msg, hint, at) = reject_cap(src);
        assert_eq!(
            (msg.as_str(), hint.as_str(), at.as_str()),
            (
                "capability used as a value",
                "call methods on the parameter itself",
                "d"
            ),
            "{src}"
        );
    }
}

#[test]
fn list_types_and_literals() {
    ok("fn f(xs: List<List<Int>>) -> Int { xs[0][1] + xs.len() } fn main() {}");
    ok("fn main() { let a = [1, 2]; let b: List<Int> = []; let c: List<Option<Int>> = [None, Some(1)]; }");
    ok("fn main() -> List<Str> { [] }");
    let (msg, at) = reject("fn main() { let x = []; }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot tell the type of `[]`", "[]"));
    assert_eq!(
        hint("fn main() { let x = []; }"),
        "add a type: `let xs: List<Int> = [];`"
    );
    let (msg, at) = reject("fn main() { let x = [1, \"a\"]; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    let (msg, _) = reject("fn main() { let x: List<Int> = [true]; }");
    assert_eq!(msg, "expected Int, found Bool");
    assert_eq!(hint("fn f(x: List<Int, Int>) {} fn main() {}"), "write `List<T>`");
    assert!(hint("fn f(x: Foo) {} fn main() {}").contains("List"));
}

#[test]
fn list_indexing() {
    let (msg, at) = reject("fn main() { let x = [1]; x[true]; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Bool", "true"));
    let (msg, at) = reject("fn main() { let x = 1; x[0]; }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot index Int", "x[0]"));
    assert_eq!(
        hint("fn main() { let x = 1; x[0]; }"),
        "only a List or Bytes can be indexed"
    );
    reject("fn main() { let x: Int = [1][0] + [\"a\"][0]; }");
    ok("fn main() { let x = [1, 2]; x == x; }");
}

#[test]
fn list_places() {
    let s = "struct S { items: List<Int> } ";
    ok(&format!(
        "{s}fn main() {{ let mut xs = [S {{ items: [1] }}]; xs[0].items[0] = 2; xs[0].items[0] += 1; }}"
    ));
    let (msg, at) = reject("fn main() { let xs = [1]; xs[0] = 2; }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("cannot assign to `xs`, it is not `mut`", "xs")
    );
    assert_eq!(
        hint("fn main() { let xs = [1]; xs[0] = 2; }"),
        "declare it with `let mut`"
    );
    let (msg, _) = reject("fn main() { let mut xs = [1]; xs[0] = \"a\"; }");
    assert_eq!(msg, "expected Int, found Str");
    reject("fn main() { let mut xs = [1]; xs[true] = 2; }");
    reject("fn main() { let mut x = 1; x[0] = 2; }");
}

#[test]
fn list_methods() {
    ok("fn main() { let mut xs: List<Int> = []; xs.push(1); let n: Int = xs.len(); }");
    ok("struct S { items: List<Int> } fn main() { let mut s = S { items: [] }; s.items.push(1); }");
    ok("fn main() { let mut xs = [[1]]; xs[0].push(2); xs.push([]); }");
    let (msg, at) = reject("fn f() -> List<Int> { [1] } fn main() { f().push(1); }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot `push` to this expression", "f()"));
    assert_eq!(
        hint("fn f() -> List<Int> { [1] } fn main() { f().push(1); }"),
        "`push` changes a list; call it on a `let mut` variable"
    );
    let (msg, at) = reject("fn main() { let xs = [1]; xs.push(2); }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("cannot `push` to `xs`, it is not `mut`", "xs")
    );
    assert_eq!(
        hint("fn main() { let xs = [1]; xs.push(2); }"),
        "declare it with `let mut`"
    );
    let (msg, at) = reject("fn main() { let mut xs = [1]; xs.push(\"a\"); }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    assert_eq!(
        hint("fn main() { let mut xs = [1]; xs.pusj(2); }"),
        "did you mean `push`?"
    );
    reject("fn main() { let mut xs = [1]; xs.push(); }");
}

#[test]
fn builtins_l4b() {
    ok("fn main() { let s = \"ab\"; let a: Bool = s.ends_with(\"b\"); let b: Option<Str> = s.strip_prefix(\"a\"); let c: Option<Str> = s.strip_suffix(\"b\"); let d: Option<Int> = s.find(\"b\"); let e: Str = s.replace(\"a\", \"c\"); }");
    assert_eq!(
        reject("fn main() { \"a\".replace(\"a\"); }").0,
        "`replace` takes 2 arguments, found 1"
    );
    assert_eq!(
        hint("fn main() { \"a\".replace(\"a\"); }"),
        "`Str.replace` is declared as `replace(Str, Str) -> Str`"
    );
    ok("fn main() { let mut xs = [3, 1]; xs.sort(); let p: Option<Int> = xs.pop(); let c: Bool = xs.contains(1); let mut ys = [\"b\"]; ys.sort(); }");
    ok("struct P { x: Int } fn main() { let xs = [P { x: 1 }]; let c: Bool = xs.contains(P { x: 1 }); }");
    assert_eq!(
        reject("fn main() { [1].contains(\"a\"); }").0,
        "expected Int, found Str"
    );
    let (msg, at) = reject("fn main() { let mut xs = [true]; xs.sort(); }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot sort List<Bool>", "xs.sort()"));
    assert_eq!(
        hint("fn main() { let mut xs = [true]; xs.sort(); }"),
        "sort works on List<Int> and List<Str>"
    );
    let src = "fn f() -> List<Int> { [1] } fn main() { f().sort(); }";
    assert_eq!(reject(src).0, "cannot `sort` this expression");
    assert_eq!(hint(src), "`sort` changes a list; call it on a `let mut` variable");
    let src = "fn f() -> List<Int> { [1] } fn main() { f().pop(); }";
    assert_eq!(reject(src).0, "cannot `pop` from this expression");
    assert_eq!(hint(src), "`pop` changes a list; call it on a `let mut` variable");
    let src = "fn main() { let xs = [1]; xs.pop(); }";
    assert_eq!(reject(src).0, "cannot `pop` from `xs`, it is not `mut`");
    assert_eq!(hint(src), "declare it with `let mut`");
    assert_eq!(
        reject("fn main() { let xs = [1]; xs.sort(); }").0,
        "cannot `sort` `xs`, it is not `mut`"
    );
    assert_eq!(
        reject("fn main() { let mut xs = [1]; xs.pop(1); }").0,
        "`pop` takes 0 arguments, found 1"
    );
    assert_eq!(
        hint("fn main() { let mut xs = [1]; xs.zzzzzz(); }"),
        "List<Int> has: all, any, contains, filter, join, len, map, pop, push, reverse, slice, sort, sort_by, sort_by_key"
    );
}

#[test]
fn lists_cannot_hold_capabilities() {
    let (msg, hint, at) = reject_cap("fn f(xs: List<Dir>) {} fn main(d: Dir) {}");
    assert_eq!(
        (msg.as_str(), hint.as_str(), at.as_str()),
        (
            "capability nested in `List<Dir>`",
            "take `Dir` as a parameter",
            "List<Dir>"
        )
    );
    let (msg, _, _) = reject_cap("fn main(d: Dir) { let xs: List<Dir> = []; }");
    assert_eq!(msg, "capability nested in `List<Dir>`");
    let (msg, hint, at) = reject_cap("fn main(input: Dir, out: Dir) { let dirs = [input, out]; }");
    assert_eq!(
        (msg.as_str(), hint.as_str(), at.as_str()),
        (
            "capability used as a value",
            "call methods on the parameter itself",
            "input"
        )
    );
}

#[test]
fn map_types_and_new() {
    ok("fn f(m: Map<Str, List<Int>>) -> Int { m.len() } fn main() { let m: Map<Int, Int> = Map::new(); }");
    ok("fn main() -> Map<Str, Int> { Map::new() }");
    let (msg, at) = reject("fn main() { let m = Map::new(); }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("cannot tell the type of `Map::new()`", "Map::new()")
    );
    assert_eq!(
        hint("fn main() { let m = Map::new(); }"),
        "add a type: `let m: Map<Str, Int> = Map::new();`"
    );
    let (msg, at) = reject("fn main() { let x: Int = Map::new(); }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found a Map", "Map::new()"));
    assert_eq!(
        hint("fn main() { let x: Int = Map::new(); }"),
        "`Map::new()` makes a `Map<K, V>`"
    );
    let src = "fn main() { let m: Map<Str, Int> = Map::new(1); }";
    assert_eq!(reject(src).0, "`Map::new` takes 0 arguments, found 1");
    assert_eq!(hint(src), "write `Map::new()`");
    for src in ["fn main() { Map::get(1); }", "fn main() { let x = Map::new; }"] {
        assert_eq!(hint(src), "the only Map function is `Map::new()`", "{src}");
    }
    assert_eq!(hint("fn f(m: Map<Int>) {} fn main() {}"), "write `Map<K, V>`");
    assert!(hint("fn f(x: Foo) {} fn main() {}").contains("Map"));
    assert_eq!(
        reject("struct Map { a: Int } fn main() {}").0,
        "`Map` is a built-in type"
    );
}

#[test]
fn map_keys_must_be_int_or_str() {
    for src in [
        "fn f(m: Map<Bool, Int>) {} fn main() {}",
        "struct K { a: Int } fn f(m: Map<K, Int>) {} fn main() {}",
        "fn f(m: Map<List<Int>, Int>) {} fn main() {}",
    ] {
        assert!(reject(src).0.ends_with("cannot be a Map key"), "{src}");
        assert_eq!(hint(src), "Map keys must be Int or Str", "{src}");
    }
    let (msg, at) = reject("fn f(m: Map<Bool, Int>) {} fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("`Bool` cannot be a Map key", "Bool"));
    ok("fn f(m: Map<Int, Map<Str, Int>>) {} fn main() {}");
}

#[test]
fn map_methods() {
    let m = "fn main() { let mut m: Map<Str, Int> = Map::new(); ";
    ok(&format!(
        "{m}m.insert(\"a\", 1); m.remove(\"a\"); let n: Int = m.len(); let k: List<Str> = m.keys(); let g: Option<Int> = m.get(\"a\"); }}"
    ));
    ok("struct S { m: Map<Int, Int> } fn main() { let mut s = S { m: Map::new() }; s.m.insert(1, 2); }");
    ok("fn main() { let xs: List<Map<Int, Int>> = [Map::new()]; }");
    let src = format!("{m}m.insert(1, 1); }}");
    assert_eq!(reject(&src).0, "expected Str, found Int");
    let src = format!("{m}m.insert(\"a\", \"b\"); }}");
    assert_eq!(reject(&src).0, "expected Int, found Str");
    let src = format!("{m}m.get(1); }}");
    assert_eq!(reject(&src).0, "expected Str, found Int");
    let src = format!("{m}m.insert(\"a\"); }}");
    assert_eq!(reject(&src).0, "`insert` takes 2 arguments, found 1");
    assert_eq!(
        hint(&src),
        "`Map<Str, Int>.insert` is declared as `insert(Str, Int) -> ()`"
    );
    assert_eq!(hint(&format!("{m}m.inser(\"a\", 1); }}")), "did you mean `insert`?");
    assert_eq!(
        hint(&format!("{m}m.push(1); }}")),
        "Map<Str, Int> has: get, insert, keys, len, remove"
    );
    let src = format!("{m}let x: Int = m.get(\"a\"); }}");
    assert_eq!(reject(&src).0, "expected Int, found Option<Int>");
    ok(&format!("{m}m == m; }}"));
}

#[test]
fn map_insert_and_remove_need_a_mut_place() {
    let f = "fn f() -> Map<Int, Int> { Map::new() } ";
    for (call, verb, to) in [("insert(1, 2)", "insert", "to"), ("remove(1)", "remove", "from")] {
        let src = format!("{f}fn main() {{ f().{call}; }}");
        let (msg, at) = reject(&src);
        assert_eq!(
            (msg, at.as_str()),
            (format!("cannot `{verb}` {to} this expression"), "f()")
        );
        assert_eq!(
            hint(&src),
            format!("`{verb}` changes a map; call it on a `let mut` variable")
        );
        let src = format!("fn main() {{ let m: Map<Int, Int> = Map::new(); m.{call}; }}");
        assert_eq!(reject(&src).0, format!("cannot `{verb}` {to} `m`, it is not `mut`"));
        assert_eq!(hint(&src), "declare it with `let mut`");
    }
    assert_eq!(
        hint("fn main() { let xs = [1]; xs.push(2); }"),
        "declare it with `let mut`"
    );
}

#[test]
fn maps_cannot_hold_capabilities() {
    let (msg, hint, at) = reject_cap("fn f(m: Map<Str, Dir>) {} fn main(d: Dir) {}");
    assert_eq!(
        (msg.as_str(), hint.as_str(), at.as_str()),
        (
            "capability nested in `Map<Str, Dir>`",
            "take `Dir` as a parameter",
            "Map<Str, Dir>"
        )
    );
    let (msg, _, _) = reject_cap("fn main(d: Dir) { let m: Map<Str, Dir> = Map::new(); }");
    assert_eq!(msg, "capability nested in `Map<Str, Dir>`");
}

/// What an agent fluent in Rust writes first; each error names the kio way.
#[test]
fn rust_isms_are_rejected_with_the_kio_way() {
    let cases = [
        (
            "fn f(s: &str) {} fn main() {}",
            "kio has no references",
            "pass by value; use `Str` instead of `&str`",
        ),
        ("fn f(s: String) {} fn main() {}", "unknown type `String`", "use `Str`"),
        (
            "fn f(v: Vec<Int>) {} fn main() {}",
            "unknown type `Vec`",
            "use `List<T>`",
        ),
        (
            "fn f(m: BTreeMap<Str, Int>) {} fn main() {}",
            "unknown type `BTreeMap`",
            "use `Map<K, V>`",
        ),
        ("fn f(n: i32) {} fn main() {}", "unknown type `i32`", "use `Int`"),
        ("fn main() { let x: f64 = 1; }", "unknown type `f64`", "use `Float`"),
        (
            "fn main() { let ys = [1].iter().map(|x| x + 1); }",
            "no method `iter` on List<Int>",
            "a List has no iterators; loop over it with `for x in xs { .. }`",
        ),
        (
            "fn main() { let f = move |x: Int| x; }",
            "`move` is not supported in kio",
            "closures always capture by copy; remove `move`",
        ),
        (
            "impl Row {} fn main() {}",
            "`impl` is not supported in kio",
            "kio has no traits or `impl`; use free functions",
        ),
        (
            "fn f<T: Clone>(x: T) -> T { x } fn main() {}",
            "only `Eq` and `Ord` bounds exist",
            "remove `Clone`: kio values are always copied and displayable",
        ),
        (
            "fn f<T>(x: T) -> T { x.clone() } fn main() {}",
            "no method `clone` on T",
            "`T` is a type parameter, which has no methods; take a function for it, like `f: fn(T) -> Str`",
        ),
    ];
    for (src, msg, hint) in cases {
        let d = &check(src, &CapTable::default()).expect_err(src)[0];
        assert_eq!((d.msg.as_str(), d.hint.as_deref()), (msg, Some(hint)), "{src}");
    }
}

/// H6: diagnostics stop at a fixed count, so output stays bounded for any source.
#[test]
fn diagnostics_are_capped() {
    let src = "struct A {}\n".repeat(500) + "fn main() {}";
    let diags = check(&src, &CapTable::default()).unwrap_err();
    assert_eq!(diags.len(), lang::check::MAX_DIAGS + 1);
    assert_eq!(diags.last().unwrap().msg, "399 more errors not shown");
}

#[test]
fn bytes_and_option_methods() {
    ok("fn main() { let mut b = \"a\".to_bytes(); b.push(b[0] + 1); }");
    ok("fn main() -> Int { Some(1).unwrap_or(2) }");
    assert_eq!(
        hint("fn main() { let mut b = \"a\".to_bytes(); b[0] = 1; }"),
        "use `push`, or build new Bytes"
    );
    assert_eq!(
        hint("fn main() { let b = \"a\".to_bytes(); b.push(1); }"),
        "declare it with `let mut`"
    );
    let (msg, at) = reject("fn main() -> Int { Some(1).unwrap_or(\"x\") }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"x\""));
    assert_eq!(
        hint("fn main() -> Int { Some(1).unwrap_or(\"x\") }"),
        "`Option<Int>.unwrap_or` is declared as `unwrap_or(Int) -> Int`"
    );
}

#[test]
fn option_result_predicates() {
    ok("fn f(o: Option<Int>, r: Result<Int, Str>) -> Bool { o.is_some() && o.is_none() || r.is_ok() || r.is_err() } fn main() {}");
    ok("fn f(r: Result<Int, Str>) -> Int { r.unwrap_or(0) } fn main() {}");
    let (msg, _) = reject("fn f(r: Result<Int, Str>) -> Int { r.unwrap_or(\"x\") } fn main() {}");
    assert_eq!(msg, "expected Int, found Str");
    assert_eq!(
        hint("fn f(r: Result<Int, Str>) -> Int { r.unwrap_or(\"x\") } fn main() {}"),
        "`Result<Int, Str>.unwrap_or` is declared as `unwrap_or(Int) -> Int`"
    );
    // `is_ok` is not an Option method.
    let h = hint("fn f(o: Option<Int>) -> Bool { o.is_ok() } fn main() {}");
    assert!(h.contains("is_some") && h.contains("is_none"), "{h}");
}

/// Lowercase-then-`::` names a module, so type names must start uppercase.
#[test]
fn type_names_start_uppercase() {
    for (src, name, fixed) in [
        ("enum status { Paid } fn main() {}", "status", "Status"),
        ("struct point { x: Int } fn main() {}", "point", "Point"),
    ] {
        let d = &check(src, &CapTable::default()).unwrap_err()[0];
        assert_eq!(d.msg, format!("type names start with an uppercase letter: `{name}`"));
        assert_eq!(d.hint.as_deref(), Some(format!("rename it `{fixed}`").as_str()));
    }
}

/// A helper using a capability it was not given is told how to receive it.
#[test]
fn hint_to_pass_a_capability_in() {
    let src = "fn f() -> Int { let r = log.get(\"/x\"); 1 } fn main(log: Http) -> Int { f() }";
    let d = &check(src, &CapTable::v0()).unwrap_err()[0];
    assert_eq!(d.msg, "unknown name `log`");
    assert_eq!(
        d.hint.as_deref(),
        Some("`log` is a capability of `main`; take it as a parameter, `log: Http`, and pass it in")
    );
}

#[test]
fn structural_equality() {
    ok("fn main() -> Bool { let r: Result<Int, Str> = Ok(1); let e: Result<Int, Str> = Err(\"e\"); Some(1) == Some(1) && r != e }");
    ok("fn main() -> Bool { [1, 2] == [1, 2] }");
    ok("fn main() -> Bool { let a: Map<Str, List<Int>> = Map::new(); a == a }");
    let (msg, _) = reject("fn main() -> Bool { Some(1) == Some(\"a\") }");
    assert_eq!(msg, "`==` cannot be applied to Option<Int> and Option<Str>");
}

#[test]
fn capabilities_have_no_equality() {
    for (src, ty) in [
        ("fn main(d: Dir) -> Bool { d == d }", "Dir"),
        ("fn main(d: Dir) -> Bool { d != d }", "Dir"),
    ] {
        let d = &check(src, &CapTable::v0()).unwrap_err()[0];
        assert_eq!(d.msg, format!("cannot compare `{ty}` values"));
        assert_eq!(d.hint.as_deref(), Some("capabilities have no equality"));
    }
}

#[test]
fn loop_types() {
    ok("fn f() -> Int { loop { return 1; } } fn main() {}");
    ok("fn main() -> Int { let mut n = 0; loop { n += 1; if n > 2 { break; } } n }");
    ok("fn main() { let mut n = 0; loop { n += 1; if n < 2 { continue; } break; } }");
    // A `break` in a nested loop does not end the outer `loop`.
    ok("fn f() -> Int { loop { while true { break; } return 1; } } fn main() {}");
    // A `loop` with a `break` can fall through, so a tail is needed.
    let (msg, _) = reject("fn f() -> Int { loop { break; } } fn main() {}");
    assert_eq!(msg, "missing return: `f` must return Int");
    ok("fn f() -> Int { loop { while true { break; } } } fn main() {}");
}

#[test]
fn result_ok_err_and_option_ok_or() {
    ok("fn f(r: Result<Int, Str>) -> Option<Str> { let o: Option<Int> = r.ok(); r.err() } fn main() {}");
    ok("fn f(o: Option<Int>) -> Result<Int, Str> { Ok(o.ok_or(\"no\")? + 1) } fn main() {}");
    // `?` on `ok_or(e)` needs the function's error type to equal the argument's type.
    let src = "fn f(o: Option<Int>) -> Result<Int, Int> { let x = o.ok_or(\"no\")?; Ok(x) } fn main() {}";
    let (msg, _) = reject(src);
    assert_eq!(msg, "`?` on Result<Int, Str> in a function returning Result<Int, Int>");
    assert_eq!(
        hint(src),
        "change the return type, convert the error with `.map_err(f)?`, or `match` on the result"
    );
    assert_eq!(
        hint("fn f(o: Option<Int>) -> Bool { o.zzzzzz() } fn main() {}"),
        "Option<Int> has: and_then, is_none, is_some, map, ok_or, unwrap_or, unwrap_or_else"
    );
    assert_eq!(
        hint("fn f(o: Result<Int, Str>) -> Bool { o.zzzzzz() } fn main() {}"),
        "Result<Int, Str> has: and_then, err, is_err, is_ok, map, map_err, ok, unwrap_or, unwrap_or_else"
    );
}

const DOUBLE: &str = "fn double(x: Int) -> Int { x * 2 } ";

#[test]
fn function_values() {
    ok(&format!(
        "{DOUBLE}fn apply(f: fn(Int) -> Int, x: Int) -> Int {{ f(x) }} fn main() -> Int {{ apply(double, 3) }}"
    ));
    // In a field, called through parentheses; in a list, an `Option` and an enum payload.
    ok(&format!(
        "{DOUBLE}struct R {{ h: fn(Int) -> Int }} enum E {{ F(fn(Int) -> Int) }}
         fn main() -> Int {{
             let r = R {{ h: double }};
             let fs: List<fn(Int) -> Int> = [double, double];
             let o: Option<fn(Int) -> Int> = Some(double);
             let e = E::F(double);
             (r.h)(1) + (fs[0])(2) + match o {{ Some(f) => f(3), None => 0 }}
         }}"
    ));
    // A function returning a function, called at once; `fn()` with no `->` returns `()`.
    ok(&format!(
        "{DOUBLE}fn pick() -> fn(Int) -> Int {{ double }} fn nop() {{}}
         fn main() -> Int {{ let n: fn() = nop; n(); pick()(4) }}"
    ));
    // A recursive function as a value.
    ok("fn fact(n: Int) -> Int { if n < 2 { 1 } else { n * fact(n - 1) } } fn main() -> Int { let f = fact; f(5) }");
    // A local hides a function of the same name, as in Rust.
    ok(&format!("{DOUBLE}fn main() -> Int {{ let double = 3; double }}"));
    ok(&format!(
        "{DOUBLE}fn main() -> Int {{ let g = double; let double = g; double(1) }}"
    ));
}

#[test]
fn function_values_carry_no_capability() {
    let caps = common::caps();
    let fn_cap = |src: &str| {
        let d = &check(src, &caps).expect_err(src)[0];
        assert_eq!(d.msg, "function values cannot take or return capabilities", "{src}");
        assert_eq!(
            d.hint.as_deref(),
            Some("pass the capability to a named function instead")
        );
        src[d.span.start..d.span.end].to_string()
    };
    assert_eq!(fn_cap("fn f(g: fn(Dir)) {} fn main() {}"), "fn(Dir)");
    assert_eq!(fn_cap("fn f() -> fn() -> Dir { f() } fn main() {}"), "fn() -> Dir");
    assert_eq!(
        fn_cap("fn f(g: List<fn(Option<Dir>) -> Int>) {} fn main() {}"),
        "List<fn(Option<Dir>) -> Int>"
    );
    assert_eq!(fn_cap("struct S { h: fn(Int, Http) } fn main() {}"), "fn(Int, Http)");
    assert_eq!(fn_cap("enum E { A(fn(fn(Dir))) } fn main() {}"), "fn(fn(Dir))");
    let src = "fn load(d: Dir) {} fn main(d: Dir) { let g: fn(Dir) = load; }";
    assert_eq!(fn_cap(src), "fn(Dir)");
    // A function with a capability parameter, used as a value: at its name.
    assert_eq!(fn_cap("fn load(d: Dir) {} fn main(d: Dir) { let g = load; }"), "load");
    let src = "fn load(d: Dir, n: Int) {} fn take(x: Int) {} fn main() { take(load); }";
    assert_eq!(fn_cap(src), "load");
    // A capability cannot be passed through a value of `fn` type.
    let src = format!("{DOUBLE}fn main(d: Dir) {{ let g = double; g(d); }}");
    assert_eq!(check(&src, &caps).unwrap_err()[0].msg, "expected Int, found Dir");
}

#[test]
fn function_value_rejections() {
    let (msg, at) = reject("fn main() { let m = main; }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`main` cannot be used as a value", "main")
    );
    assert_eq!(
        hint("fn main() { let m = main; }"),
        "move the code into another function and use that"
    );
    let r = "struct R { h: fn(Int) -> Int } ";
    for (src, ty) in [
        (
            format!("{DOUBLE}fn main() -> Bool {{ let f = double; f == double }}"),
            "fn(Int) -> Int",
        ),
        (
            format!("{DOUBLE}{r}fn main() -> Bool {{ let r = R {{ h: double }}; r != r }}"),
            "R",
        ),
        (
            format!("{DOUBLE}fn main() -> Bool {{ let o = Some([double]); o == o }}"),
            "Option<List<fn(Int) -> Int>>",
        ),
        (
            format!("{DOUBLE}fn main() -> Bool {{ [double].contains(double) }}"),
            "fn(Int) -> Int",
        ),
    ] {
        let (msg, _) = reject(&src);
        assert_eq!(msg, format!("cannot compare `{ty}` values"), "{src}");
        assert_eq!(hint(&src), "functions have no equality");
    }
    // Equality still works on recursive types without functions.
    ok("struct T { kids: List<T> } fn main() -> Bool { let t = T { kids: [] }; t == t }");
    let src = "fn f(m: Map<fn(Int), Int>) {} fn main() {}";
    assert_eq!(reject(src).0, "`fn(Int)` cannot be a Map key");
    assert_eq!(hint(src), "Map keys must be Int or Str");
}

#[test]
fn calling_a_non_function() {
    let (msg, at) = reject("fn main() { let x = 1; x(2); }");
    assert_eq!((msg.as_str(), at.as_str()), ("`x` is Int, not a function", "x"));
    assert_eq!(
        hint("fn main() { let x = 1; x(2); }"),
        "only functions and values of `fn` type can be called"
    );
    let src = "fn f() {} fn main() { let f = 1; f(); }";
    assert_eq!(reject(src).0, "`f` is Int, not a function");
    assert_eq!(hint(src), "the local `f` hides the function `f`; rename the local");
    let (msg, at) = reject("fn main() { (1 + 2)(3); }");
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("this expression is Int, not a function", "1 + 2")
    );
    let d = &check("fn main(d: Dir) { d(1); }", &common::caps()).unwrap_err()[0];
    assert_eq!(d.msg, "`d` is Dir, not a function");
}

#[test]
fn calls_through_values_check_against_the_fn_type() {
    let src = format!("{DOUBLE}fn main() {{ let g = double; g(1, 2); }}");
    let (msg, at) = reject(&src);
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`g` takes 1 arguments, found 2", "g(1, 2)")
    );
    assert_eq!(hint(&src), "`g` has type `fn(Int) -> Int`");
    let src = "struct R { h: fn(Int) -> Int } fn f(r: R) { (r.h)(); } fn main() {}";
    assert_eq!(reject(src).0, "this expression takes 1 arguments, found 0");
    let (msg, at) = reject(&format!("{DOUBLE}fn main() {{ let g = double; g(\"a\"); }}"));
    assert_eq!((msg.as_str(), at.as_str()), ("expected Int, found Str", "\"a\""));
    let (msg, _) = reject(&format!("{DOUBLE}fn main() {{ let g: fn(Str) -> Int = double; }}"));
    assert_eq!(msg, "expected fn(Str) -> Int, found fn(Int) -> Int");
    let (msg, _) = reject(&format!("{DOUBLE}fn main() -> Str {{ let g = double; g(1) }}"));
    assert_eq!(msg, "expected Str, found Int");
    // `r.h(x)` is a method call; the hint shows how to call the field.
    let src = "struct R { h: fn(Int) -> Int } fn f(r: R) -> Int { r.h(1) } fn main() {}";
    assert_eq!(reject(src).0, "no method `h` on R");
    assert_eq!(hint(src), "`h` is a field; call it as `(r.h)(..)`");
}

#[test]
fn floats_typecheck() {
    ok("fn main() -> Bool {
            let x: Float = -1.5 * 2.0 / 3.0 % 1.0 + 0.5 - 1e3;
            let n: Option<Int> = x.to_int();
            let s: Str = x.abs().floor().ceil().round().sqrt().to_str();
            let f: Option<Float> = s.parse_float();
            x < 1.0 && x <= 1.0 && x > 0.0 && x >= 0.0 && x == x && x != 1.0 && !x.is_nan() && 1.to_float() == 1.0
        }");
}

#[test]
fn int_and_float_never_mix() {
    let convert = "convert with `.to_float()` or `.to_int()`";
    for (src, msg, at) in [
        ("fn main() -> Float { 1 + 2.0 }", "cannot add Int and Float", "1 + 2.0"),
        (
            "fn main() -> Float { 1.0 - 2 }",
            "cannot subtract Float and Int",
            "1.0 - 2",
        ),
        (
            "fn main() -> Float { 1.0 * 2 }",
            "cannot multiply Float and Int",
            "1.0 * 2",
        ),
        (
            "fn main() -> Float { 1 / 2.0 }",
            "cannot divide Int and Float",
            "1 / 2.0",
        ),
        (
            "fn main() -> Float { 1 % 2.0 }",
            "cannot take the remainder of Int and Float",
            "1 % 2.0",
        ),
        (
            "fn main() -> Bool { 1 < 2.0 }",
            "cannot compare Int and Float",
            "1 < 2.0",
        ),
        (
            "fn main() -> Bool { 1.0 == 1 }",
            "cannot compare Float and Int",
            "1.0 == 1",
        ),
        (
            "fn main() { let mut x = 1.0; x += 1; }",
            "cannot add Float and Int",
            "x += 1;",
        ),
        (
            "fn main() { let mut n = 1; n -= 1.0; }",
            "cannot subtract Int and Float",
            "n -= 1.0;",
        ),
    ] {
        assert_eq!(reject(src), (msg.to_string(), at.to_string()), "{src}");
        assert_eq!(hint(src), convert, "{src}");
    }
    assert_eq!(
        reject("fn main() -> Float { let x: Float = 1; x }").0,
        "expected Float, found Int"
    );
    assert_eq!(hint("fn main() -> Bool { 1.0 && true }"), "`&&` works on two Bool");
    assert_eq!(
        hint("fn main() -> Float { 1.0 + \"a\" }"),
        "convert with `1.0.to_str()`"
    );
}

#[test]
fn floats_cannot_be_matched_or_keys() {
    let compare = "compare with `<` or `==`";
    let src = "fn main() -> Int { let x = 1.5; match x { _ => 1 } }";
    assert_eq!(reject(src), ("cannot match on Float".to_string(), "x".to_string()));
    assert_eq!(hint(src), compare);
    let src = "fn main() { if let _ = 1.5 { } }";
    assert_eq!(reject(src).0, "cannot match on Float");
    let src = "fn main() -> Int { match Some(1.5) { Some(1.5) => 1, _ => 2 } }";
    assert_eq!(
        reject(src),
        ("a `Float` cannot be a pattern".to_string(), "1.5".to_string())
    );
    assert_eq!(hint(src), compare);
    let src = "fn main() -> Int { match 1 { -1.5 => 1, _ => 2 } }";
    assert_eq!(reject(src).0, "a `Float` cannot be a pattern");
    // Matching an `Option<Float>` by its variants is fine.
    ok("fn main() -> Float { match Some(1.5) { Some(x) => x, None => 0.0 } }");
    let src = "fn main() { let m: Map<Float, Int> = Map::new(); }";
    assert_eq!(reject(src).0, "`Float` cannot be a Map key");
    assert_eq!(hint(src), "Map keys must be Int or Str");
    assert_eq!(
        hint("fn main() { let mut xs = [1.0]; xs.sort(); }"),
        "sort works on List<Int> and List<Str>"
    );
    assert_eq!(reject("fn main() -> Float { -true }").0, "expected Int, found Bool");
}

#[test]
fn assignment_passes_the_place_type() {
    ok("fn main() { let mut o: Option<Int> = Some(1); o = None; }");
    ok("fn main() { let mut r: Result<Int, Str> = Ok(1); r = Err(\"x\"); r = Ok(2); }");
    ok("fn main() { let mut xs: List<Int> = [1]; xs = []; }");
    ok("fn main() { let mut m: Map<Str, Int> = Map::new(); m = Map::new(); }");
    ok("struct S { f: Option<Int> } fn main() { let mut s = S { f: Some(1) }; s.f = None; }");
    ok("fn main() { let mut xs: List<Option<Int>> = [Some(1)]; xs[0] = None; }");
    let (msg, _) = reject("fn main() { let mut o: Option<Int> = None; o = 1; }");
    assert_eq!(msg, "expected Option<Int>, found Int");
    let (msg, _) = reject("fn main() { let mut x = 1; x = None; }");
    assert_eq!(msg, "expected Int, found an Option");
}

#[test]
fn closures_check() {
    // Parameter types from annotations, a `let` type, a parameter, a field, a list, an `Option`.
    ok("fn main() -> Int { let n = 1; let f = |x: Int| x + n; f(2) }");
    ok("fn main() -> Int { let n = 1; let f: fn(Int) -> Int = |x| x + n; f(2) }");
    ok("fn apply(f: fn(Int, Int) -> Int, x: Int) -> Int { f(x, x) } fn main() -> Int { apply(|a, b| { a * b }, 3) }");
    ok("struct R { h: fn(Str) -> Int } fn main() -> Int { let r = R { h: |s| s.len() }; (r.h)(\"ab\") }");
    ok("fn main() -> Int { let fs: List<fn(Int) -> Int> = [|x| x, |x| x + 1]; (fs[1])(1) }");
    ok("fn main() -> Int { let o: Option<fn() -> Int> = Some(|| 7); match o { Some(f) => f(), None => 0 } }");
    // A closure returned from a function, and one returning a closure.
    ok("fn adder(n: Int) -> fn(Int) -> Int { |x| x + n } fn main() -> Int { adder(1)(2) }");
    ok("fn main() -> Int { let add = |x: Int| |y: Int| x + y; add(1)(2) }");
    // Assigned to a `let mut` of `fn` type; `return` returns from the closure.
    ok("fn main() -> Int { let mut f: fn(Int) -> Int = |x| x; f = |x| x * 2; f(1) }");
    ok("fn main() -> Int { let f = |x: Int| { if x > 0 { return 1; } 2 }; f(1) }");
    ok("fn main() -> Option<Int> { let f: fn(Str) -> Option<Int> = |s| { let n = s.parse_int()?; Some(n + 1) }; f(\"1\") }");
    // A loop inside a closure may `break`; a parameter may shadow an outer local.
    ok("fn main() -> Int { let n = 5; let f = |n: Int| { let mut i = 0; loop { i += 1; if i == n { break; } } i }; f(n) }");
    // A closure is a value without equality.
    let (msg, _) = reject("fn main() -> Bool { let f = |x: Int| x; f == f }");
    assert_eq!(msg, "cannot compare `fn(Int) -> Int` values");
}

#[test]
fn closure_rejections() {
    let cases = [
        (
            "fn main() { let f = |x| x; }",
            "cannot tell the type of `x`",
            "x",
            "add a type: `|x: Int| ..`",
        ),
        (
            "fn main() { let n: Int = |x| x; }",
            "expected Int, found a closure",
            "x",
            "add a type: `|x: Int| ..`",
        ),
        (
            "fn main() { let f: fn(Int) -> Int = |a, b| a; }",
            "expected fn(Int) -> Int, found a closure of 2 parameters",
            "|a, b| a",
            "`fn(Int) -> Int` takes 1",
        ),
        (
            "fn main() { let mut n = 1; let f = || { n = 2; }; }",
            "`n` is captured by copy and cannot be assigned",
            "n",
            "return the new value instead",
        ),
        (
            "fn main() { let mut n = 1; let f = |x: Int| { n += x; }; }",
            "`n` is captured by copy and cannot be assigned",
            "n",
            "return the new value instead",
        ),
        (
            "fn main() { let mut xs = [1]; let f = || xs.push(2); }",
            "`xs` is captured by copy and cannot be changed",
            "xs",
            "return the new value instead",
        ),
        (
            "fn main() { loop { let f = || { break; }; } }",
            "`break` cannot leave a closure",
            "break;",
            "return from the closure instead",
        ),
        (
            "fn main() { for i in 0..3 { let f = || { continue; }; } }",
            "`continue` cannot leave a closure",
            "continue;",
            "return from the closure instead",
        ),
        (
            "fn main() { let f = |a: Int, a: Int| a; }",
            "duplicate parameter `a`",
            "a: Int",
            "rename one of them",
        ),
        (
            "fn main() { let f = |None: Int| 1; }",
            "`None` is a built-in constructor",
            "None: Int",
            "choose another name",
        ),
        (
            "fn main() { let f = |x: Int| { let y = Some(x)?; y }; }",
            "cannot tell what `?` returns from this closure",
            "Some(x)?",
            "give the closure a type: `let f: fn(Int) -> Option<Int> = |x| ..`",
        ),
    ];
    for (src, msg, at, h) in cases {
        assert_eq!(reject(src), (msg.to_string(), at.to_string()), "{src}");
        assert_eq!(hint(src), h, "{src}");
    }
    // The body against the expected return type, and against an earlier `return`.
    let (msg, at) = reject("fn main() { let f: fn(Int) -> Str = |x| x; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Str, found Int", "x"));
    let (msg, at) = reject("fn main() { let f = |x: Int| { if x > 0 { return \"a\"; } x }; }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Str, found Int", "x"));
    // A closure's own locals and parameters are not captures.
    ok("fn main() -> Int { let f = |x: Int| { let mut y = x; y += 1; y }; f(1) }");
}

#[test]
fn builtins_that_take_functions_check() {
    // Types follow the function's: the closure's parameter from the receiver, its result from its body.
    ok("fn main() -> List<Str> { [1, 2].map(|x| x.to_str()) }");
    ok("fn main() -> List<Int> { [1, -2].filter(|x| x > 0).map(|x| x * 2) }");
    ok("fn main() -> Bool { [\"a\"].any(|s| s.len() > 0) && [1].all(|x: Int| x > 0) }");
    ok("fn main() -> Option<Str> { Some(1).map(|x| x.to_str()) }");
    ok("fn main() -> Option<Int> { Some(\"1\").and_then(|s| s.parse_int()) }");
    ok("fn main() -> Result<Int, Int> { let r: Result<Int, Str> = Err(\"e\"); r.map_err(|e| e.len()) }");
    ok("fn main() -> Result<Bool, Str> { let r: Result<Int, Str> = Ok(1); r.map(|x| x > 0) }");
    ok("fn main() -> Result<Int, Str> { let r: Result<Str, Str> = Ok(\"1\"); r.and_then(|s| { let n = s.parse_int().ok_or(\"no\")?; Ok(n) }) }");
    // A function name, a field, a local; a closure nested in one passed directly.
    ok("fn len(s: Str) -> Int { s.len() } fn main() -> List<Int> { [\"a\"].map(len) }");
    ok("struct S { f: fn(Int) -> Bool } fn main() -> Bool { let s = S { f: |x| x > 0 }; [1].all(s.f) }");
    ok("fn main() -> List<List<Int>> { let n = 2; [[1]].map(|xs| xs.map(|x| x * n)) }");
    // `Ok` outside an `and_then` closure still needs a type.
    let (msg, _) = reject("fn main() { let r: Result<Int, Str> = Ok(1); let x = r.map(|x| Ok(x)); }");
    assert_eq!(msg, "cannot tell the type of `Ok`");
}

#[test]
fn builtins_that_take_functions_reject() {
    let r = "let r: Result<Int, Str> = Ok(1);";
    let cases = [
        (
            "fn add(a: Int, b: Int) -> Int { a + b } fn main() { [1].map(add); }".to_string(),
            "`map` takes a function of 1 parameter, found fn(Int, Int) -> Int",
            "add",
            "`List<Int>.map` is declared as `map(fn(Int) -> U) -> List<U>`",
        ),
        (
            "fn main() { [1].map(|a, b| a); }".to_string(),
            "`map` takes a function of 1 parameter, found a closure of 2 parameters",
            "|a, b| a",
            "`List<Int>.map` is declared as `map(fn(Int) -> U) -> List<U>`",
        ),
        (
            "fn main() { [1].filter(|x| x * 2); }".to_string(),
            "`filter` needs a function returning Bool, found fn(Int) -> Int",
            "|x| x * 2",
            "`List<Int>.filter` is declared as `filter(fn(Int) -> Bool) -> List<Int>`",
        ),
        (
            "fn main() { [1].any(|x| x); }".to_string(),
            "`any` needs a function returning Bool, found fn(Int) -> Int",
            "|x| x",
            "`List<Int>.any` is declared as `any(fn(Int) -> Bool) -> Bool`",
        ),
        (
            "fn main() { [1].all(|x: Str| true); }".to_string(),
            "`all` needs a function taking Int, found fn(Str) -> Bool",
            "|x: Str| true",
            "`List<Int>.all` is declared as `all(fn(Int) -> Bool) -> Bool`",
        ),
        (
            "fn main() { [1].map(1); }".to_string(),
            "`map` takes a function, found Int",
            "1",
            "`List<Int>.map` is declared as `map(fn(Int) -> U) -> List<U>`",
        ),
        (
            "fn main() { [1].map(); }".to_string(),
            "`map` takes 1 arguments, found 0",
            "[1].map()",
            "`List<Int>.map` is declared as `map(fn(Int) -> U) -> List<U>`",
        ),
        (
            "fn main() { Some(1).and_then(|x| x + 1); }".to_string(),
            "`and_then` needs a function returning Option<..>, found fn(Int) -> Int",
            "|x| x + 1",
            "`Option<Int>.and_then` is declared as `and_then(fn(Int) -> Option<U>) -> Option<U>`",
        ),
        (
            format!("fn f(x: Int) -> Result<Int, Int> {{ Ok(x) }} fn main() {{ {r} r.and_then(f); }}"),
            "`and_then` needs a function returning Result<.., Str>, found fn(Int) -> Result<Int, Int>",
            "f",
            "`Result<Int, Str>.and_then` is declared as `and_then(fn(Int) -> Result<U, Str>) -> Result<U, Str>`",
        ),
        (
            format!("fn main() {{ {r} r.map_err(|e: Int| e); }}"),
            "`map_err` needs a function taking Str, found fn(Int) -> Int",
            "|e: Int| e",
            "`Result<Int, Str>.map_err` is declared as `map_err(fn(Str) -> F) -> Result<Int, F>`",
        ),
        (
            "fn main() { Some(\"1\").and_then(|s| { let r: Result<Int, Str> = Err(s); let n = r?; Some(n) }); }"
                .to_string(),
            "`?` on Result<Int, Str> in a closure returning Option<..>",
            "r?",
            "`match` on the value, or convert it: `.ok_or(e)` makes an Option a Result",
        ),
        (
            "fn main() { Some(\"1\").and_then(|s| if s == \"\" { None } else { s.parse_int() }); }".to_string(),
            "cannot tell the type of `None`",
            "None",
            "give the closure a type: `let f: fn(Str) -> Option<Int> = |s| ..;`, then `.and_then(f)`",
        ),
        (
            "fn main() { [1].sum(); }".to_string(),
            "no method `sum` on List<Int>",
            "[1].sum()",
            "List<Int> has: all, any, contains, filter, join, len, map, pop, push, reverse, slice, sort, sort_by, sort_by_key",
        ),
    ];
    for (src, msg, at, h) in cases {
        assert_eq!(reject(&src), (msg.to_string(), at.to_string()), "{src}");
        assert_eq!(hint(&src), h, "{src}");
    }
}

/// Security: a closure cannot reach a capability by any path, and its type cannot name one.
#[test]
fn closures_cannot_reach_capabilities() {
    let caps = common::caps();
    let captures = |src: &str, cap: &str, ty: &str| {
        let d = &check(src, &caps).expect_err(src)[0];
        assert_eq!(d.msg, format!("closure captures capability `{cap}`"), "{src}");
        assert_eq!(
            d.hint.as_deref(),
            Some(&*format!(
                "use a named function that takes it as a parameter, `{cap}: {ty}`"
            ))
        );
        assert_eq!(&src[d.span.start..d.span.end], cap, "{src}");
    };
    // A method call on a capability of `main`, or on a parameter of any function.
    captures("fn main(out: Dir) { let f = || out.read(\"a\"); }", "out", "Dir");
    captures("fn g(h: Http) { let f = |u: Str| h.get(u); } fn main() {}", "h", "Http");
    // Passed to a helper, or through a value of `fn` type, from inside the body.
    captures(
        "fn load(d: Dir) {} fn main(out: Dir) { let f = || load(out); }",
        "out",
        "Dir",
    );
    captures(
        "fn main(out: Dir) { let f = || { let g = |x: Int| x; g(out) }; }",
        "out",
        "Dir",
    );
    // Bound to a local, compared, called, assigned, or used as a value.
    captures("fn main(out: Dir) { let f = || { let d = out; }; }", "out", "Dir");
    captures("fn main(out: Dir) { let f = || out == out; }", "out", "Dir");
    captures("fn main(out: Dir) { let f = || out(1); }", "out", "Dir");
    captures("fn main(out: Dir) { let f = || { out = out; }; }", "out", "Dir");
    captures("fn main(out: Dir) { let f = || Some(out); }", "out", "Dir");
    // Through nested closures and a nested block.
    captures(
        "fn main(out: Dir) { let f = || || { { out.read(\"a\") } }; }",
        "out",
        "Dir",
    );
    captures(
        "fn main(kv: Kv) { let f = |k: Str| { let g = || kv.get(k); 1 }; }",
        "kv",
        "Kv",
    );
    // The hint's fix is rejected by the `fn` type rule: the real fix is a named function.
    let fn_cap = |src: &str| {
        let d = &check(src, &caps).expect_err(src)[0];
        assert_eq!(d.msg, "function values cannot take or return capabilities", "{src}");
        assert_eq!(
            d.hint.as_deref(),
            Some("pass the capability to a named function instead")
        );
        src[d.span.start..d.span.end].to_string()
    };
    assert_eq!(
        fn_cap("fn main(out: Dir) { let f = |out: Dir| out.read(\"a\"); }"),
        "Dir"
    );
    assert_eq!(
        fn_cap("fn main() { let f = |x: Int, o: Option<Dir>| x; }"),
        "Option<Dir>"
    );
    assert_eq!(fn_cap("fn main() { let f = |g: fn(Dir)| 1; }"), "fn(Dir)");
    assert_eq!(fn_cap("fn main() { let f: fn(Dir) = |d| {}; }"), "fn(Dir)");
    // A parameter that shadows a capability is a plain value, not a capture.
    assert!(check("fn main(out: Dir) { let f = |out: Int| out + 1; }", &caps).is_ok());
    // A named function with the capability parameter is the way.
    let src = "fn save(out: Dir, s: Str) -> Result<(), IoErr> { out.create(\"a\", s.to_bytes()) }
               fn main(out: Dir) -> Result<(), IoErr> { let f = |n: Int| n.to_str(); save(out, f(1)) }";
    assert!(check(src, &caps).is_ok());
}

#[test]
fn l12_text_plus_number_and_try_hints() {
    let cases = [
        (
            "fn main() -> Str { \"n=\" + 3 }",
            "\"n=\" + 3",
            "convert with `3.to_str()`",
        ),
        (
            "fn main() -> Int { 3 + \"x\" }",
            "3 + \"x\"",
            "convert with `3.to_str()`",
        ),
        (
            "fn main() -> Str { let n = 2.5; \"n=\" + n }",
            "\"n=\" + n",
            "convert with `n.to_str()`",
        ),
        (
            "fn main() { let mut s = \"a\"; s += 3; }",
            "s += 3;",
            "convert with `3.to_str()`",
        ),
        (
            "fn f() -> Result<Int, Str> { Ok(1) } fn main() -> Result<Int, Int> { let x = f()?; Ok(x) }",
            "f()?",
            "change the return type, convert the error with `.map_err(f)?`, or `match` on the result",
        ),
    ];
    for (src, at, h) in cases {
        let d = &check(src, &CapTable::default()).expect_err(src)[0];
        assert_eq!(&src[d.span.start..d.span.end], at, "{src}");
        assert_eq!(d.hint.as_deref(), Some(h), "{src}");
    }
}
