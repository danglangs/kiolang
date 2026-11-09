mod common;

use lang::ir::{EnumId, EnumRef, StructId};
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
    // `main` calls `down(999)`, which recurses to `down(0)`: 1_000 calls in all.
    assert_eq!(int(&down(999)), Value::Int(0));
    let src = down(1_000);
    let e = exec(&src).unwrap_err();
    assert_eq!(e.msg, "evaluation is too deep");
    assert_eq!(&src[e.span.start..e.span.end], "down(n - 1)");
    // Recursion through `match`, a loop and `?` stops at the same count.
    let shapes = [
        "fn down(n: Int) -> Int { match n { 0 => 0, _ => { let x = [down(n - 1)]; x[0] } } }",
        "fn down(n: Int) -> Int { let mut s = 0; for i in 0..1 { while n > 0 { s = down(n - 1); break; } } s }",
    ];
    for f in shapes {
        assert_eq!(
            int(&format!("{f} fn main() -> Int {{ down(999) }}")),
            Value::Int(0),
            "{f}"
        );
        fails(
            &format!("{f} fn main() -> Int {{ down(100000) }}"),
            "evaluation is too deep",
        );
    }
    let r = "fn down(n: Int) -> Result<Int, Str> { if n == 0 { return Ok(0); } Ok(down(n - 1)?) }";
    fails(
        &format!("{r} fn main() -> Int {{ down(100000).unwrap_or(1) }}"),
        "evaluation is too deep",
    );
}

const P: &str = "struct P { a: Int, b: Q } struct Q { c: Int } ";

fn p_main(body: &str) -> Value {
    int(&format!("{P}{body}"))
}

#[test]
fn struct_value_and_field_read() {
    let v = p_main("fn main() -> Q { let p = P { b: Q { c: 7 }, a: 1 }; p.b }");
    assert_eq!(v, Value::Struct(StructId(1), vec![Value::Int(7)]));
    let v = p_main("fn main() -> P { P { b: Q { c: 7 }, a: 1 } }");
    let q = Value::Struct(StructId(1), vec![Value::Int(7)]);
    assert_eq!(v, Value::Struct(StructId(0), vec![Value::Int(1), q]));
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
        Value::Enum(EnumRef::User(EnumId(0)), 2, vec![Value::Int(3), Value::Str("x".into())])
    );
    assert_eq!(
        s_main("fn main() -> S { S::Done }"),
        Value::Enum(EnumRef::User(EnumId(0)), 0, vec![])
    );
    let v = s_main("fn main() -> S { S::Due(1 + 1) }");
    assert_eq!(v, Value::Enum(EnumRef::User(EnumId(0)), 1, vec![Value::Int(2)]));
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
    Value::Enum(EnumRef::Option, 1, vec![v])
}

fn none() -> Value {
    Value::Enum(EnumRef::Option, 0, vec![])
}

