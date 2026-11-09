use kio_host::{Compiled, Kio, Limits};

/// A run's value, or its first error as (message, start, end).
type Run = Result<String, (String, usize, usize)>;

/// Enough fuel for the interpreter's 3 M loop.
fn limits() -> Limits {
    Limits {
        fuel: 20_000_000_000,
        ..Limits::default()
    }
}

fn compiled(src: &str) -> Run {
    match Compiled::run(src, limits()).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].msg.clone(), d[0].span.start, d[0].span.end)),
    }
}

fn interpreted(src: &str) -> Run {
    match Kio::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

/// Runs `src` both ways, asserts they agree, and returns the result.
fn both(src: &str) -> Run {
    let c = compiled(src);
    assert_eq!(c, interpreted(src), "compiled vs interpreted: {src}");
    c
}

fn value(src: &str) -> String {
    both(src).unwrap_or_else(|e| panic!("{src}: {e:?}"))
}

fn error(src: &str) -> String {
    both(src).unwrap_err().0
}

#[test]
fn forty_plus_two_compiled() {
    let out = Compiled::run("fn main() -> Int { 40 + 2 }", Limits::default())
        .unwrap()
        .unwrap();
    assert_eq!((out.value.as_str(), out.failed), ("42", false));
}

#[test]
fn main_returns_int_bool_or_unit() {
    assert_eq!(value("fn main() -> Int { 40 + 2 }"), "42");
    assert_eq!(value("fn main() -> Bool { 1 < 2 }"), "true");
    assert_eq!(value("fn main() -> Bool { !true }"), "false");
    assert_eq!(value("fn main() { let x = 1; }"), "()");
    assert_eq!(value("fn main() -> () { () }"), "()");
}

#[test]
fn arithmetic_and_comparisons() {
    assert_eq!(value("fn main() -> Int { 7 - 10 * 2 / 3 % 4 }"), "5");
    assert_eq!(value("fn main() -> Int { -7 / 2 + -7 % 2 * 10 }"), "-13");
    assert_eq!(value("fn main() -> Int { -(3 - 5) }"), "2");
    assert_eq!(
        value("fn main() -> Int { -9223372036854775807 - 1 }"),
        "-9223372036854775808"
    );
    assert_eq!(
        value("fn main() -> Int { 3037000499 * 3037000499 }"),
        "9223372030926249001"
    );
    assert_eq!(
        value("fn main() -> Int { -1 * -9223372036854775807 }"),
        "9223372036854775807"
    );
    for (op, want) in [
        ("<", "true"),
        ("<=", "true"),
        (">", "false"),
        (">=", "false"),
        ("==", "false"),
        ("!=", "true"),
    ] {
        assert_eq!(value(&format!("fn main() -> Bool {{ -1 {op} 2 }}")), want);
    }
    assert_eq!(value("fn main() -> Bool { true == (1 < 2) }"), "true");
    assert_eq!(value("fn main() -> Bool { true != false }"), "true");
    assert_eq!(value("fn main() -> Bool { () == () }"), "true");
    assert_eq!(value("fn main() -> Bool { false || true && !false }"), "true");
}

#[test]
fn every_integer_error_matches() {
    let min = "(-9223372036854775807 - 1)";
    let max = "9223372036854775807";
    for e in [
        format!("{max} + 1"),
        format!("{min} + -1"),
        format!("{min} - 1"),
        format!("{max} - -1"),
        "0 - 9223372036854775807 - 2".to_string(),
        format!("{max} * 2"),
        format!("{min} * -1"),
        format!("-1 * {min}"),
        "3037000500 * 3037000500".to_string(),
        "-3037000500 * 3037000500".to_string(),
        format!("{min} / -1"),
        format!("{min} % -1"),
        format!("-{min}"),
    ] {
        assert_eq!(error(&format!("fn main() -> Int {{ {e} }}")), "Int overflow", "{e}");
    }
    for e in [
        "1 / 0",
        "1 % 0",
        "(-9223372036854775807 - 1) / 0",
        "(-9223372036854775807 - 1) % 0",
    ] {
        assert_eq!(error(&format!("fn main() -> Int {{ {e} }}")), "division by zero", "{e}");
    }
    // `+=` and `-=` report the statement's span.
    let add = "fn main() -> Int { let mut x = 9223372036854775807; x += 1; x }";
    assert_eq!(both(add).unwrap_err(), ("Int overflow".to_string(), 52, 59));
    assert_eq!(
        error("fn main() -> Int { let mut x = -9223372036854775807; x -= 2; x }"),
        "Int overflow"
    );
    // The error is at the inner operation, inside a call.
    assert_eq!(
        error("fn f(a: Int, b: Int) -> Int { a / b } fn main() -> Int { 1 + f(1, 0) }"),
        "division by zero"
    );
}

#[test]
fn short_circuit_skips_the_right_side() {
    let big = "fn big() -> Bool { 9223372036854775807 + 1 > 0 }";
    assert_eq!(value(&format!("{big} fn main() -> Bool {{ false && big() }}")), "false");
    assert_eq!(value(&format!("{big} fn main() -> Bool {{ true || big() }}")), "true");
    assert_eq!(
        error(&format!("{big} fn main() -> Bool {{ true && big() }}")),
        "Int overflow"
    );
    assert_eq!(
        error(&format!("{big} fn main() -> Bool {{ false || big() }}")),
        "Int overflow"
    );
}

#[test]
fn if_else_if_and_blocks() {
    let sign = "fn sign(n: Int) -> Int { if n < 0 { -1 } else if n == 0 { 0 } else { 1 } }";
    assert_eq!(
        value(&format!(
            "{sign} fn main() -> Int {{ sign(-5) * 100 + sign(0) * 10 + sign(7) }}"
        )),
        "-99"
    );
    assert_eq!(value("fn main() -> Int { let x = { let y = 2; y * 3 }; x }"), "6");
    assert_eq!(value("fn main() -> Int { let mut x = 1; if x > 0 { x = 5; } x }"), "5");
}

#[test]
fn fib_and_the_loop() {
    let fib = "fn fib(n: Int) -> Int { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }";
    assert_eq!(value(&format!("{fib} fn main() -> Int {{ fib(20) }}")), "6765");
    let lp = "fn main() -> Int { let mut s = 0; let mut i = 0; while i < 3000000 { s = s + i % 7; i += 1; } s }";
    assert_eq!(value(lp), "8999994");
}

#[test]
fn loops_break_continue_and_return() {
    // `continue` in `while` re-tests the condition; `break` leaves only the inner loop.
    let src = "fn main() -> Int {
        let mut s = 0; let mut i = 0;
        while i < 10 { i += 1; if i % 2 == 0 { continue; } s += i; }
        loop { s += 100; if s > 300 { break; } }
        for j in 0..10 { if j == 3 { continue; } if j == 6 { break; } s += j * 1000; }
        s
    }";
    assert_eq!(value(src), "12325");
    // `return` from inside nested loops, and in expression position.
    let find = "fn find(n: Int) -> Int {
        for a in 1..n { let mut b = 1; loop { if a * b == n { return a * 1000 + b; } if b > n { break; } b += 1; } }
        -1
    }";
    assert_eq!(
        value(&format!("{find} fn main() -> Int {{ find(35) * 10 + find(1) }}")),
        "10349"
    );
    let early = "fn f(x: Int) -> Int { let y = if x > 0 { x } else { return 0 - x; }; y * 10 }";
    assert_eq!(value(&format!("{early} fn main() -> Int {{ f(3) + f(-4) }}")), "34");
    assert_eq!(value("fn f() { return; } fn main() -> Int { f(); 1 }"), "1");
    assert_eq!(
        value("fn f(n: Int) -> Int { loop { return n; } } fn main() -> Int { f(9) }"),
        "9"
    );
    assert_eq!(
        value("fn f() -> Int { while true { return 1; } 2 } fn main() -> Int { f() }"),
        "1"
    );
}

