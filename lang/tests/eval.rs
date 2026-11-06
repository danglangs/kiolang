mod common;

use lang::{check, run, CapTable, Caps, RunErr, Value};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

fn exec(src: &str) -> Result<Value, RunErr> {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || run(&p, &mut NoCaps))
}

fn int(src: &str) -> Value {
    exec(src).unwrap()
}

fn fails(src: &str, msg: &str) {
    assert_eq!(exec(src).unwrap_err().msg, msg);
}

#[test]
fn fib() {
    let src = "fn fib(n: Int) -> Int { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }
               fn main() -> Int { fib(20) }";
    assert_eq!(int(src), Value::Int(6765));
}

#[test]
fn mutual_recursion() {
    let src = "fn main() -> Bool { is_even(10) && !is_even(7) }
               fn is_even(n: Int) -> Bool { if n == 0 { true } else { is_odd(n - 1) } }
               fn is_odd(n: Int) -> Bool { if n == 0 { false } else { is_even(n - 1) } }";
    assert_eq!(int(src), Value::Bool(true));
}

#[test]
fn short_circuit() {
    assert_eq!(int("fn main() -> Bool { false && 1 / 0 == 0 }"), Value::Bool(false));
    assert_eq!(int("fn main() -> Bool { true || 1 / 0 == 0 }"), Value::Bool(true));
    fails("fn main() -> Bool { true && 1 / 0 == 0 }", "division by zero");
}

#[test]
fn values_of_each_type() {
    assert_eq!(int("fn main() {}"), Value::Unit);
    assert_eq!(int("fn main() -> Str { \"a\\n\" + \"b\" }"), Value::Str("a\nb".into()));
    assert_eq!(
        int("fn main() -> Bool { \"a\" == \"a\" && () == () && true != false }"),
        Value::Bool(true)
    );
    assert_eq!(
        int("fn main() -> Bool { 1 < 2 && 2 <= 2 && 3 > 2 && 3 >= 3 }"),
        Value::Bool(true)
    );
}

#[test]
fn division_truncates_toward_zero() {
    assert_eq!(int("fn main() -> Int { -7 / 2 }"), Value::Int(-3));
    assert_eq!(int("fn main() -> Int { -7 % 2 }"), Value::Int(-1));
}

#[test]
fn loops_and_assignment() {
    let src = "fn main() -> Int {
        let mut i = 0; let mut sum = 0;
        while i < 5 { i += 1; sum += i; }
        sum -= 1; sum
    }";
    assert_eq!(int(src), Value::Int(14));
}

#[test]
fn early_return_and_shadowing() {
    let src = "fn f(n: Int) -> Int {
        let x = 1;
        { let x = 5; if n > 0 { return x; } }
        x
    }
    fn main() -> Int { f(1) * 10 + f(0) }";
    assert_eq!(int(src), Value::Int(51));
}

#[test]
fn return_inside_while() {
    let src = "fn main() -> Int { let mut i = 0; while true { i += 1; if i == 3 { return i; } } 0 }";
    assert_eq!(int(src), Value::Int(3));
}

#[test]
fn for_loops() {
    let sum = "fn main() -> Int { let mut s = 0; for x in [1, 2, 3] { s += x; } s }";
    assert_eq!(int(sum), Value::Int(6));
    let range = "fn main() -> Int { let mut s = 0; for i in 2..5 { s += i; } for i in 5..2 { s += 100; } s }";
    assert_eq!(int(range), Value::Int(9));
}

#[test]
fn break_and_continue() {
    let src = "fn main() -> Int { let mut s = 0;
        for i in 0..10 { if i == 2 { continue; } if i == 5 { break; } s += i; }
        let mut j = 0;
        while true { j += 1; if j < 3 { continue; } if j == 6 { break; } s += 100; }
        s }";
    assert_eq!(int(src), Value::Int(8 + 300));
}

#[test]
fn break_leaves_only_the_inner_loop() {
    let src = "fn main() -> Int { let mut s = 0;
        for i in 0..3 { for j in 0..10 { if j == 2 { break; } s += 1; } s += 10; }
        s }";
    assert_eq!(int(src), Value::Int(36));
}

#[test]
fn for_iterates_over_a_snapshot() {
    let src = "fn main() -> Int { let mut xs = [1, 2]; let mut n = 0;
        for x in xs { xs.push(x); n += 1; }
        n * 10 + xs.len() }";
    assert_eq!(int(src), Value::Int(24));
}

