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

/// Runs on the stack `run` documents for debug builds.
fn exec(src: &str) -> Result<Value, RunErr> {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let t = std::thread::Builder::new().stack_size(8 << 20);
    t.spawn(move || run(&p, &mut NoCaps)).unwrap().join().unwrap()
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
    assert_eq!(int("fn main() -> Bytes { \"a\".to_bytes() }"), Value::Bytes(vec![97]));
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
    let ints = |ns: &[i64]| Value::List(ns.iter().map(|n| Value::Int(*n)).collect());
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
        Value::List(vec![Value::Int(1), Value::Int(2)])
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