#[test]
fn for_ranges() {
    // Bounds are evaluated once; an empty or backward range runs no pass.
    let src = "fn main() -> Int { let mut n = 3; let mut s = 0; for i in 0..n { n += 1; s += i; } s * 100 + n }";
    assert_eq!(value(src), "306");
    assert_eq!(
        value("fn main() -> Int { let mut s = 0; for i in 5..5 { s += 1; } for i in 5..-5 { s += 1; } s }"),
        "0"
    );
    assert_eq!(
        value("fn main() -> Int { let mut s = 0; for i in -3..3 { s += i; } s }"),
        "-3"
    );
    // The last pass's `i` is end - 1, so `i + 1` cannot overflow, even at the top.
    let top = "fn main() -> Int { let mut s = 0; for i in 9223372036854775805..9223372036854775807 { s += 1; } s }";
    assert_eq!(value(top), "2");
    assert_eq!(
        error("fn main() -> Int { let mut s = 0; for i in 0..1 / 0 { s += 1; } s }"),
        "division by zero"
    );
}

#[test]
fn shadowing_and_unit_locals() {
    let src = "fn main() -> Int { let x = 1; let x = x + 1; let b = x == 2; let x = if b { x * 10 } else { 0 }; x }";
    assert_eq!(value(src), "20");
    // A `()` parameter and local have no Wasm local; the parameters after them still line up.
    let unit = "fn f(u: (), a: Int, v: (), b: Bool) -> Int { let w = u; if b { a } else { 0 } }
        fn main() -> Int { let u = (); f(u, 7, (), true) }";
    assert_eq!(value(unit), "7");
}

