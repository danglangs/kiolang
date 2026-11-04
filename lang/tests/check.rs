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
fn struct_equality_is_rejected() {
    let (msg, _) = reject_p("fn main() -> Bool { Q { c: 1 } == Q { c: 1 } }");
    assert_eq!(msg, "`==` cannot be applied to Q and Q");
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
fn enum_equality_is_rejected() {
    let (msg, _) = reject_s("fn main() -> Bool { S::Done == S::Done }");
    assert_eq!(msg, "`==` cannot be applied to S and S");
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
    assert_eq!(hint("fn main() -> Int { 1 + true }"), "`+` works on two Int or two Str");
    assert_eq!(hint("fn main() -> Int { 1 - \"a\" }"), "`-` works on two Int");
    assert_eq!(hint("fn main() -> Bool { 1 && 2 }"), "`&&` works on two Bool");
    assert_eq!(
        hint("fn main() -> Bool { 1 == true }"),
        "`==` works on two Int, two Bool, two Str or two Bytes"
    );
    assert_eq!(
        hint("fn main() { let mut x = true; x += 1; }"),
        "`+` works on two Int or two Str"
    );
    assert_eq!(hint("fn main() { 1(2); }"), "call a function by name, like `f(x)`");
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
    assert_eq!(msg, "`==` cannot be applied to Option<Int> and Option<Int>");
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
    assert_eq!(hint(src), "change the return type or `match` on the result");
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
        "Str has: contains, len, lines, parse_int, split, starts_with, to_bytes, trim"
    );
    ok("fn main() { let a: List<Str> = \"a\".split(\",\"); let b: List<Str> = \"a\".lines(); let c: Str = \"a\".trim(); let d: Bool = \"a\".contains(\"a\") && \"a\".starts_with(\"a\"); let e: Option<Int> = \"1\".parse_int(); }");
    assert_eq!(reject("fn main() { \"a\".split(1); }").0, "expected Str, found Int");
    assert_eq!(hint("fn main() { 1.zzzzzz(); }"), "Int has: to_str");
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
    assert_rejected("fn main(d: Dir, e: Dir) -> Bool { d == e }", value, AS_VALUE, "d");
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
        "unknown variant `Dir::new`",
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
        ("no method `delete` on Dir", "Dir has: read, create")
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
    assert_eq!(hint("fn main() { let x = 1; x[0]; }"), "only a List can be indexed");
    reject("fn main() { let x: Int = [1][0] + [\"a\"][0]; }");
    let (msg, _) = reject("fn main() { let x = [1, 2]; x == x; }");
    assert_eq!(msg, "`==` cannot be applied to List<Int> and List<Int>");
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
    let (msg, _) = reject(&format!("{m}m == m; }}"));
    assert_eq!(msg, "`==` cannot be applied to Map<Str, Int> and Map<Str, Int>");
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
            "fn f(m: HashMap<Str, Int>) {} fn main() {}",
            "unknown type `HashMap`",
            "use `Map<K, V>`",
        ),
        ("fn f(n: i32) {} fn main() {}", "unknown type `i32`", "use `Int`"),
        (
            "fn main() { let x: f64 = 1; }",
            "unknown type `f64`",
            "kio has no floats; use `Int`, e.g. cents",
        ),
        (
            "fn main() { let ys = [1].iter().map(|x| x + 1); }",
            "kio has no closures",
            "use a `for` loop",
        ),
        (
            "impl Row {} fn main() {}",
            "`impl` is not supported in kio",
            "kio has no traits or `impl`; use free functions",
        ),
    ];
    for (src, msg, hint) in cases {
        let d = &check(src, &CapTable::default()).expect_err(src)[0];
        assert_eq!((d.msg.as_str(), d.hint.as_deref()), (msg, Some(hint)), "{src}");
    }
}