#[test]
fn return_inside_for_and_locals_are_scoped() {
    let src = "fn f() -> Int { for x in [4, 5] { let y = x; if y == 5 { return y; } } 0 }
        fn main() -> Int { let a = f(); for x in [1] { let z = 1; if z == 1 { continue; } } a }";
    assert_eq!(int(src), Value::Int(5));
}

#[test]
fn arguments_are_evaluated_in_order_per_call() {
    let src = "fn sub(a: Int, b: Int) -> Int { a - b } fn main() -> Int { sub(10, 3) }";
    assert_eq!(int(src), Value::Int(7));
}

#[test]
fn overflow() {
    fails("fn main() -> Int { 9223372036854775807 + 1 }", "Int overflow");
    fails("fn main() -> Int { 0 - 9223372036854775807 - 2 }", "Int overflow");
    fails("fn main() -> Int { 9223372036854775807 * 2 }", "Int overflow");
    fails("fn main() -> Int { -(0 - 9223372036854775807 - 1) }", "Int overflow");
    fails(
        "fn main() -> Int { (0 - 9223372036854775807 - 1) / -1 }",
        "Int overflow",
    );
    fails(
        "fn main() -> Int { (0 - 9223372036854775807 - 1) % -1 }",
        "Int overflow",
    );
    fails("fn main() { let mut x = 9223372036854775807; x += 1; }", "Int overflow");
}

#[test]
fn division_by_zero() {
    let src = "fn main() -> Int { 1 / 0 }";
    let e = exec(src).unwrap_err();
    assert_eq!(e.msg, "division by zero");
    assert_eq!(&src[e.span.start..e.span.end], "1 / 0");
    fails("fn main() -> Int { 1 % 0 }", "division by zero");
}

#[test]
fn evaluation_depth() {
    let down = |n: i64| {
        format!(
            "fn down(n: Int) -> Int {{ if n == 0 {{ 0 }} else {{ down(n - 1) }} }} fn main() -> Int {{ down({n}) }}"
        )
    };
    assert_eq!(int(&down(1_000)), Value::Int(0));
    fails(&down(100_000), "evaluation is too deep");
    // The deepest nesting the parser accepts still stops cleanly at the budget.
    let e = format!("{}down(n - 1)", "- ".repeat(58));
    let nested =
        format!("fn down(n: Int) -> Int {{ if n == 0 {{ 0 }} else {{ {e} }} }} fn main() -> Int {{ down(100000) }}");
    fails(&nested, "evaluation is too deep");
}

const P: &str = "struct P { a: Int, b: Q } struct Q { c: Int } ";

fn p_main(body: &str) -> Value {
    int(&format!("{P}{body}"))
}

#[test]
fn struct_value_and_field_read() {
    let v = p_main("fn main() -> Q { let p = P { b: Q { c: 7 }, a: 1 }; p.b }");
    assert_eq!(v, Value::Struct("Q".into(), vec![Value::Int(7)]));
    let v = p_main("fn main() -> P { P { b: Q { c: 7 }, a: 1 } }");
    let q = Value::Struct("Q".into(), vec![Value::Int(7)]);
    assert_eq!(v, Value::Struct("P".into(), vec![Value::Int(1), q]));
    assert_eq!(
        p_main("fn main() -> Int { P { a: 1, b: Q { c: 7 } }.b.c }"),
        Value::Int(7)
    );
}

#[test]
fn field_assignment() {
    let src = "fn main() -> Int {
        let mut p = P { a: 1, b: Q { c: 2 } };
        p.b.c = 10; p.b.c += 5; p.a -= 4;
        p.a + p.b.c
    }";
    assert_eq!(p_main(src), Value::Int(12));
    fails(
        &format!("{P}fn main() {{ let mut q = Q {{ c: 9223372036854775807 }}; q.c += 1; }}"),
        "Int overflow",
    );
}

#[test]
fn let_copies_structs() {
    let src = "fn main() -> Int {
        let a = Q { c: 1 }; let mut b = a; b.c = 2;
        a.c * 10 + b.c
    }";
    assert_eq!(p_main(src), Value::Int(12));
}

#[test]
fn arguments_are_copied() {
    let src = "fn bump(mut_q: Q) -> Int { let mut q = mut_q; q.c += 1; q.c }
               fn main() -> Int { let q = Q { c: 1 }; bump(q) * 10 + q.c }";
    assert_eq!(p_main(src), Value::Int(21));
    let src = "fn set(p: P) -> P { let mut p = p; p.b.c = 5; p }
               fn main() -> Int { let p = P { a: 0, b: Q { c: 1 } }; let r = set(p); p.b.c * 10 + r.b.c }";
    assert_eq!(p_main(src), Value::Int(15));
}