#[test]
fn recursion_and_mutual_calls() {
    let src = "fn even(n: Int) -> Bool { if n == 0 { true } else { odd(n - 1) } }
        fn odd(n: Int) -> Bool { if n == 0 { false } else { even(n - 1) } }
        fn main() -> Bool { even(500) && !odd(10) }";
    assert_eq!(value(src), "true");
}

/// Decision 6: compiled code has no call counter; recursion stops at `--stack`.
#[test]
fn deep_recursion_is_a_stack_overflow() {
    // Wasm runs on this thread's stack, so it needs room above `--stack`, as `kio` gives it.
    let t = std::thread::Builder::new().stack_size(4 * kio_host::MAX_STACK);
    t.spawn(deep_recursion).unwrap().join().unwrap();
}

fn deep_recursion() {
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { 1 + down(n - 1) } } fn main() -> Int { down(10000000) }";
    let e = Compiled::run(src, Limits::default()).unwrap_err();
    assert_eq!(e.to_string(), "stack overflow (limit 2048 KB; raise it with --stack)");
    // Within the interpreter's limit both agree.
    assert_eq!(value(&src.replace("10000000", "900")), "900");
}

#[test]
fn overflow_matches_the_interpreter() {
    let src = "fn main() -> Int { 9223372036854775807 + 1 }";
    assert_eq!(both(src).unwrap_err(), ("Int overflow".to_string(), 19, 42));
}

// Str and Bytes (C3b). Each ownership rule (docs/codegen.md, "Ownership") has a test whose
// program leaks ("compiled program leaked N objects") or reads freed memory without it.

