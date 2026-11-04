use lang::{check, CapTable};

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
    reject("fn f(x: List<Int>) {} fn main() {}");
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
    assert_eq!(diags[0].hint.as_deref(), Some("fields: c"));
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