#[test]
fn option_and_result_values() {
    assert_eq!(int("fn main() -> Option<Int> { Some(1) }"), some(Value::Int(1)));
    assert_eq!(int("fn main() -> Option<Int> { None }"), none());
    let ok = Value::Enum(EnumRef::Result, 0, vec![Value::Unit]);
    assert_eq!(int("fn main() -> Result<(), Str> { Ok(()) }"), ok);
    let err = Value::Enum(EnumRef::Result, 1, vec![Value::Str("e".into())]);
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
        Value::Enum(EnumRef::Result, 1, vec![Value::Str("bad z".into())])
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
fn nested_patterns() {
    // Each L4a example; `IoErr` comes from the capability table.
    let src = "fn f(r: Result<Int, IoErr>) -> Int { match r { Err(IoErr::Exists) => 1, Err(_) => 2, Ok(n) => n } }
               fn main() -> Int { f(Err(IoErr::Exists)) * 100 + f(Err(IoErr::Denied)) * 10 + f(Ok(7)) }";
    let p = check(src, &CapTable::v0()).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(run(&p, &mut NoCaps).unwrap(), Value::Int(127));
    let src = "fn f(r: Result<Bool, Str>) -> Int { match r { Ok(true) => 1, Ok(false) => 2, Err(_) => 3 } }
               fn g(o: Option<Int>) -> Int { match o { Some(0) => 1, Some(n) => n, None => -1 } }
               fn h(o: Option<Option<Int>>) -> Int { match o { Some(Some(x)) => x, Some(None) => 0, None => -1 } }
               fn main() -> List<Int> { [f(Ok(true)), f(Ok(false)), f(Err(\"e\")), g(Some(0)), g(Some(5)), g(None), h(Some(Some(9))), h(Some(None)), h(None)] }";
    let want = [1, 2, 3, 1, 5, -1, 9, 0, -1].map(Value::Int).to_vec();
    assert_eq!(int(src), Value::List(want.into()));
    let src = "enum Shape { Line(Int, Int), Dot }
               enum Req { Get(Str, Option<Shape>), Other }
               fn f(s: Shape) -> Int { match s { Shape::Line(1, _) => 1, Shape::Line(a, b) => a + b, Shape::Dot => 0 } }
               fn g(r: Req) -> Str { match r { Req::Get(\"GET\", Some(Shape::Line(_, n))) => n.to_str(), Req::Get(\"GET\", _) => \"get\", Req::Get(m, _) => m, Req::Other => \"other\" } }
               fn main() -> Str { f(Shape::Line(1, 9)).to_str() + f(Shape::Line(2, 3)).to_str() + g(Req::Get(\"GET\", Some(Shape::Line(0, 4)))) + g(Req::Get(\"GET\", None)) + g(Req::Get(\"PUT\", None)) + g(Req::Other) }";
    assert_eq!(int(src), Value::Str("154getPUTother".into()));
}

#[test]
fn nested_bindings_are_scoped_and_moved() {
    let src = "fn main() -> Int {
                   let x = 100;
                   let o: Option<Result<Int, Str>> = Some(Ok(1));
                   let r = match o { Some(Ok(x)) => x + 1, Some(Err(_)) => 0, None => 0 };
                   r + x
               }";
    assert_eq!(int(src), Value::Int(102));
    let src = "fn main() -> List<Str> { let o = Some([\"a\"]); match o { Some(xs) => { let mut ys = xs; ys.push(\"b\"); ys }, None => [] } }";
    assert_eq!(
        int(src),
        Value::List(vec![Value::Str("a".into()), Value::Str("b".into())].into())
    );
}

#[test]
fn if_let_runs() {
    let src = "fn f(o: Option<Int>) -> Int { if let Some(x) = o { x } else { -1 } }
               fn g(o: Option<Int>) -> Int { let mut n = 0; if let Some(x) = o { n = x; } n }
               fn h(o: Option<Int>, r: Result<Int, Str>) -> Int { if let Some(x) = o { x } else if let Ok(y) = r { y * 10 } else if o.is_none() { 7 } else { 8 } }
               fn main() -> List<Int> { [f(Some(3)), f(None), g(Some(4)), g(None), h(Some(1), Ok(2)), h(None, Ok(2)), h(None, Err(\"e\"))] }";
    let want = [3, -1, 4, 0, 1, 20, 7].map(Value::Int).to_vec();
    assert_eq!(int(src), Value::List(want.into()));
    let src = "fn main() -> Int { let x = 5; if let Some(Some(x)) = Some(Some(1)) { x + 10 } else { x } }";
    assert_eq!(int(src), Value::Int(11));
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

#[test]
fn loop_break_continue() {
    let src = "fn main() -> Int {
        let mut i = 0;
        let mut s = 0;
        loop {
            i += 1;
            if i < 3 { continue; }
            if i == 6 { break; }
            s += i;
        }
        s * 100 + i
    }";
    assert_eq!(int(src), Value::Int(12 * 100 + 6));
    assert_eq!(
        int("fn f() -> Int { let mut i = 0; loop { i += 1; if i == 4 { return i; } } } fn main() -> Int { f() }"),
        Value::Int(4)
    );
}

#[test]
fn return_break_continue_as_match_arms() {
    let src = "fn find(xs: List<Int>) -> Int {
          let mut s = 0;
          for x in xs {
            match x { 0 => continue, 100 => break, _ => { s += x; } }
          }
          s
        }
        fn first(o: Option<Int>) -> Int { match o { Some(n) => return n, None => return -1 } }
        fn unit(o: Option<Int>) { match o { Some(_) => return, None => {} } }
        fn main() -> Int {
          unit(Some(1));
          find([1, 0, 2, 100, 5]) * 100 + first(Some(4)) * 10 - first(None)
        }";
    assert_eq!(int(src), Value::Int(300 + 40 + 1));
}

#[test]
fn break_in_a_match_arm_outside_a_loop_is_rejected() {
    let src = "fn main() { match Some(1) { Some(_) => break, None => {} } }";
    let d = check(src, &CapTable::default()).unwrap_err().remove(0);
    assert_eq!(d.msg, "`break` outside a loop");
    assert_eq!(d.hint.as_deref(), Some("use `return` to leave a function"));
}

fn some_int(n: i64) -> Value {
    Value::Enum(EnumRef::Option, 1, vec![Value::Int(n)])
}

fn some_str(s: &str) -> Value {
    Value::Enum(EnumRef::Option, 1, vec![Value::Str(s.into())])
}

fn none_val() -> Value {
    Value::Enum(EnumRef::Option, 0, vec![])
}

#[test]
fn str_ends_with() {
    assert_eq!(
        int("fn main() -> Bool { \"abc\".ends_with(\"bc\") }"),
        Value::Bool(true)
    );
    assert_eq!(int("fn main() -> Bool { \"abc\".ends_with(\"\") }"), Value::Bool(true));
    assert_eq!(int("fn main() -> Bool { \"\".ends_with(\"a\") }"), Value::Bool(false));
    assert_eq!(
        int("fn main() -> Bool { \"abc\".ends_with(\"abcd\") }"),
        Value::Bool(false)
    );
}

#[test]
fn str_strip_prefix_and_suffix() {
    assert_eq!(
        int("fn main() -> Option<Str> { \"abc\".strip_prefix(\"ab\") }"),
        some_str("c")
    );
    assert_eq!(
        int("fn main() -> Option<Str> { \"abc\".strip_prefix(\"abc\") }"),
        some_str("")
    );
    assert_eq!(
        int("fn main() -> Option<Str> { \"abc\".strip_prefix(\"\") }"),
        some_str("abc")
    );
    assert_eq!(
        int("fn main() -> Option<Str> { \"abc\".strip_prefix(\"b\") }"),
        none_val()
    );
    assert_eq!(int("fn main() -> Option<Str> { \"\".strip_prefix(\"a\") }"), none_val());
    assert_eq!(
        int("fn main() -> Option<Str> { \"abc\".strip_suffix(\"bc\") }"),
        some_str("a")
    );
    assert_eq!(
        int("fn main() -> Option<Str> { \"abc\".strip_suffix(\"\") }"),
        some_str("abc")
    );
    assert_eq!(
        int("fn main() -> Option<Str> { \"abc\".strip_suffix(\"b\") }"),
        none_val()
    );
}

#[test]
fn str_find() {
    assert_eq!(int("fn main() -> Option<Int> { \"abcb\".find(\"b\") }"), some_int(1));
    assert_eq!(int("fn main() -> Option<Int> { \"abc\".find(\"\") }"), some_int(0));
    assert_eq!(int("fn main() -> Option<Int> { \"\".find(\"\") }"), some_int(0));
    assert_eq!(int("fn main() -> Option<Int> { \"abc\".find(\"z\") }"), none_val());
    assert_eq!(int("fn main() -> Option<Int> { \"\".find(\"a\") }"), none_val());
    // A byte offset, not a character index.
    assert_eq!(int("fn main() -> Option<Int> { \"éa\".find(\"a\") }"), some_int(2));
}

#[test]
fn str_replace() {
    assert_eq!(
        int("fn main() -> Str { \"abab\".replace(\"b\", \"xy\") }"),
        Value::Str("axyaxy".into())
    );
    assert_eq!(
        int("fn main() -> Str { \"aaa\".replace(\"aa\", \"b\") }"),
        Value::Str("ba".into())
    );
    assert_eq!(
        int("fn main() -> Str { \"abc\".replace(\"z\", \"y\") }"),
        Value::Str("abc".into())
    );
    assert_eq!(
        int("fn main() -> Str { \"abc\".replace(\"\", \"-\") }"),
        Value::Str("-a-b-c-".into())
    );
    assert_eq!(
        int("fn main() -> Str { \"\".replace(\"\", \"-\") }"),
        Value::Str("-".into())
    );
    assert_eq!(
        int("fn main() -> Str { \"abc\".replace(\"b\", \"\") }"),
        Value::Str("ac".into())
    );
}

#[test]
fn list_contains() {
    assert_eq!(int("fn main() -> Bool { [1, 2].contains(2) }"), Value::Bool(true));
    assert_eq!(int("fn main() -> Bool { [1, 2].contains(3) }"), Value::Bool(false));
    let empty = "fn main() -> Bool { let xs: List<Int> = []; xs.contains(1) }";
    assert_eq!(int(empty), Value::Bool(false));
    assert_eq!(int("fn main() -> Bool { [\"a\"].contains(\"a\") }"), Value::Bool(true));
    let src = "struct P { x: Int } fn main() -> Bool { [P { x: 1 }].contains(P { x: 1 }) }";
    assert_eq!(int(src), Value::Bool(true));
}

#[test]
fn list_sort() {
    let sorted = "fn main() -> List<Int> { let mut xs = [3, -1, 2, 2]; xs.sort(); xs }";
    assert_eq!(
        int(sorted),
        Value::List(vec![Value::Int(-1), Value::Int(2), Value::Int(2), Value::Int(3)].into())
    );
    assert_eq!(
        int("fn main() -> List<Str> { let mut xs = [\"b\", \"B\", \"a\", \"\", \"ab\"]; xs.sort(); xs }"),
        strs(&["", "B", "a", "ab", "b"])
    );
    let empty = "fn main() -> List<Int> { let mut xs: List<Int> = []; xs.sort(); xs }";
    assert_eq!(int(empty), Value::List(vec![].into()));
    let nested = "struct S { v: List<Int> } fn main() -> List<Int> { let mut s = S { v: [2, 1] }; s.v.sort(); s.v }";
    assert_eq!(int(nested), Value::List(vec![Value::Int(1), Value::Int(2)].into()));
    // Bytes order: uppercase before lowercase, multibyte after ASCII.
    assert_eq!(
        int("fn main() -> List<Str> { let mut xs = [\"é\", \"z\"]; xs.sort(); xs }"),
        strs(&["z", "é"])
    );
}

#[test]
fn list_sort_and_pop_copy() {
    let src = "fn main() -> List<Int> { let a = [2, 1]; let mut b = a; b.sort(); a }";
    assert_eq!(int(src), Value::List(vec![Value::Int(2), Value::Int(1)].into()));
    let src = "fn main() -> Int { let a = [2, 1]; let mut b = a; b.sort(); b[0] }";
    assert_eq!(int(src), Value::Int(1));
    let src = "fn main() -> Int { let a = [1, 2]; let mut b = a; b.pop(); a.len() * 10 + b.len() }";
    assert_eq!(int(src), Value::Int(21));
}

#[test]
fn list_pop() {
    let src = "fn main() -> Option<Int> { let mut xs = [1, 2]; xs.pop() }";
    assert_eq!(int(src), some_int(2));
    let src = "fn main() -> Int { let mut xs = [1, 2]; xs.pop(); xs.len() }";
    assert_eq!(int(src), Value::Int(1));
    let src = "fn main() -> Option<Int> { let mut xs: List<Int> = []; xs.pop() }";
    assert_eq!(int(src), none_val());
    let src = "struct S { v: List<Int> } fn main() -> Option<Int> { let mut s = S { v: [1, 5] }; s.v.pop() }";
    assert_eq!(int(src), some_int(5));
}

#[test]
fn result_ok_and_err() {
    let src = "fn main() -> Int {
        let c: Result<Int, Str> = Ok(5);
        let d: Result<Int, Str> = Err(\"e\");
        c.ok().unwrap_or(0) + d.ok().unwrap_or(10) + c.err().unwrap_or(\"\").len() + d.err().unwrap_or(\"\").len()
    }";
    assert_eq!(int(src), Value::Int(16));
}

#[test]
fn option_ok_or() {
    let src = "fn f(o: Option<Int>) -> Result<Int, Str> { let x = o.ok_or(\"none\")?; Ok(x + 1) }
        fn main() -> Int {
            let a = f(Some(1)).unwrap_or(-1);
            let b = f(None);
            let n = if b.is_err() { 10 } else { 0 };
            a + n + b.err().unwrap_or(\"\").len() * 100
        }";
    assert_eq!(int(src), Value::Int(2 + 10 + 400));
}

#[test]
fn ok_or_takes_its_error_type_from_the_argument() {
    let src = "fn f(o: Option<Int>) -> Result<Int, Int> {
          let r: Result<Int, Str> = o.ok_or(\"none\");
          match r { Ok(n) => Ok(n), Err(_) => Err(0) }
        }
        fn main() -> Int { f(None).unwrap_or(7) }";
    assert_eq!(int(src), Value::Int(7));
}

#[test]
fn passing_a_function_to_another() {
    let src = "fn double(x: Int) -> Int { x * 2 }
               fn apply(f: fn(Int) -> Int, xs: List<Int>) -> List<Int> {
                   let mut out: List<Int> = [];
                   for x in xs { out.push(f(x)); }
                   out
               }
               fn main() -> List<Int> { apply(double, [1, 2, 3]) }";
    let want = Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)].into());
    assert_eq!(int(src), want);
}

#[test]
fn route_table_of_handlers() {
    let src = "struct Request { path: Str }
               struct Response { status: Int, body: Str }
               struct Route { path: Str, handler: fn(Request) -> Response }
               fn home(r: Request) -> Response { Response { status: 200, body: \"home\" } }
               fn about(r: Request) -> Response { Response { status: 200, body: \"about \" + r.path } }
               fn dispatch(routes: List<Route>, req: Request) -> Response {
                   for r in routes { if r.path == req.path { return (r.handler)(req); } }
                   Response { status: 404, body: \"\" }
               }
               fn main() -> Str {
                   let routes = [Route { path: \"/\", handler: home }, Route { path: \"/about\", handler: about }];
                   let a = dispatch(routes, Request { path: \"/about\" });
                   let b = dispatch(routes, Request { path: \"/x\" });
                   a.body + \" \" + b.status.to_str()
               }";
    assert_eq!(int(src), Value::Str("about /about 404".into()));
}

#[test]
fn function_returning_a_function() {
    let src = "fn inc(x: Int) -> Int { x + 1 }
               fn dec(x: Int) -> Int { x - 1 }
               fn pick(up: Bool) -> fn(Int) -> Int { if up { inc } else { dec } }
               fn main() -> Int { let f = pick(false); pick(true)(10) * 100 + f(10) }";
    assert_eq!(int(src), Value::Int(1109));
}

#[test]
fn recursive_function_as_a_value() {
    let src = "fn fact(n: Int) -> Int { if n < 2 { 1 } else { n * fact(n - 1) } }
               fn twice(f: fn(Int) -> Int, x: Int) -> Int { f(f(x)) }
               fn main() -> Int { twice(fact, 3) }";
    assert_eq!(int(src), Value::Int(720));
    // Calls through values count toward the depth limit.
    fails(
        "fn spin(n: Int) -> Int { let f = spin; f(n) } fn main() -> Int { spin(1) }",
        "evaluation is too deep",
    );
}

#[test]
fn local_hides_function_of_the_same_name() {
    let src = "fn f(x: Int) -> Int { x + 1 }
               fn g(x: Int) -> Int { x * 10 }
               fn main() -> Int { let a = f(1); let f = g; a + f(1) }";
    assert_eq!(int(src), Value::Int(12));
}

#[test]
fn function_values_display() {
    let src = "fn double(x: Int) -> Int { x * 2 } fn main() -> List<fn(Int) -> Int> { [double] }";
    let p = check(src, &CapTable::default()).unwrap();
    let v = run(&p, &mut NoCaps).unwrap();
    assert_eq!(p.display(&v), "[<fn double>]");
/// `main`'s value as `kio run` prints it.
fn shown(src: &str) -> String {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let v = run(&p, &mut NoCaps).unwrap();
    p.display(&v)
}

/// `e`, a `Float` expression, printed by `to_str`.
fn float(e: &str) -> String {
    shown(&format!("fn main() -> Str {{ ({e}).to_str() }}"))
}

#[test]
fn float_literals_and_arithmetic() {
    assert_eq!(int("fn main() -> Float { 1.5 }"), Value::Float(1.5));
    assert_eq!(
        int("fn main() -> Float { 1.5e-3 + 2E+4 + 1e9 }"),
        Value::Float(1.5e-3 + 2e4 + 1e9)
    );
    assert_eq!(float("0.1 + 0.2"), "0.30000000000000004");
    assert_eq!(float("7.5 - 10.0"), "-2.5");
    assert_eq!(float("1.5 * 4.0"), "6");
    assert_eq!(float("1.0 / 3.0"), format!("{}", 1.0f64 / 3.0));
    assert_eq!(float("7.5 % 2.0"), "1.5");
    assert_eq!(float("-7.5 % 2.0"), "-1.5");
    assert_eq!(float("-(2.5)"), "-2.5");
    let src = "fn main() -> Float { let mut x = 1.0; x += 0.5; x -= 2.0; x }";
    assert_eq!(int(src), Value::Float(-0.5));
    let src = "struct P { x: Float } fn main() -> Float { let mut ps = [P { x: 1.0 }]; ps[0].x += 2.0; ps[0].x }";
    assert_eq!(int(src), Value::Float(3.0));
}

#[test]
fn float_ieee_results() {
    // No runtime errors: division by zero, overflow and NaN give IEEE values.
    assert_eq!(float("1.0 / 0.0"), "inf");
    assert_eq!(float("-1.0 / 0.0"), "-inf");
    assert_eq!(float("0.0 / 0.0"), "NaN");
    assert_eq!(float("1.0 % 0.0"), "NaN");
    assert_eq!(float("1e308 * 10.0"), "inf");
    assert_eq!(float("-0.0"), "-0");
    assert_eq!(float("0.0 * -1.0"), "-0");
    assert_eq!(float("1.0 / -0.0"), "-inf");
    assert_eq!(float("(1.0 / 0.0) - (1.0 / 0.0)"), "NaN");
}

#[test]
fn float_comparison_is_ieee() {
    let nan = "let nan = 0.0 / 0.0;";
    for (e, want) in [
        ("nan == nan", false),
        ("nan != nan", true),
        ("nan < 1.0 || nan >= 1.0", false),
        ("0.0 == -0.0", true),
        ("-0.0 < 0.0", false),
        ("1.0 < 2.0 && 2.0 <= 2.0 && 3.0 > 2.0 && 2.0 >= 2.0", true),
        ("1.0 / 0.0 > 1e308", true),
        ("[nan] == [nan]", false),
        ("[nan] != [nan]", true),
        ("[0.0] == [-0.0]", true),
        ("Some(nan) == Some(nan)", false),
        ("[nan].contains(nan)", false),
    ] {
        let src = format!("fn main() -> Bool {{ {nan} {e} }}");
        assert_eq!(int(&src), Value::Bool(want), "{e}");
    }
    let src = "struct P { x: Float, n: Int } enum E { A(Float) }
               fn main() -> Bool {
                   let nan = 0.0 / 0.0;
                   let p = P { x: nan, n: 1 };
                   let q = P { x: 1.0, n: 1 };
                   p != p && !(p == p) && q == q && E::A(nan) != E::A(nan) && E::A(-0.0) == E::A(0.0)
               }";
    assert_eq!(int(src), Value::Bool(true));
    // As Rust's derived `PartialEq`.
    assert_ne!(vec![f64::NAN], vec![f64::NAN]);
    let nans = || Value::List(vec![Value::Float(f64::NAN)].into());
    assert_ne!(nans(), nans());
}

#[test]
fn float_to_str_matches_rust() {
    for (e, x) in [
        ("1.0", 1.0f64),
        ("0.5", 0.5),
        ("100.0", 100.0),
        ("1e21", 1e21),
        ("1e-7", 1e-7),
        ("123456789.125", 123456789.125),
        ("2.5e-308", 2.5e-308),
        ("1.7976931348623157e308", f64::MAX),
    ] {
        assert_eq!(float(e), format!("{x}"), "{e}");
    }
}

#[test]
fn float_to_int() {
    let to_int = |e: &str| int(&format!("fn main() -> Option<Int> {{ ({e}).to_int() }}"));
    assert_eq!(to_int("2.9"), some(Value::Int(2)));
    assert_eq!(to_int("-2.9"), some(Value::Int(-2)));
    assert_eq!(to_int("-0.0"), some(Value::Int(0)));
    assert_eq!(to_int("-9223372036854775808.0"), some(Value::Int(i64::MIN)));
    // 2^63 is the first float above `i64::MAX`.
    assert_eq!(to_int("9223372036854775808.0"), none());
    assert_eq!(to_int("9223372036854774784.0"), some(Value::Int(9223372036854774784)));
    assert_eq!(to_int("-9223372036854777856.0"), none());
    assert_eq!(to_int("0.0 / 0.0"), none());
    assert_eq!(to_int("1.0 / 0.0"), none());
    assert_eq!(to_int("-1.0 / 0.0"), none());
}

#[test]
fn float_methods() {
    assert_eq!(float("(-2.5).abs()"), "2.5");
    assert_eq!(float("(-0.0).abs()"), "0");
    assert_eq!(float("2.7.floor()"), "2");
    assert_eq!(float("(-2.2).floor()"), "-3");
    assert_eq!(float("2.2.ceil()"), "3");
    assert_eq!(float("(-0.5).ceil()"), "-0");
    // Half away from zero, as Rust's `round`.
    assert_eq!(float("2.5.round()"), "3");
    assert_eq!(float("(-2.5).round()"), "-3");
    assert_eq!(float("0.49999999999999994.round()"), "0");
    assert_eq!(float("9.0.sqrt()"), "3");
    assert_eq!(float("(-1.0).sqrt()"), "NaN");
    assert_eq!(float("(1.0 / 0.0).floor()"), "inf");
    assert_eq!(
        int("fn main() -> Bool { (0.0 / 0.0).is_nan() && !1.0.is_nan() }"),
        Value::Bool(true)
    );
    assert_eq!(int("fn main() -> Float { 3.to_float() }"), Value::Float(3.0));
    assert_eq!(float("(-7).to_float()"), "-7");
    // Rounds to nearest, as Rust's `as f64`.
    assert_eq!(float("9223372036854775807.to_float()"), format!("{}", i64::MAX as f64));
}

#[test]
fn str_parse_float() {
    let parse = |s: &str| shown(&format!("fn main() -> Option<Float> {{ \"{s}\".parse_float() }}"));
    for (s, want) in [
        ("1.5", "Some(1.5)"),
        ("-2", "Some(-2)"),
        ("+3", "Some(3)"),
        ("1e3", "Some(1000)"),
        (".5", "Some(0.5)"),
        ("5.", "Some(5)"),
        ("inf", "Some(inf)"),
        ("-infinity", "Some(-inf)"),
        ("NaN", "Some(NaN)"),
        ("1e400", "Some(inf)"),
        ("-0", "Some(-0)"),
        ("", "None"),
        (" 1", "None"),
        ("1,5", "None"),
        ("0x1", "None"),
        ("e3", "None"),
    ] {
        assert_eq!(parse(s), want, "{s:?}");
    }
}

#[test]
fn floats_display_nested() {
    assert_eq!(shown("fn main() -> Float { 1.0 }"), "1");
    let src = "struct P { x: Float }
               fn main() -> List<P> { [P { x: 0.1 + 0.2 }, P { x: -0.0 }] }";
    assert_eq!(shown(src), "[P { x: 0.30000000000000004 }, P { x: -0 }]");
    let src = "fn main() -> Map<Str, List<Float>> {
                   let mut m: Map<Str, List<Float>> = Map::new();
                   m.insert(\"a\", [1.0 / 0.0, 0.0 / 0.0, 2.5]);
                   m
               }";
    assert_eq!(shown(src), "{\"a\": [inf, NaN, 2.5]}");
    assert_eq!(shown("fn main() -> Option<Float> { Some(-1.0 / 0.0) }"), "Some(-inf)");
}