#[test]
fn strings_and_bytes_values_and_builtins() {
    assert_eq!(value(r#"fn main() -> Str { "hi" }"#), "hi");
    assert_eq!(value(r#"fn main() -> Bytes { "hi!".to_bytes() }"#), "<3 bytes>");
    assert_eq!(value(r#"fn main() -> Str { "a" + 12.to_str() + "b" }"#), "a12b");
    let src = r#"fn main() -> Int {
        let s = "  héllo  ".trim();
        let mut n = s.len() * 1000 + s.to_bytes().len() * 100;
        if s.contains("ll") { n += 10; }
        if s.starts_with("hé") && !s.starts_with("e") { n += 1; }
        n
    }"#;
    assert_eq!(value(src), "6611");
    let src = r#"fn main() -> Bool { "ab" == "a" + "b" && "ab" != "ba" && "x".to_bytes() == "x".to_bytes() }"#;
    assert_eq!(value(src), "true");
    let src = r#"fn main() -> Str { let mut b = "".to_bytes(); for i in 0..40 { b.push(65 + i % 26); } b.to_str() }"#;
    assert_eq!(value(src), "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMN");
    assert_eq!(
        value(r#"fn main() -> Int { let b = "AZ".to_bytes(); b[0] * 1000 + b[1] }"#),
        "65090"
    );
}

#[test]
fn string_and_bytes_errors_match() {
    let src = r#"fn main() -> Int { let b = "ab".to_bytes(); b[2] }"#;
    assert_eq!(both(src).unwrap_err().0, "index 2 out of bounds for length 2");
    assert_eq!(
        error(r#"fn main() -> Int { "ab".to_bytes()[0 - 1] }"#),
        "index -1 out of bounds for length 2"
    );
    assert_eq!(
        error(r#"fn main() { let mut b = "".to_bytes(); b.push(256); }"#),
        "byte 256 out of range 0..=255"
    );
    assert_eq!(
        error(r#"fn main() -> Str { let mut b = "".to_bytes(); b.push(255); b.to_str() }"#),
        "invalid UTF-8"
    );
}

/// Reading a local is a copy (`rt_inc`): without it the first use frees the string.
#[test]
fn reading_a_local_twice() {
    let src = r#"fn main() -> Str { let s = 1.to_str() + "x"; let t = s + s; s + t + s }"#;
    assert_eq!(value(src), "1x1x1x1x");
}

/// An argument is the callee's, which drops its parameters when it returns.
#[test]
fn an_argument_is_the_callees() {
    let src = r#"fn f(s: Str, t: Str) -> Int { s.len() + t.len() }
        fn main() -> Int { let mut n = 0; let k = "ab"; for i in 0..10 { n += f(i.to_str() + "ab", k); } n + f(k, k) }"#;
    assert_eq!(value(src), "54");
}

/// Assignment drops the old value after evaluating the new one, which may read it.
#[test]
fn reassignment_in_a_loop() {
    let src = r#"fn main() -> Str { let mut s = ""; let mut t = "x"; for i in 0..30 { s = s + i.to_str(); t += s; t = "y"; } s + t }"#;
    assert_eq!(value(src), "01234567891011121314151617181920212223242526272829y");
}

/// A `let` in a loop body is dropped at the body's end, each pass.
#[test]
fn a_local_going_out_of_scope_in_a_loop() {
    let src = r#"fn main() -> Int { let mut n = 0; for i in 0..20 { let t = i.to_str(); let u = t + "-"; n += u.len(); } n }"#;
    assert_eq!(value(src), "50");
}

/// An expression statement's value, and the operands of `==` and of a builtin, are dropped.
#[test]
fn unused_values_are_dropped() {
    let src = r#"fn main() -> Int { let mut n = 0; for i in 0..20 { i.to_str(); if i.to_str() == "7" { n += 1; } n += i.to_str().to_bytes().len(); } n }"#;
    assert_eq!(value(src), "31");
}

/// `return` drops the locals of every scope it leaves, and the parameters.
#[test]
fn return_from_nested_blocks() {
    let src = r#"fn f(p: Str, n: Int) -> Int {
            let a = n.to_str();
            if n > 0 {
                let b = a + "x";
                while true { let c = b + p; if n > 1 { return c.len(); } return 0 - 1; }
            }
            a.len()
        }
        fn main() -> Int { f("p", 5) * 100 + f("p", 1) * 10 + f("p", 0) }"#;
    assert_eq!(value(src), "291");
}

/// `break` and `continue` drop the loop body's locals that are alive at that point.
#[test]
fn break_and_continue_out_of_a_loop() {
    let src = r#"fn main() -> Int {
        let mut n = 0; let mut i = 0; let keep = "k";
        loop {
            let s = i.to_str() + keep;
            i += 1;
            if i % 2 == 0 { continue; }
            let t = s + "t";
            if i > 9 { break; }
            n += t.len();
            while true { let u = t + "u"; if n > 0 { break; } }
        }
        n
    }"#;
    assert_eq!(value(src), "15");
}

/// An owned temporary (the left of `+`, a call's earlier argument, a builtin's receiver, an
/// indexed `Bytes`) is held in a hidden local that a `return` in a later operand drops.
#[test]
fn temporaries_survive_an_early_return() {
    let src = r#"fn g(a: Str, b: Int) -> Int { a.len() + b }
        fn cat(n: Int) -> Str { n.to_str() + if n > 5 { return "big"; } else { "!" } }
        fn call(n: Int) -> Int { g(n.to_str(), if n > 5 { return 0 - 1; } else { 1 }) }
        fn recv(n: Int) -> Bool { n.to_str().contains(if n > 5 { return false; } else { "1" }) }
        fn index(n: Int) -> Int { "ab".to_bytes()[if n > 5 { return 0 - 1; } else { 1 }] }
        fn main() -> Str {
            let mut s = "";
            for n in 0..10 {
                s = s + cat(n) + call(n).to_str();
                if recv(n) { s += "T"; }
                s += index(n).to_str();
                s += ",";
            }
            s
        }"#;
    assert_eq!(
        value(src),
        "0!298,1!2T98,2!298,3!298,4!298,5!298,big-1-1,big-1-1,big-1-1,big-1-1,"
    );
}

/// `push` copies a shared `Bytes` first (`c` has spare capacity, so the push would not copy
/// otherwise), and stores back the object it returns.
#[test]
fn bytes_push_copies_when_shared() {
    let src = r#"fn main() -> Int { let mut b = "ab".to_bytes(); b.push(1); let c = b; b.push(99); for i in 0..100 { b.push(i); } b.len() * 10 + c.len() }"#;
    assert_eq!(value(src), "1043");
}