const S: &str = "enum S { Done, Due(Int), Pair(Int, Str) } ";

fn s_main(body: &str) -> Value {
    int(&format!("{S}{body}"))
}

#[test]
fn enum_values() {
    let v = s_main("fn main() -> S { S::Pair(3, \"x\") }");
    assert_eq!(
        v,
        Value::Enum("S".into(), 2, vec![Value::Int(3), Value::Str("x".into())])
    );
    assert_eq!(s_main("fn main() -> S { S::Done }"), Value::Enum("S".into(), 0, vec![]));
    let v = s_main("fn main() -> S { S::Due(1 + 1) }");
    assert_eq!(v, Value::Enum("S".into(), 1, vec![Value::Int(2)]));
}

#[test]
fn match_on_enum_with_payloads() {
    let src = "fn f(s: S) -> Int {
                   match s { S::Done => 0, Due(n) => n, S::Pair(a, _) => a * 10 }
               }
               fn main() -> Int { f(S::Done) + f(S::Due(4)) + f(S::Pair(2, \"q\")) }";
    assert_eq!(s_main(src), Value::Int(24));
}

#[test]
fn bare_variant_and_catch_all_patterns() {
    let src = "fn f(s: S) -> Int { match s { Done => 1, other => match other { Due(_) => 2, _ => 3 } } }
               fn main() -> Int { f(S::Done) * 100 + f(S::Due(0)) * 10 + f(S::Pair(0, \"\")) }";
    assert_eq!(s_main(src), Value::Int(123));
}

#[test]
fn match_literals_in_order() {
    let src = "fn f(n: Int) -> Str { match n { 0 => \"zero\", -1 => \"neg\", x => \"other\" } }
               fn g(b: Bool) -> Int { match b { true => 1, false => 0 } }
               fn h(t: Str) -> Int { match t { \"a\" => 1, _ => 2 } }
               fn main() -> Str { f(0) + f(-1) + f(5) }";
    assert_eq!(s_main(src), Value::Str("zeronegother".into()));
    assert_eq!(
        s_main("fn g(b: Bool) -> Int { match b { true => 1, false => 0 } } fn main() -> Int { g(false) }"),
        Value::Int(0)
    );
    assert_eq!(
        s_main("fn main() -> Int { match \"b\" { \"a\" => 1, _ => 2 } }"),
        Value::Int(2)
    );
}

#[test]
fn arm_bindings_are_scoped_to_the_arm() {
    let src = "fn main() -> Int {
                   let x = 100;
                   let r = match S::Pair(1, \"a\") { S::Pair(x, _) => x + 1, _ => 0 };
                   let y = match 5 { n => n };
                   r + x + y
               }";
    assert_eq!(s_main(src), Value::Int(107));
}

#[test]
fn match_with_return_arm() {
    let src = "fn f(s: S) -> Int { let n = match s { Due(n) => n, _ => { return 0; } }; n + 1 }
               fn main() -> Int { f(S::Due(4)) * 10 + f(S::Done) }";
    assert_eq!(s_main(src), Value::Int(50));
}

#[test]
fn enums_are_copied() {
    let src = "struct W { s: S }
               fn main() -> Int {
                   let a = W { s: S::Due(1) };
                   let mut b = a;
                   b.s = S::Due(2);
                   let x = match a.s { Due(n) => n, _ => 0 };
                   let y = match b.s { Due(n) => n, _ => 0 };
                   x * 10 + y
               }";
    assert_eq!(s_main(src), Value::Int(12));
}

fn some(v: Value) -> Value {
    Value::Enum("Option".into(), 1, vec![v])
}

fn none() -> Value {
    Value::Enum("Option".into(), 0, vec![])
}

#[test]
fn option_and_result_values() {
    assert_eq!(int("fn main() -> Option<Int> { Some(1) }"), some(Value::Int(1)));
    assert_eq!(int("fn main() -> Option<Int> { None }"), none());
    let ok = Value::Enum("Result".into(), 0, vec![Value::Unit]);
    assert_eq!(int("fn main() -> Result<(), Str> { Ok(()) }"), ok);
    let err = Value::Enum("Result".into(), 1, vec![Value::Str("e".into())]);
    assert_eq!(int("fn main() -> Result<(), Str> { return Err(\"e\"); }"), err);
    assert_eq!(int("fn main() -> Option<Int> { let x: Option<Int> = None; x }"), none());
}

#[test]
fn try_propagates_through_calls() {
    let src = "fn div(a: Int, b: Int) -> Option<Int> { if b == 0 { None } else { Some(a / b) } }
               fn mid(a: Int, b: Int) -> Option<Int> { let q = div(a, b)?; Some(q + 1) }
               fn main() -> Option<Int> { let x = mid(10, 2)?; let y = mid(x, 0)?; Some(y) }";
    assert_eq!(int(src), none());
    let src = "fn div(a: Int, b: Int) -> Option<Int> { if b == 0 { None } else { Some(a / b) } }
               fn mid(a: Int, b: Int) -> Option<Int> { let q = div(a, b)?; Some(q + 1) }
               fn main() -> Option<Int> { let x = mid(10, 2)?; mid(x, 1) }";
    assert_eq!(int(src), some(Value::Int(7)));
    let src = "fn parse(s: Str) -> Result<Int, Str> { if s == \"1\" { Ok(1) } else { Err(\"bad \" + s) } }
               fn add(a: Str, b: Str) -> Result<Int, Str> { Ok(parse(a)? + parse(b)?) }
               fn main() -> Result<Int, Str> { let x = add(\"1\", \"1\")?; add(\"1\", \"z\") }";
    assert_eq!(
        int(src),
        Value::Enum("Result".into(), 1, vec![Value::Str("bad z".into())])
    );
}

#[test]
fn match_on_option_and_result() {
    let src = "fn f(o: Option<Int>) -> Int { match o { Some(x) => x, None => -1 } }
               fn g(r: Result<Int, Str>) -> Int { match r { Result::Ok(x) => x, Result::Err(_) => 0 } }
               fn main() -> Int { f(Some(5)) + f(None) + g(Ok(10)) + g(Err(\"e\")) }";
    assert_eq!(int(src), Value::Int(14));
}

#[test]
fn methods() {
    let round = "fn main() -> Str { \"h\u{e9}llo \u{4e16}\".to_bytes().to_str() }";
    assert_eq!(int(round), Value::Str("h\u{e9}llo \u{4e16}".into()));
    let src = "fn main() -> Int { \"h\u{e9}\".len() * 10 + \"h\u{e9}\".to_bytes().len() }";
    assert_eq!(int(src), Value::Int(33));
    assert_eq!(int("fn main() -> Str { (-42).to_str() }"), Value::Str("-42".into()));
    assert_eq!(
        int("fn main() -> Bytes { \"a\".to_bytes() }"),
        Value::Bytes(vec![97].into())
    );
    assert_eq!(
        int("fn main() -> Bool { \"a\".to_bytes() == \"a\".to_bytes() }"),
        Value::Bool(true)
    );
    assert_eq!(
        int("fn main() -> Bool { \"a\".to_bytes() != \"b\".to_bytes() }"),
        Value::Bool(true)
    );
}

#[test]
fn list_values_and_indexing() {
    let ints = |ns: &[i64]| Value::List(ns.iter().map(|n| Value::Int(*n)).collect::<Vec<_>>().into());
    assert_eq!(int("fn main() -> List<Int> { [1, 2 + 1,] }"), ints(&[1, 3]));
    assert_eq!(int("fn main() -> List<Int> { [] }"), ints(&[]));
    assert_eq!(
        int("fn main() -> Int { [[1, 2], [3]][0][1] + [5].len() }"),
        Value::Int(3)
    );
    assert_eq!(int("fn main() -> Int { let xs = [7, 8]; xs[1 - 1] }"), Value::Int(7));
}

#[test]
fn list_out_of_bounds() {
    let src = "fn main() -> Int { let xs = [1, 2, 3]; xs[5] }";
    let e = exec(src).unwrap_err();
    assert_eq!(e.msg, "index 5 out of bounds for length 3");
    assert_eq!(&src[e.span.start..e.span.end], "xs[5]");
    fails("fn main() -> Int { [1][0 - 1] }", "index -1 out of bounds for length 1");
    let src = "fn main() { let mut xs = [1]; xs[2] = 0; }";
    let e = exec(src).unwrap_err();
    assert_eq!(e.msg, "index 2 out of bounds for length 1");
    assert_eq!(&src[e.span.start..e.span.end], "xs[2]");
    fails(
        "fn main() { let mut xs = [[1]]; xs[0][1] += 1; }",
        "index 1 out of bounds for length 1",
    );
}

#[test]
fn list_assignment_and_push() {
    let src = "struct S { items: List<Int> }
               fn main() -> Int {
                   let mut xs = [S { items: [1, 2] }];
                   xs[0].items[1] = 10;
                   xs[0].items[0] += 5;
                   xs[0].items.push(3);
                   xs.push(S { items: [] });
                   xs[1].items.push(4);
                   xs[0].items[0] + xs[0].items[1] + xs[0].items[2] + xs[1].items[0] + xs.len()
               }";
    assert_eq!(int(src), Value::Int(6 + 10 + 3 + 4 + 2));
    assert_eq!(
        int("fn main() -> List<Int> { let mut xs = [1]; xs.push(2); xs }"),
        Value::List(vec![Value::Int(1), Value::Int(2)].into())
    );
}

#[test]
fn list_index_is_evaluated_before_the_value() {
    fails(
        "fn main() { let mut xs = [1]; xs[1 / 0] = 9223372036854775807 + 1; }",
        "division by zero",
    );
    fails("fn main() { let mut xs = [1]; xs[5] = 1 / 0; }", "division by zero");
}

#[test]
fn lists_are_copied() {
    let src = "fn main() -> Int {
                   let mut a = [1, 2];
                   let mut b = a;
                   b[0] = 9;
                   a.push(3);
                   a[1] = 7;
                   a[0] * 1000 + b[0] * 100 + a.len() * 10 + b.len()
               }";
    assert_eq!(int(src), Value::Int(1000 + 900 + 30 + 2));
    let src = "fn main() -> Int {
                   let mut a = [[1], [2]];
                   let mut b = a;
                   b[0][0] = 5;
                   a[1].push(6);
                   a[0][0] * 100 + b[0][0] * 10 + b[1].len() + a[1].len() * 1000
               }";
    assert_eq!(int(src), Value::Int(2000 + 100 + 50 + 1));
    let src = "struct W { xs: List<Int> }
               fn main() -> Int {
                   let a = W { xs: [1] };
                   let mut b = a;
                   b.xs.push(2);
                   b.xs[0] = 9;
                   a.xs[0] * 10 + a.xs.len() + b.xs[0] * 100 + b.xs.len() * 1000
               }";
    assert_eq!(int(src), Value::Int(10 + 1 + 900 + 2000));
}

#[test]
fn list_argument_changed_in_callee() {
    let src = "fn bump(xs: List<Int>) -> List<Int> { let mut ys = xs; ys[0] += 1; ys.push(0); ys }
               fn main() -> Int {
                   let a = [1];
                   let b = bump(a);
                   a[0] * 100 + a.len() * 10 + b[0] + b.len() * 1000
               }";
    assert_eq!(int(src), Value::Int(100 + 10 + 2 + 2000));
}

#[test]
fn word_count() {
    let src = "fn count(words: List<Str>) -> Map<Str, Int> {
                   let mut m: Map<Str, Int> = Map::new();
                   for w in words {
                       let n = match m.get(w) { Some(n) => n, None => 0 };
                       m.insert(w, n + 1);
                   }
                   m
               }
               fn main() -> Int {
                   let m = count([\"b\", \"a\", \"b\", \"b\"]);
                   let a = match m.get(\"a\") { Some(n) => n, None => 0 };
                   let b = match m.get(\"b\") { Some(n) => n, None => 0 };
                   a * 10 + b * 100 + m.len()
               }";
    assert_eq!(int(src), Value::Int(10 + 300 + 2));
}

#[test]
fn map_keys_are_sorted() {
    let src = "fn main() -> List<Str> {
                   let mut m: Map<Str, Int> = Map::new();
                   m.insert(\"b\", 1); m.insert(\"a\", 2); m.insert(\"c\", 3);
                   m.keys()
               }";
    let strs = ["a", "b", "c"].map(|s| Value::Str(s.into()));
    assert_eq!(int(src), Value::List(strs.to_vec().into()));
    let src = "fn main() -> List<Int> {
                   let mut m: Map<Int, Str> = Map::new();
                   m.insert(3, \"x\"); m.insert(-1, \"y\"); m.insert(2, \"z\");
                   m.keys()
               }";
    assert_eq!(int(src), Value::List([-1, 2, 3].map(Value::Int).to_vec().into()));
}

#[test]
fn map_get_insert_remove() {
    let src = "fn main() -> Int {
                   let mut m: Map<Int, Int> = Map::new();
                   m.insert(1, 10);
                   m.insert(1, 20);
                   m.insert(2, 5);
                   m.remove(2);
                   m.remove(9);
                   match m.get(1) { Some(v) => v + m.len() * 1000, None => 0 }
               }";
    assert_eq!(int(src), Value::Int(1020));
    let src = "fn main() -> Bool { let m: Map<Str, Int> = Map::new(); match m.get(\"x\") { Some(_) => false, None => true } }";
    assert_eq!(int(src), Value::Bool(true));
}

#[test]
fn map_in_nested_places() {
    let src = "struct S { m: Map<Str, List<Int>> }
               fn main() -> Int {
                   let mut s = S { m: Map::new() };
                   s.m.insert(\"a\", [1]);
                   let mut xs = [s];
                   xs[0].m.insert(\"b\", []);
                   xs[0].m.remove(\"a\");
                   xs[0].m.len() * 10 + xs[0].m.keys().len()
               }";
    assert_eq!(int(src), Value::Int(11));
}

#[test]
fn maps_are_copied() {
    let src = "fn main() -> Int {
                   let mut a: Map<Int, Int> = Map::new();
                   a.insert(1, 1);
                   let mut b = a;
                   b.insert(2, 2);
                   a.insert(1, 7);
                   let x = match a.get(1) { Some(v) => v, None => 0 };
                   let y = match b.get(1) { Some(v) => v, None => 0 };
                   x * 100 + y * 10 + a.len() * 1000 + b.len() * 10000
               }";
    assert_eq!(int(src), Value::Int(700 + 10 + 1000 + 20000));
    // A value read with `get` is a copy.
    let src = "fn main() -> Int {
                   let mut m: Map<Int, List<Int>> = Map::new();
                   m.insert(1, [1]);
                   let mut xs: List<Int> = match m.get(1) { Some(v) => v, None => [] };
                   xs.push(2);
                   xs[0] = 9;
                   match m.get(1) { Some(v) => v[0] * 10 + v.len(), None => 0 }
               }";
    assert_eq!(int(src), Value::Int(11));
    let src = "fn put(m: Map<Int, Int>) -> Int { let mut n = m; n.insert(5, 5); n.len() }
               fn main() -> Int { let m: Map<Int, Int> = Map::new(); put(m) * 10 + m.len() }";
    assert_eq!(int(src), Value::Int(10));
}

fn strs(xs: &[&str]) -> Value {
    Value::List(xs.iter().map(|s| Value::Str((*s).into())).collect::<Vec<_>>().into())
}

#[test]
fn str_split() {
    assert_eq!(
        int("fn main() -> List<Str> { \"a,b,,c\".split(\",\") }"),
        strs(&["a", "b", "", "c"])
    );
    assert_eq!(int("fn main() -> List<Str> { \"\".split(\",\") }"), strs(&[""]));
    let src = "fn main() -> List<Str> { \"abc\".split(\"\") }";
    fails(src, "`split` needs a non-empty separator");
    let e = exec(src).unwrap_err();
    assert_eq!(&src[e.span.start..e.span.end], "\"abc\".split(\"\")");
}

#[test]
fn str_lines() {
    assert_eq!(
        int("fn main() -> List<Str> { \"a\r\nb\n\nc\n\".lines() }"),
        strs(&["a", "b", "", "c"])
    );
    assert_eq!(int("fn main() -> List<Str> { \"\".lines() }"), strs(&[]));
}

#[test]
fn str_trim_contains_starts_with() {
    assert_eq!(
        int("fn main() -> Str { \" \\t a b\\n\".trim() }"),
        Value::Str("a b".into())
    );
    let src =
        "fn main() -> Bool { \"hello\".contains(\"ell\") && !\"hello\".contains(\"x\") && \"hello\".contains(\"\") }";
    assert_eq!(int(src), Value::Bool(true));
    let src = "fn main() -> Bool { \"hello\".starts_with(\"he\") && !\"hello\".starts_with(\"el\") }";
    assert_eq!(int(src), Value::Bool(true));
}

#[test]
fn str_parse_int() {
    let parse = |s: &str| int(&format!("fn main() -> Option<Int> {{ \"{s}\".parse_int() }}"));
    assert_eq!(parse("42"), some(Value::Int(42)));
    assert_eq!(parse("-7"), some(Value::Int(-7)));
    assert_eq!(parse("+7"), some(Value::Int(7)));
    for bad in ["", "-", "+", "1.5", " 1", "1 ", "0x1", "9223372036854775808"] {
        assert_eq!(parse(bad), none(), "{bad:?}");
    }
    assert_eq!(parse("-9223372036854775808"), some(Value::Int(i64::MIN)));
}

#[test]
fn reads_in_place() {
    let loop_over = |n: i64| {
        format!(
            "fn main() -> Int {{ let mut xs: List<Int> = []; for i in 0..{n} {{ xs.push(i); }} \
             let mut s = 0; for i in 0..xs.len() {{ s += xs[i]; }} s }}"
        )
    };
    assert_eq!(int(&loop_over(40_000)), Value::Int(799_980_000));
    // An operand that assigns still sees the value the receiver had before it ran.
    let src = "fn main() -> Int { let mut xs: List<Int> = [1, 2]; xs[{ xs[0] = 9; 0 }] }";
    assert_eq!(int(src), Value::Int(1));
    let src = "fn main() -> Int { let mut xs: List<Int> = [1, 2]; \
               let a = [xs.len(), { xs.push(0); 0 }]; a[0] }";
    assert_eq!(int(src), Value::Int(2));
}

const JSON_STR: &str = "fn json_str(s: Str) -> Str {
    let b = s.to_bytes();
    let mut out = \"\\\"\".to_bytes();
    for i in 0..b.len() {
        let c = b[i];
        if c == 34 || c == 92 { out.push(92); }
        out.push(c);
    }
    out.push(34);
    out.to_str()
}";

#[test]
fn bytes_json_str() {
    let src = format!("{JSON_STR} fn main() -> Str {{ json_str(\"a\\\"b\\\\c\") }}");
    assert_eq!(int(&src), Value::Str("\"a\\\"b\\\\c\"".into()));
}

#[test]
fn bytes_percent_decode() {
    let src = "fn hex(c: Int) -> Int { if c >= 65 { c - 55 } else { c - 48 } }
        fn decode(s: Str) -> Str {
            let b = s.to_bytes();
            let mut out = \"\".to_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == 37 && i + 2 < b.len() {
                    out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            out.to_str()
        }
        fn main() -> Str { decode(\"%41b%42\") }";
    assert_eq!(int(src), Value::Str("AbB".into()));
}

#[test]
fn bytes_errors() {
    fails(
        "fn main() { let mut b = \"\".to_bytes(); b.push(256); }",
        "byte 256 out of range 0..=255",
    );
    fails(
        "fn main() { let mut b = \"\".to_bytes(); b.push(0 - 1); }",
        "byte -1 out of range 0..=255",
    );
    let src = "fn main() -> Int { let b = \"ab\".to_bytes(); b[2] }";
    let e = exec(src).unwrap_err();
    assert_eq!(e.msg, "index 2 out of bounds for length 2");
    assert_eq!(&src[e.span.start..e.span.end], "b[2]");
    fails(
        "fn main() -> Int { \"ab\".to_bytes()[0 - 1] }",
        "index -1 out of bounds for length 2",
    );
    fails(
        "fn main() -> Int { let xs = [\"a\".to_bytes()]; xs[0][1] }",
        "index 1 out of bounds for length 1",
    );
}

#[test]
fn option_unwrap_or() {
    assert_eq!(int("fn main() -> Int { Some(3).unwrap_or(9) }"), Value::Int(3));
    assert_eq!(
        int("fn main() -> Int { \"x\".parse_int().unwrap_or(9) }"),
        Value::Int(9)
    );
    let src = "fn main() -> Str { let m: Map<Str, Str> = Map::new(); m.get(\"k\").unwrap_or(\"d\") }";
    assert_eq!(int(src), Value::Str("d".into()));
}

const JSON: &str = r#"
enum Json { Null, Bool(Bool), Num(Int), Str(Str), Arr(List<Json>), Obj(Map<Str, Json>) }
fn encode(j: Json) -> Str {
    match j {
        Json::Null => "null",
        Json::Bool(b) => if b { "true" } else { "false" },
        Json::Num(n) => n.to_str(),
        Json::Str(s) => "\"" + s + "\"",
        Json::Arr(items) => {
            let mut out = "[";
            let mut i = 0;
            while i < items.len() {
                if i > 0 { out = out + ","; }
                out = out + encode(items[i]);
                i += 1;
            }
            out + "]"
        }
        Json::Obj(m) => {
            let mut out = "{";
            let mut first = true;
            for k in m.keys() {
                if !first { out = out + ","; }
                first = false;
                match m.get(k) {
                    Some(v) => { out = out + "\"" + k + "\":" + encode(v); }
                    None => {}
                }
            }
            out + "}"
        }
    }
}
fn doc() -> Json {
    let mut o: Map<Str, Json> = Map::new();
    o.insert("a", Json::Arr([Json::Num(1), Json::Null, Json::Arr([Json::Bool(true)])]));
    o.insert("b", Json::Str("x"));
    Json::Obj(o)
}
"#;

#[test]
fn recursive_json_encoder() {
    let src = format!("{JSON} fn main() -> Str {{ encode(doc()) }}");
    assert_eq!(int(&src), Value::Str(r#"{"a":[1,null,[true]],"b":"x"}"#.into()));
}

#[test]
fn recursive_value_displays() {
    let src = format!("{JSON} fn main() -> Json {{ doc() }}");
    let p = check(&src, &CapTable::default()).unwrap();
    let v = run(&p, &mut NoCaps).unwrap();
    let want = r#"Json::Obj({"a": Json::Arr([Json::Num(1), Json::Null, Json::Arr([Json::Bool(true)])]), "b": Json::Str("x")})"#;
    assert_eq!(p.display(&v), want);
}

#[test]
fn bytes_argument_changed_in_callee() {
    let src = "fn grow(b: Bytes) -> Bytes { let mut c = b; c.push(1); c }
               fn main() -> Int {
                   let a = \"x\".to_bytes();
                   let b = grow(a);
                   a.len() * 10 + b.len()
               }";
    assert_eq!(int(src), Value::Int(12));
}

#[test]
fn str_argument_changed_in_callee() {
    let src = "fn grow(s: Str) -> Str { let mut t = s; t += \"y\"; t }
               fn main() -> Int {
                   let a = \"x\";
                   let b = grow(a);
                   a.len() * 10 + b.len()
               }";
    assert_eq!(int(src), Value::Int(12));
}

#[test]
fn list_push_and_assign_in_callee() {
    let src = "fn grow(xs: List<Int>) -> List<Int> {
                   let mut ys = xs;
                   ys.push(9);
                   ys[0] = 5;
                   ys
               }
               fn main() -> Int {
                   let a = [1, 2];
                   let b = grow(a);
                   a.len() * 1000 + a[0] * 100 + b.len() * 10 + b[0]
               }";
    assert_eq!(int(src), Value::Int(2000 + 100 + 30 + 5));
}

#[test]
fn map_insert_and_remove_in_callee() {
    let src = "fn change(m: Map<Int, Int>) -> Map<Int, Int> {
                   let mut n = m;
                   n.insert(1, 7);
                   n.insert(2, 8);
                   n.remove(3);
                   n
               }
               fn main() -> Int {
                   let mut a: Map<Int, Int> = Map::new();
                   a.insert(1, 1);
                   a.insert(3, 3);
                   let b = change(a);
                   a.len() * 1000 + a.get(1).unwrap_or(0) * 100 + b.len() * 10 + b.get(1).unwrap_or(0)
               }";
    assert_eq!(int(src), Value::Int(2000 + 100 + 20 + 7));
}

#[test]
fn for_iterates_the_original_list() {
    let src = "fn main() -> Int {
                   let mut xs = [1, 2, 3];
                   let mut n = 0;
                   for x in xs {
                       xs.push(x);
                       n += 1;
                   }
                   n * 10 + xs.len()
               }";
    assert_eq!(int(src), Value::Int(36));
}

#[test]
fn option_result_predicates() {
    let src = "fn main() -> Int {
        let a: Option<Int> = Some(1);
        let b: Option<Int> = None;
        let c: Result<Int, Str> = Ok(5);
        let d: Result<Int, Str> = Err(\"e\");
        let mut n = 0;
        if a.is_some() { n += 1; }
        if b.is_none() { n += 10; }
        if c.is_ok() { n += 100; }
        if d.is_err() { n += 1000; }
        if a.is_none() || b.is_some() || c.is_err() || d.is_ok() { n += 10000; }
        n + c.unwrap_or(7) * 100000 + d.unwrap_or(7) * 1000000
    }";
    assert_eq!(int(src), Value::Int(1111 + 500000 + 7000000));
}

#[test]
fn structural_equality() {
    let pre = "struct P { a: Int, b: Str } enum E { A, B(Int) } ";
    for (expr, want) in [
        ("P { a: 1, b: \"x\" } == P { a: 1, b: \"x\" }", true),
        ("P { a: 1, b: \"x\" } == P { a: 2, b: \"x\" }", false),
        ("E::B(1) == E::B(1)", true),
        ("E::B(1) != E::B(2)", true),
        ("E::A == E::B(1)", false),
        ("Some(1) == Some(1)", true),
        (
            "{ let r: Result<Int, Int> = Ok(1); let e: Result<Int, Int> = Err(1); r == e }",
            false,
        ),
        ("[1, 2] == [1, 2]", true),
        ("[1, 2] != [1, 2, 3]", true),
    ] {
        let src = format!("{pre} fn main() -> Bool {{ {expr} }}");
        assert_eq!(int(&src), Value::Bool(want), "{expr}");
    }
    let src = "fn main() -> Bool {
        let mut a: Map<Str, Int> = Map::new();
        let mut b: Map<Str, Int> = Map::new();
        a.insert(\"k\", 1);
        b.insert(\"k\", 1);
        a == b
    }";
    assert_eq!(int(src), Value::Bool(true));
}
