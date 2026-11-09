use kio_host::{Interp, Kio, Limits};

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
    match Kio::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(limits()).unwrap().run(src, vec![]).unwrap() {
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
    let out = Kio::new(Limits::default())
        .unwrap()
        .run("fn main() -> Int { 40 + 2 }", vec![])
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
    let e = Kio::new(Limits::default()).unwrap().run(src, vec![]).unwrap_err();
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

/// `s = s + x` and `s += x` grow `s` in place only when it holds the only reference (C8b1): a
/// string shared by another local keeps its value.
#[test]
fn append_copies_a_shared_string() {
    let src = r#"fn main() -> Str { let mut s = ""; for i in 0..10 { s = s + i.to_str(); }
        let t = s; s = s + "x"; let u = s; s += "y"; t + "|" + u + "|" + s }"#;
    assert_eq!(value(src), "0123456789|0123456789x|0123456789xy");
}

/// `s = s + s`: each read of `s` holds its own reference, so the string is copied, never grown
/// under its own bytes. A part that reassigns `s` leaves the old value to the first read alone.
#[test]
fn append_of_itself() {
    let src = r#"fn main() -> Str { let mut s = 1.to_str(); for i in 0..5 { s = s + s; s += s; s = s + "-" + s; }
        let mut t = 0.to_str(); t = t + "c" + t + t;
        let mut u = 7.to_str(); u = u + "x"; u = u + if true { u = "z"; "q" } else { "w" } + u;
        s.len().to_str() + "|" + s.slice(0, 9) + "|" + t + "|" + u }"#;
    assert_eq!(value(src), "37449|1111-1111|0c00|7xqz");
}

/// A literal is pinned: appending to a local holding one copies it, and the literal is unchanged.
#[test]
fn append_to_a_literal_copies_it() {
    let src = r#"fn f() -> Str { let mut s = "lit"; s = s + "x"; s += "y"; s }
        fn main() -> Str { let a = f(); let b = f(); a + b + "lit" }"#;
    assert_eq!(value(src), "litxylitxylit");
}

/// `xs[i] += x` and `p.f += x` append to the element or field, unique after the place's walk;
/// a list sharing the element keeps its value.
#[test]
fn append_to_an_element_or_field() {
    let src = r#"struct P { s: Str }
        fn main() -> Str { let mut xs = [1.to_str()]; for i in 0..5 { xs[0] += i.to_str(); }
        let ys = xs; xs[0] += "!"; let z = xs[0]; xs[0] += "?";
        let mut p = P { s: "a" }; for i in 0..3 { p.s += i.to_str(); } let q = p; p.s += "!";
        ys[0] + "|" + z + "|" + xs[0] + "|" + q.s + "|" + p.s }"#;
    assert_eq!(value(src), "101234|101234!|101234!?|a012|a012!");
}

// List and Map (C3c). As above, each ownership rule has a test that leaks, frees too early or
// gives a wrong value without it.

#[test]
fn lists_and_maps_values_and_builtins() {
    let src = "fn main() -> Int { let xs = [3, 4, 5]; xs[0] * 100 + xs[2] * 10 + xs.len() }";
    assert_eq!(value(src), "353");
    let src = "fn main() -> Bool { let e: List<Int> = []; [1, 2] == [1, 2] && [1] != [2] && e.len() == 0 && [[true]] == [[true]] }";
    assert_eq!(value(src), "true");
    let src = r#"fn main() -> Str {
        let mut m: Map<Str, Int> = Map::new(); m.insert("b", 2); m.insert("a", 1); m.insert("c", 3); m.remove("c"); m.remove("zz");
        let ks = m.keys();
        let mut n: Map<Str, Int> = Map::new(); n.insert("a", 1); n.insert("b", 2);
        let mut e: Map<Int, ()> = Map::new(); e.insert(1, ()); e.remove(1);
        ks[0] + ks[1] + m.len().to_str() + if m == n { "T" } else { "F" } + e.len().to_str()
    }"#;
    assert_eq!(value(src), "ab2T0");
    let src = r#"fn main() -> Str {
        let parts = "a,b,,c".split(",");
        let lines = "x\ny\n".lines();
        let mut s = parts.len().to_str() + lines.len().to_str();
        for p in parts { s += "[" + p + "]"; }
        s + lines[1]
    }"#;
    assert_eq!(value(src), "42[a][b][][c]y");
    let src = r#"fn main() -> Int { let mut xs = [1, 2]; xs[1] += 40; xs[0] -= 10; let mut ss = ["a"]; ss[0] += "bc"; xs[0] + xs[1] + ss[0].len() }"#;
    assert_eq!(value(src), "36");
    let src = r#"fn main() -> Int { let mut b = ["".to_bytes()]; b[0].push(7); b[0].push(9); let c = b[0]; c[0] + c[1] + c.len() }"#;
    assert_eq!(value(src), "18");
}

#[test]
fn list_errors_match() {
    let index = "fn main() -> Int { let xs = [1, 2]; xs[2] }";
    assert_eq!(
        both(index).unwrap_err(),
        ("index 2 out of bounds for length 2".to_string(), 36, 41)
    );
    assert_eq!(
        error("fn main() -> Int { [1][0 - 1] }"),
        "index -1 out of bounds for length 1"
    );
    // A place write evaluates the indexes, then the value, then checks bounds outward.
    assert_eq!(
        error("fn main() { let mut xs = [1]; xs[5] = 1 / 0; }"),
        "division by zero"
    );
    let outer = "fn main() { let mut xs = [[1]]; xs[3][1 / 1] = 2; }";
    assert_eq!(
        both(outer).unwrap_err(),
        ("index 3 out of bounds for length 1".to_string(), 32, 37)
    );
    let inner = "fn main() { let mut xs = [[1]]; xs[0][4] = 2; }";
    assert_eq!(
        both(inner).unwrap_err(),
        ("index 4 out of bounds for length 1".to_string(), 32, 40)
    );
    // Bounds come before the overflow of `+=`, which reports the statement.
    assert_eq!(
        error("fn main() { let mut xs = [9223372036854775807]; xs[1] += 1; }"),
        "index 1 out of bounds for length 1"
    );
    let add = "fn main() { let mut xs = [9223372036854775807]; xs[0] += 1; }";
    assert_eq!(both(add).unwrap_err(), ("Int overflow".to_string(), 48, 59));
    assert_eq!(
        error("fn main() { let mut xs = [[1]]; xs[2].push(1 / 0); }"),
        "division by zero"
    );
    assert_eq!(
        error(r#"fn main() { let mut b = ["".to_bytes()]; b[0].push(300); }"#),
        "byte 300 out of range 0..=255"
    );
    assert_eq!(
        error(r#"fn main() -> Int { "a".split("").len() }"#),
        "`split` needs a non-empty separator"
    );
}

/// A list literal takes its elements: dropping them after the push frees them inside the list.
#[test]
fn list_literal_takes_its_elements() {
    let src = r#"fn main() -> Str { let s = "s" + "t"; let xs = [s, 1.to_str() + "x", s]; xs[0] + xs[1] + xs[2] }"#;
    assert_eq!(value(src), "st1xst");
}

/// `xs[i]` is a new reference (the runtime's inc) and its list is dropped after the read.
#[test]
fn an_element_read_is_owned() {
    let src = r#"fn f() -> List<Str> { ["a" + "b", "c" + ""] }
        fn main() -> Str { let xs = f(); let s = xs[0]; let t = f()[1]; s + xs[0] + t }"#;
    assert_eq!(value(src), "ababc");
}

/// A place write copies a shared list first (`rt_unique`) and stores the copy back.
#[test]
fn element_write_copies_a_shared_list() {
    let src = "fn main() -> Int { let mut a = [1, 2]; let mut b = a; b[0] = 9; a[1] += 5; a[0] * 1000 + a[1] * 100 + b[0] * 10 + b[1] }";
    assert_eq!(value(src), "1792");
    let src = r#"fn main() -> Str { let mut a = ["x" + "y"]; let b = a; a[0] += "z"; a[0] = a[0] + "!"; a[0] + b[0] }"#;
    assert_eq!(value(src), "xyz!xy");
}

/// `xs[i][j] = v` makes each list on the path unique, and stores each back into its parent.
#[test]
fn nested_place_write() {
    let src = "fn main() -> Int {
        let mut xs = [[1, 2], [3, 4]];
        let ys = xs;
        let inner = xs[1];
        xs[1][0] = 9;
        xs[1][1] += 10;
        xs[0][0] = xs[1][0];
        xs[0][0] * 100000 + xs[1][0] * 10000 + xs[1][1] * 100 + ys[1][0] * 10 + inner[0]
    }";
    assert_eq!(value(src), "991433");
}

/// `push` copies a shared list, at the root or inside another list.
#[test]
fn push_through_a_shared_list() {
    let src = "fn main() -> Int {
        let mut a = [1]; let b = a; a.push(2);
        let mut xs = [[1], [2]]; let inner = xs[0]; xs[0].push(5); xs[0].push(6);
        let ys = xs; xs[1].push(7);
        a.len() * 10000 + b.len() * 1000 + xs[0].len() * 100 + inner.len() * 10 + ys[1].len()
    }";
    assert_eq!(value(src), "21311");
    // The pushed value becomes the list's.
    let src = r#"fn main() -> Str { let mut ss = [1.to_str()]; let tt = ss; ss.push(ss[0] + "x"); ss.push(tt[0]); ss[0] + ss[1] + ss[2] + tt[0] }"#;
    assert_eq!(value(src), "11x11");
}

/// `for x in xs` holds its own reference to the list, dropped on every way out of the loop,
/// and drops each element after its pass.
#[test]
fn for_over_a_list_releases_on_every_exit() {
    let src = r#"fn find(xs: List<Str>, w: Str) -> Int { let mut i = 0; for x in xs { if x == w { return i; } i += 1; } 0 - 1 }
        fn upto(xs: List<Str>, w: Str) -> Str { let mut s = ""; for x in xs { if x == w { break; } if x == "-" { continue; } s += x; } s }
        fn main() -> Str {
            let xs = ["a" + "", "-" + "", "b" + "", "c" + ""];
            let mut s = find(xs, "b").to_str() + find(xs, "z").to_str() + upto(xs, "c") + upto(xs, "q");
            for y in ["p" + "", "q" + ""] { s += y; }
            s
        }"#;
    assert_eq!(value(src), "2-1ababcpq");
}

/// A change to the list inside `for x in xs` copies it: the loop sees the list as it began.
#[test]
fn changing_the_list_inside_its_for() {
    let src = "fn main() -> Int { let mut xs = [1, 2, 3]; let mut n = 0; for x in xs { xs[2] = 100; xs.push(x); n += x; } n * 1000 + xs.len() * 100 + xs[2] }";
    assert_eq!(value(src), "6700");
}

/// `insert` takes the value and drops a value it replaces; `insert` and `remove` borrow the
/// key; `remove` drops the value; `keys` gives a new list that is dropped after use.
#[test]
fn map_insert_remove_and_keys_ownership() {
    let src = r#"fn main() -> Int {
        let mut m: Map<Str, Str> = Map::new();
        for i in 0..10 { m.insert(i.to_str() + "k", i.to_str() + "v"); m.insert(i.to_str() + "k", "v" + i.to_str()); }
        for i in 0..5 { m.remove(i.to_str() + "k"); }
        let mut n = 0;
        for i in 0..3 { n += m.keys().len(); n += m.keys()[0].len(); }
        let ms = [m]; let mut ms2 = ms; ms2[0].remove("9k");
        n * 100 + m.len() * 10 + ms2[0].len()
    }"#;
    assert_eq!(value(src), "2154");
}

#[test]
fn structs_enums_options_and_results() {
    let src = r#"struct P { x: Int, name: Str }
        enum Shape { Dot, Line(Int, Int), Named(Str) }
        fn area(s: Shape) -> Int { match s { Shape::Dot => 0, Shape::Line(a, b) => a * b, Shape::Named(n) => n.len() } }
        fn main() -> Str {
            let p = P { name: "pt" + "", x: 3 };
            let mut m: Map<Str, Int> = Map::new();
            m.insert("k", 4);
            let n = area(Shape::Line(p.x, 4)) + area(Shape::Dot) + area(Shape::Named(p.name));
            let a = "12".parse_int().unwrap_or(0) + "x".parse_int().unwrap_or(7) + m.get("k").unwrap_or(100) + m.get("j").unwrap_or(100);
            let r: Result<Int, Str> = Err("e" + "");
            let o: Option<Str> = None;
            let s: Option<Str> = Some("s" + "");
            let flags = [o.is_some(), o.is_none(), s.is_some(), r.is_err(), r.is_ok()];
            let mut t = "";
            for f in flags { if f { t += "1"; } else { t += "0"; } }
            n.to_str() + "," + a.to_str() + "," + r.unwrap_or(5).to_str() + p.name + o.unwrap_or("d") + s.unwrap_or("d") + t
        }"#;
    assert_eq!(value(src), "14,123,5ptds01110");
}

#[test]
fn main_returns_a_struct_enum_or_option() {
    let src = r#"struct P { x: Int, s: Str } fn main() -> P { P { x: 1, s: "a" + "b" } }"#;
    assert_eq!(value(src), r#"P { x: 1, s: "ab" }"#);
    assert_eq!(value("enum E { A, B(Int) } fn main() -> E { E::B(2) }"), "E::B(2)");
    assert_eq!(value("enum E { A, B(Int) } fn main() -> E { E::A }"), "E::A");
    assert_eq!(value("fn main() -> Option<Int> { None }"), "None");
    assert_eq!(value(r#"fn main() -> Option<Str> { Some("x" + "") }"#), r#"Some("x")"#);
}

/// A struct literal and a variant take their parts: the same `Str` stored twice is counted twice.
#[test]
fn struct_literal_and_variant_take_their_parts() {
    let src = r#"struct W { a: Str, b: Str }
        enum E { Two(Str, Str) }
        fn main() -> Str {
            let s = "x" + "y";
            let w = W { b: s, a: "p" + "q" };
            let v = W { a: s, b: s };
            let e = E::Two(s, s + "");
            match e { E::Two(c, d) => w.a + w.b + v.a + v.b + s + c + d }
        }"#;
    assert_eq!(value(src), "pqxyxyxyxyxyxy");
}

/// `w.s` is a new reference; a base that is not a local is dropped after the read.
#[test]
fn a_field_read_is_owned() {
    let src = r#"struct W { s: Str, n: Int }
        struct V { w: W }
        fn mk() -> W { W { s: "a" + "b", n: 1 } }
        fn main() -> Str { let w = mk(); let t = w.s; let u = mk().s; let v = V { w: mk() }; let x = v.w.s; t + w.s + u + x + v.w.n.to_str() }"#;
    assert_eq!(value(src), "abababab1");
}

/// A field write copies a shared struct (`rt_unique`), so the other copy is unchanged.
#[test]
fn field_write_copies_a_shared_struct() {
    let src = r#"struct P { x: Int, s: Str }
        fn main() -> Str {
            let mut a = P { x: 1, s: "s" + "" };
            let mut b = a;
            b.x = 2;
            b.s = "t" + "";
            a.s += "!";
            a.x -= 5;
            a.x.to_str() + a.s + b.x.to_str() + b.s
        }"#;
    assert_eq!(value(src), "-4s!2t");
}

/// Field and index steps mixed: each container on the path is made unique and stored back.
#[test]
fn nested_field_and_index_place() {
    let src = r#"struct In { v: List<Int>, n: Int }
        struct Out { ins: List<In>, name: Str }
        fn main() -> Str {
            let mut o = Out { ins: [In { v: [1, 2], n: 0 }, In { v: [3], n: 5 }], name: "o" + "" };
            let keep = o;
            let inner = o.ins[1];
            o.ins[1].n += 10;
            o.ins[0].v[1] = 7;
            o.ins[1].v.push(4);
            o.name = "p" + "";
            o.ins[0] = In { v: [o.ins[0].v[1]], n: o.ins[1].n };
            let k = o.ins[1].n * 1000 + o.ins[0].v[0] * 100 + o.ins[1].v.len() * 10 + keep.ins[1].n + inner.v.len() + keep.ins[0].v[1] + o.ins[0].n;
            k.to_str() + o.name + keep.name
        }"#;
    assert_eq!(value(src), "15743po");
}

/// Match bindings are new references, released at the end of their arm (here in a loop that
/// takes the arms in turn) and on a `return` from inside it.
#[test]
fn match_bindings_are_released() {
    let src = r#"enum T { Leaf(Str), Pair(Str, Str) }
        fn first(t: T) -> Str { match t { T::Pair(a, b) => { if a == "x" { return b; } a + b }, T::Leaf(s) => s } }
        fn main() -> Str {
            let mut out = "";
            for i in 0..4 {
                let t = if i % 2 == 0 { T::Pair(i.to_str() + "", "y" + "") } else { T::Leaf("l" + "") };
                match t { T::Pair(a, b) => { out += a; out += b; }, T::Leaf(s) => { out += s; } }
            }
            out + first(T::Pair("x" + "", "z" + "")) + first(T::Leaf("q" + "")) + first(T::Pair("m" + "", "n" + ""))
        }"#;
    assert_eq!(value(src), "0yl2ylzqmn");
}

/// The scrutinee is released when the match ends and on every exit from an arm: `break`,
/// `continue` and `return`.
#[test]
fn scrutinee_is_released_on_every_exit() {
    let src = r#"fn find(xs: List<Option<Str>>) -> Str {
            for x in xs { match x { Some(s) => { if s == "r" { return s + "!"; } }, None => {} } }
            "none"
        }
        fn main() -> Str {
            let xs = [Some("a" + ""), None, Some("b" + ""), Some("r" + ""), Some("c" + "")];
            let mut out = "";
            for x in xs {
                match x {
                    Some(s) => { if s == "b" { continue; } if s == "c" { break; } out += s; },
                    None => { out += "-"; },
                }
            }
            let mut i = 0;
            loop { i += 1; match Some(i.to_str() + "") { Some(s) => { if i == 3 { break; } out += s; }, None => {} } }
            out + find(xs) + find([None])
        }"#;
    assert_eq!(value(src), "a-r12r!none");
}

/// `?` returns early releasing every owned local; an `Err` rebuilt for the function's return
/// type holds its own reference to the error, and one of the same type is returned as is.
#[test]
fn question_mark_releases_and_keeps_the_error() {
    let src = r#"fn half(n: Int) -> Result<Int, Str> { if n % 2 == 0 { Ok(n / 2) } else { Err(n.to_str() + " is odd") } }
        fn quarter(n: Int) -> Result<Str, Str> { let keep = "k" + ""; let h = half(n)?; let q = half(h)?; Ok(keep + q.to_str()) }
        fn same(n: Int) -> Result<Int, Str> { let keep = "k" + ""; let r = half(n); let h = r?; Ok(h + keep.len()) }
        fn get(m: Map<Str, Str>, k: Str) -> Option<Int> { let keep = k + ""; let v = m.get(k)?; Some(v.len() + keep.len()) }
        fn show(r: Result<Str, Str>) -> Str { match r { Ok(s) => s, Err(e) => e } }
        fn main() -> Str {
            let mut m: Map<Str, Str> = Map::new();
            m.insert("a", "xyz" + "");
            let a = show(quarter(8)) + "," + show(quarter(3)) + "," + show(quarter(6));
            let b = same(4).unwrap_or(0) * 10 + same(5).unwrap_or(0);
            let c = get(m, "a").unwrap_or(0) * 10 + get(m, "b").unwrap_or(9);
            a + "," + b.to_str() + "," + c.to_str()
        }"#;
    assert_eq!(value(src), "k2,3 is odd,3 is odd,30,49");
}

/// `None` and payload-free variants are pinned statics: copied and dropped any number of times.
#[test]
fn pinned_variants_survive_any_number_of_drops() {
    let src = r#"enum C { Red, Green(Int) }
        fn id(c: C) -> C { c }
        fn main() -> Int {
            let mut n = 0;
            let none: Option<Str> = None;
            for i in 0..1000 {
                let a = C::Red; let b = id(a); let c = [b, C::Red, C::Green(i)];
                let o = none; let p = [o, None];
                if c[0] == C::Red && p[1] == none { n += 1; }
            }
            n
        }"#;
    assert_eq!(value(src), "1000");
}

/// `==` on structs and enums compares structure, not identity.
#[test]
fn struct_and_enum_equality() {
    let src = r#"struct P { x: Int, s: Str }
        enum E { A, B(Str, Option<Int>) }
        fn main() -> Str {
            let p = P { x: 1, s: "a" + "b" };
            let q = P { x: 1, s: "ab" };
            let e = E::B("x" + "", Some(2));
            let r = [p == q, p != q, e == E::B("x", Some(2)), e == E::B("x", None), e == E::A, E::A == E::A, Some(e) == Some(E::A)];
            let mut t = "";
            for f in r { if f { t += "1"; } else { t += "0"; } }
            t
        }"#;
    assert_eq!(value(src), "1010010");
}

/// Nested patterns: variants, literals and bindings at any depth, arms in order.
#[test]
fn nested_patterns() {
    let src = r#"enum E { A(Option<Int>, Str), B(Bool) }
        fn f(e: E) -> Str {
            match e {
                E::A(Some(0), _) => "zero",
                E::A(Some(n), "s") => "s" + n.to_str(),
                E::A(Some(n), t) => t + n.to_str(),
                E::A(None, t) => t,
                E::B(true) => "yes",
                E::B(_) => "no",
            }
        }
        fn main() -> Str {
            let mut out = f(E::A(Some(0), "q" + "")) + f(E::A(Some(4), "s" + "")) + f(E::A(Some(5), "t" + ""));
            out += f(E::A(None, "n" + "")) + f(E::B(true)) + f(E::B(false));
            if let Some(Some(x)) = Some(Some(7)) { out += x.to_str(); }
            let k = match 3 { 1 => "one", 3 => "three", _ => "many" };
            let l = match "b" + "" { "a" => out, _ => out + "." };
            k + l
        }"#;
    assert_eq!(value(src), "threezeros4t5nyesno7.");
}

/// `main`'s value and failed flag both ways: a `Result` that is `Err` fails and shows its error
/// alone (a `Str` bare); anything else shows as a value.
fn outcome(src: &str) -> (String, bool) {
    let c = Kio::new(limits()).unwrap().run(src, vec![]).unwrap().unwrap();
    let i = Interp::new(limits()).unwrap().run(src, vec![]).unwrap().unwrap();
    let (c, i) = ((c.value, c.failed), (i.value, i.failed));
    assert_eq!(c, i, "compiled vs interpreted: {src}");
    c
}

#[test]
fn main_returns_any_type() {
    let ok = |s: &str| (s.to_string(), false);
    let failed = |s: &str| (s.to_string(), true);
    assert_eq!(
        outcome(r#"fn main() -> Result<Int, Str> { Err("x" + "") }"#),
        failed("x")
    );
    assert_eq!(outcome(r#"fn main() -> Result<Int, Str> { Ok(3) }"#), ok("Ok(3)"));
    assert_eq!(outcome("fn main() -> Result<(), Str> { Ok(()) }"), ok("Ok(())"));
    assert_eq!(outcome("fn main() -> Result<Int, ()> { Err(()) }"), failed("()"));
    assert_eq!(outcome("fn main() -> Result<Int, Int> { Err(7) }"), failed("7"));
    assert_eq!(
        outcome(r#"fn main() -> Result<Int, List<Str>> { Err(["a" + "", "b"]) }"#),
        failed(r#"["a", "b"]"#)
    );
    let src = r#"enum E { Bad(Str) }
        fn f(n: Int) -> Result<Int, E> { if n > 1 { Err(E::Bad("big" + "")) } else { Ok(n) } }
        fn main() -> Result<Int, E> { let a = f(1)?; let b = f(a + 1)?; Ok(b) }"#;
    assert_eq!(outcome(src), failed(r#"E::Bad("big")"#));
    assert_eq!(
        outcome("fn main() -> Option<Result<Int, Int>> { Some(Err(1)) }"),
        ok("Some(Err(1))")
    );
    assert_eq!(
        outcome(r#"fn main() -> List<Str> { ["a" + "", "b"] }"#),
        ok(r#"["a", "b"]"#)
    );
    let src = r#"fn main() -> Map<Str, List<Int>> { let mut m: Map<Str, List<Int>> = Map::new(); m.insert("k", [1, 2]); m.insert("a", []); m }"#;
    assert_eq!(outcome(src), ok(r#"{"a": [], "k": [1, 2]}"#));
    assert_eq!(outcome("fn main() {}"), ok("()"));
}

/// The new `Str` methods, with Rust's edge cases: empty patterns, a whole-string match, and
/// `None` returned many times (pinned, never freed).
#[test]
fn str_ends_with_strip_find_replace() {
    let src = r#"fn show(o: Option<Str>) -> Str { match o { Some(s) => "[" + s + "]", None => "-" } }
        fn at(o: Option<Int>) -> Str { match o { Some(i) => i.to_str(), None => "-" } }
        fn main() -> Str {
            let s = "a,b,,c" + "";
            let mut out = "";
            for i in 0..3 { out += show(s.strip_prefix("x")) + at(s.find("z")) + show(s.strip_suffix("q")); }
            if s.ends_with(",c") && s.ends_with("") && !s.ends_with("b") { out += "E"; }
            out += show(s.strip_prefix("a,")) + show(s.strip_suffix(",c")) + show(s.strip_prefix("")) + show(s.strip_prefix(s));
            out += at(s.find(",")) + at(s.find("")) + at("héllo".find("l")) + at("".find(""));
            out += s.replace(",", ";") + "|" + s.replace(",,", "") + "|" + "ab".replace("", "-") + "|" + "aaa".replace("aa", "b") + "|" + s.replace("q", "r");
            out
        }"#;
    assert_eq!(
        value(src),
        "---------E[b,,c][a,b,][a,b,,c][]1030a;b;;c|a,bc|-a-b-|ba|a,b,,c"
    );
}

/// `contains` borrows the list and the value (each compared by `==`); `ok`, `err` and `ok_or`
/// borrow their receiver and argument and return new references, so every value here is used
/// again after them, and nothing leaks or is freed twice.
#[test]
fn contains_ok_err_ok_or_borrow() {
    let src = r#"struct P { s: Str }
        fn main() -> Str {
            let xs = [P { s: "a" + "" }, P { s: "b" + "" }];
            let k = P { s: "b" + "" };
            let mut out = "";
            for i in 0..3 {
                if xs.contains(k) && !xs.contains(P { s: "c" + "" }) && [[1], [2, 3]].contains([2, 3]) { out += "c"; }
            }
            let r: Result<Str, Str> = Ok("ok" + "");
            let e: Result<Str, Str> = Err("er" + "");
            let o: Option<Str> = Some("so" + "");
            let n: Option<Str> = None;
            let d = "dflt" + "";
            for i in 0..2 {
                out += r.ok().unwrap_or("-") + r.err().unwrap_or("-") + e.ok().unwrap_or("-") + e.err().unwrap_or("-");
                out += o.ok_or(d).unwrap_or("-") + n.ok_or(d).err().unwrap_or("-");
            }
            let t = o.ok_or(d);
            let u = n.ok_or(d);
            out + "," + r.ok().unwrap_or("") + e.err().unwrap_or("") + o.unwrap_or("") + d + t.unwrap_or("") + u.err().unwrap_or("") + k.s + xs[1].s
        }"#;
    assert_eq!(value(src), "cccok--ersodfltok--ersodflt,okersodfltsodfltbb");
}

/// `pop` moves the last element into the `Some` without another reference, so it is freed
/// once; on a shared list it copies first, at the root or below a field and an index.
#[test]
fn pop_moves_and_copies_a_shared_list() {
    let src = r#"struct B { xs: List<List<Str>> }
        fn main() -> Str {
            let mut xs = ["a" + "", "b" + "", "c" + ""];
            let ys = xs;
            let mut out = "";
            for i in 0..4 { match xs.pop() { Some(s) => { out += s; }, None => { out += "-"; } } }
            let mut b = B { xs: [["p" + "", "q" + ""]] };
            let c = b;
            let inner = b.xs[0];
            let q = b.xs[0].pop();
            xs.push("z" + "");
            xs.pop();
            out + q.unwrap_or("") + b.xs[0].len().to_str() + c.xs[0].len().to_str() + inner.len().to_str() + ys.len().to_str() + xs.len().to_str() + ys[2]
        }"#;
    assert_eq!(value(src), "cba-q12230c");
}

/// `sort` on a shared list copies it; `Int` sorts signed, `Str` by bytes.
#[test]
fn sort_copies_a_shared_list() {
    let src = r#"fn main() -> Str {
            let mut a = [3, -1, 2, -9223372036854775807 - 1, 0];
            let b = a;
            a.sort();
            let mut s = ["b" + "", "B", "a", "é", "", "ab"];
            let t = s;
            s.sort();
            let mut l = [[5, 1]];
            let m = l;
            l[0].sort();
            let mut out = "";
            for x in a { out += x.to_str() + " "; }
            for x in s { out += x + "."; }
            out + b[0].to_str() + t[0] + l[0][0].to_str() + m[0][0].to_str()
        }"#;
    assert_eq!(value(src), "-9223372036854775808 -1 0 2 3 .B.a.ab.b.é.3b15");
}

/// Floats the operator and method tests combine: signed zeros, NaN, infinities, the smallest
/// subnormal, the largest float, halves, and values around 2^63.
const FLOATS: &str = "[0.0, -0.0, 0.0 / 0.0, 1.0 / 0.0, -1.0 / 0.0, 5e-324, 1.7976931348623157e308, 0.1, 0.2,
    7.5, -2.0, 2.5, -2.5, 0.5, -0.5, 0.49999999999999994, 4503599627370495.5, 9223372036854775807.0,
    -9223372036854775808.0, 9223372036854774784.0, -9223372036854777856.0, 1e21, 1e-7, 2.7, -2.7]";

/// `to_str` is Rust's `{}`, at top level and nested; `main` may return a `Float`.
#[test]
fn float_to_str_matches_the_interpreter() {
    let tiny = format!("0.{}5", "0".repeat(323));
    for (e, want) in [
        ("1.0", "1"),
        ("0.1 + 0.2", "0.30000000000000004"),
        ("0.0 / 0.0", "NaN"),
        ("1.0 / 0.0", "inf"),
        ("-1.0 / 0.0", "-inf"),
        ("-0.0", "-0"),
        ("1e21", "1000000000000000000000"),
        ("1e-7", "0.0000001"),
        ("5e-324", &tiny),
    ] {
        assert_eq!(value(&format!("fn main() -> Str {{ ({e}).to_str() }}")), want);
        assert_eq!(value(&format!("fn main() -> Float {{ {e} }}")), want);
    }
    value(&format!("fn main() -> List<Float> {{ {FLOATS} }}"));
    let src = "struct P { x: Float }
               fn main() -> Map<Int, List<P>> {
                   let mut m: Map<Int, List<P>> = Map::new();
                   m.insert(1, [P { x: 0.1 + 0.2 }, P { x: -0.0 }]);
                   m
               }";
    assert_eq!(value(src), "{1: [P { x: 0.30000000000000004 }, P { x: -0 }]}");
    assert_eq!(value("fn main() -> Option<Float> { Some(-1.0 / 0.0) }"), "Some(-inf)");
}

/// Every operator on every pair: IEEE, as the interpreter's `f64`, including `%`.
#[test]
fn float_operators_on_every_pair() {
    let src = format!(
        "fn main() -> Str {{
            let xs = {FLOATS};
            let mut out = \"\";
            for a in xs {{
                for b in xs {{
                    out += (a + b).to_str() + \" \" + (a - b).to_str() + \" \" + (a * b).to_str() + \" \";
                    out += (a / b).to_str() + \" \" + (a % b).to_str() + \" \" + (-a).to_str() + \" \";
                    if a < b {{ out += \"<\"; }}
                    if a <= b {{ out += \"<=\"; }}
                    if a > b {{ out += \">\"; }}
                    if a >= b {{ out += \">=\"; }}
                    if a == b {{ out += \"==\"; }}
                    if a != b {{ out += \"!=\"; }}
                    out += \"\\n\";
                }}
            }}
            out
        }}"
    );
    let out = value(&src);
    assert_eq!(out.lines().count(), 625);
    assert!(out.starts_with("0 0 0 NaN NaN -0 <=>===\n"), "{}", &out[..40]);
}

/// The methods on every float: `round` halves away from zero (0.49999999999999994 is 0),
/// `to_int` is `None` past `Int` or for NaN and infinities.
#[test]
fn float_methods_match_the_interpreter() {
    let src = format!(
        "fn main() -> Str {{
            let mut out = \"\";
            for x in {FLOATS} {{
                out += x.to_str() + \": \" + x.abs().to_str() + \" \" + x.floor().to_str() + \" \" + x.ceil().to_str() + \" \";
                out += x.round().to_str() + \" \" + x.sqrt().to_str() + \" \";
                out += x.to_int().unwrap_or(7).to_str() + \" \";
                if x.to_int().is_some() {{ out += \"some \"; }}
                if x.is_nan() {{ out += \"nan \"; }}
                if x.is_finite() {{ out += \"finite \"; }}
                if x.is_infinite() {{ out += \"infinite \"; }}
                out += x.to_str().parse_float().unwrap_or(1.5).to_str() + \"\\n\";
            }}
            out
        }}"
    );
    let out = value(&src);
    assert!(
        out.contains("\n0.49999999999999994: 0.49999999999999994 0 1 0 "),
        "{out}"
    );
    assert!(out.contains("\n2.5: 2.5 2 3 3 "), "{out}");
    assert!(out.contains("\n-2.5: 2.5 -3 -2 -3 "), "{out}");
    let ints = "fn main() -> List<Float> {
                    [0.to_float(), (-7).to_float(), 9007199254740993.to_float(),
                     9223372036854775807.to_float(), (-9223372036854775807 - 1).to_float()]
                }";
    assert_eq!(
        value(ints),
        "[0, -7, 9007199254740992, 9223372036854776000, -9223372036854776000]"
    );
    let src = r#"fn main() -> List<Option<Float>> {
            let mut out: List<Option<Float>> = [];
            for s in ["1.5", "-2", "+3", "1e3", ".5", "5.", "inf", "-infinity", "NaN", "1e400", "-0", "", " 1", "1,5", "0x1", "e3"] {
                out.push(s.parse_float());
            }
            out
        }"#;
    assert_eq!(
        value(src),
        "[Some(1.5), Some(-2), Some(3), Some(1000), Some(0.5), Some(5), Some(inf), Some(-inf), Some(NaN), Some(inf), Some(-0), None, None, None, None, None]"
    );
}

/// `==` on floats is IEEE, also inside containers (`rt_eq`), and a value holding a NaN is not
/// equal to itself.
#[test]
fn float_equality_is_ieee_in_containers() {
    for (e, want) in [
        ("nan == nan", "false"),
        ("nan != nan", "true"),
        ("0.0 == -0.0", "true"),
        ("[nan] == [nan]", "false"),
        ("[0.0] == [-0.0]", "true"),
        ("Some(nan) == Some(nan)", "false"),
        ("[nan].contains(nan)", "false"),
        ("[-0.0].contains(0.0)", "true"),
        ("{ let xs = [nan]; xs == xs }", "false"),
        ("{ let o = Some(1.5); o == o }", "true"),
    ] {
        assert_eq!(
            value(&format!("fn main() -> Bool {{ let nan = 0.0 / 0.0; {e} }}")),
            want,
            "{e}"
        );
    }
}

/// `+=` and `-=` on a `Float` local, element and field; a `Float` payload bound by `match`.
#[test]
fn float_places_and_patterns() {
    let src = "struct P { x: Float }
               enum E { A(Float), B }
               fn half(x: Float) -> Option<Float> { if x > 0.0 { Some(x / 2.0) } else { None } }
               fn main() -> Str {
                   let mut x = 1.0;
                   x += 0.5;
                   x -= 2.0;
                   let mut ps = [P { x: 1.0 }];
                   ps[0].x += 2.0;
                   let mut xs = [1.0, 2.0];
                   xs[1] -= 0.25;
                   let mut p = P { x: 0.1 };
                   p.x += 0.2;
                   let e = E::A(-0.0);
                   let y = match e { E::A(v) => v, E::B => 1.0 };
                   let z = match half(3.0) { Some(h) => h, None => 0.0 };
                   x.to_str() + \" \" + ps[0].x.to_str() + \" \" + xs[1].to_str() + \" \" + p.x.to_str() + \" \" + y.to_str() + \" \" + z.to_str() + \" \" + half(-1.0).unwrap_or(9.0).to_str()
               }";
    assert_eq!(value(src), "-0.5 3 1.75 0.30000000000000004 -0 1.5 9");
}

/// `slice` on `Str` and `Bytes`, and both of its errors with the interpreter's messages and
/// spans; the out-of-bounds message's length comes through the runtime's `EXTRA` site.
#[test]
fn slices_and_their_errors() {
    let src = r#"fn main() -> Str { "hello".slice(1, 3) + "|" + "aé".slice(1, 3) + "|" + "".slice(0, 0) + "|" + "hello".slice(5, 5) }"#;
    assert_eq!(value(src), "el|é||");
    let src = r#"fn main() -> Bool { "abc".to_bytes().slice(1, 3) == "bc".to_bytes() && "abc".to_bytes().slice(3, 3).len() == 0 }"#;
    assert_eq!(value(src), "true");
    for (call, msg) in [
        (r#""abc".slice(2, 4)"#, "slice 2..4 out of bounds for length 3"),
        (r#""abc".slice(3, 1)"#, "slice 3..1 out of bounds for length 3"),
        (r#""abc".slice(-1, 2)"#, "slice -1..2 out of bounds for length 3"),
        (
            r#""abc".slice(-9223372036854775807 - 1, 9223372036854775807)"#,
            "slice -9223372036854775808..9223372036854775807 out of bounds for length 3",
        ),
        (
            r#""abc".slice(0, 4294967296)"#,
            "slice 0..4294967296 out of bounds for length 3",
        ),
        (r#""aé".slice(0, 2)"#, "slice 0..2 is not on a character boundary"),
        (r#""aé".slice(2, 3)"#, "slice 2..3 is not on a character boundary"),
        (r#""é".slice(1, 3)"#, "slice 1..3 out of bounds for length 2"),
        (
            r#""abc".to_bytes().slice(1, 9)"#,
            "slice 1..9 out of bounds for length 3",
        ),
        (
            r#""abc".to_bytes().slice(2, 1)"#,
            "slice 2..1 out of bounds for length 3",
        ),
    ] {
        let src = format!("fn main() -> Int {{ {call}.len() }}");
        let (m, start, end) = both(&src).unwrap_err();
        assert_eq!((m.as_str(), &src[start..end]), (msg, call));
    }
}

/// A closure holds its own reference (+1) to each capture, released when the closure dies:
/// the captured value outlives the closure and the closure outlives its scope's locals.
#[test]
fn closure_copies_and_releases_its_captures() {
    let src = r#"fn make(s: Str) -> fn(Int) -> Str { let t = s + "!"; |n| t + n.to_str() }
        fn main() -> Str {
            let s = "a" + "b";
            let mut out = "";
            if true { let f = |x: Int| s + x.to_str(); out += f(1); }
            let g = make(s);
            out + s + g(2)
        }"#;
    assert_eq!(value(src), "ab1abab!2");
}

/// The callee of a call through a value is dropped after the call: a fresh closure from a
/// call, or one read from a list.
#[test]
fn callee_is_dropped_after_the_call() {
    let src = r#"fn adder(n: Int) -> fn(Str) -> Str { let p = n.to_str() + ":"; |s| p + s }
        fn main() -> Str {
            let fs = [adder(7), adder(8)];
            let mut out = "";
            for i in 0..3 { out += adder(i)("x" + "") + (fs[i % 2])("y" + ""); }
            out
        }"#;
    assert_eq!(value(src), "0:x7:y1:x8:y2:x7:y");
}

/// Closures in struct fields and lists, called after the locals they captured are gone.
#[test]
fn closures_stored_and_called_later() {
    let src = r#"struct Op { name: Str, run: fn(Str) -> Str }
        fn ops() -> List<Op> {
            let k = "k" + "";
            [Op { name: "add" + "", run: |x| x + k }, Op { name: "dup", run: |x| x + x }]
        }
        fn main() -> Str {
            let mut out = "";
            let os = ops();
            for op in os { out += op.name + "=" + (op.run)("a" + "") + " "; }
            let o = os[0];
            let fs: List<fn(Str) -> Str> = [o.run, |x| x];
            out + (fs[0])("b") + (os[1].run)("c")
        }"#;
    assert_eq!(value(src), "add=ak dup=aa bkcc");
}

/// A closure returning a closure: the inner one captures the outer's parameter and, through
/// the outer, a local of `main`.
#[test]
fn closure_returning_a_closure() {
    let src = r#"fn main() -> Str {
            let a = "a" + "";
            let add = |x: Str| |y: Str| x + y + a;
            let f = add("x" + "");
            let g = add("z" + "");
            let nested = |x: Str| { let b = "b" + ""; let h = |y: Str| x + y + a + b; h("1") };
            f("y" + "") + g("w") + add("p")("q") + nested("n" + "")
        }"#;
    assert_eq!(value(src), "xyazwapqan1ab");
}

/// A function used as a value is one pinned object: copied and dropped any number of times,
/// never freed, and displayed `<fn name>`; a closure displays `<closure>`.
#[test]
fn function_items_are_pinned() {
    let src = r#"fn double(x: Int) -> Int { x * 2 }
        fn main() -> Int {
            let mut n = 0;
            for i in 0..1000 { let f = double; let fs = [f, double]; let g = fs[1]; n += g(i) - f(i); n += (fs[0])(1); }
            n
        }"#;
    assert_eq!(value(src), "2000");
    let src = "fn double(x: Int) -> Int { x * 2 } fn main() -> List<fn(Int) -> Int> { [double] }";
    assert_eq!(value(src), "[<fn double>]");
    assert_eq!(value("fn f() {} fn main() -> fn() { f }"), "<fn f>");
    let src = r#"struct H { f: fn(Int) -> Int, g: Option<fn() -> Str> }
        fn main() -> H { let s = "s" + ""; H { f: |x| x, g: Some(|| s) } }"#;
    assert_eq!(value(src), "H { f: <closure>, g: Some(<closure>) }");
}

/// Each call through a closure gives the lifted function its own reference to each capture,
/// so a closure called many times keeps its captures and leaks nothing.
#[test]
fn closure_called_in_a_loop() {
    let src = r#"fn main() -> Int {
            let s = "abc" + "";
            let xs = [1, 2];
            let f = |i: Int| s.len() + xs.len() + i;
            let mut n = 0;
            for i in 0..1000 { n += f(i); }
            n + s.len()
        }"#;
    assert_eq!(value(src), "504503");
}

/// Captures and parameters of every kind of value: `()`, `Bool`, `Float`, and a closure.
#[test]
fn closures_over_scalars_and_unit() {
    let src = r#"fn main() -> Str {
            let u = ();
            let b = true;
            let x = 1.5;
            let inner = |n: Int| n + 1;
            let f = |v: (), y: Float| { let w = u; if b { (x + y).to_str() + inner(1).to_str() } else { "" } };
            let g = || f((), 2.0);
            g() + f(u, 0.25)
        }"#;
    assert_eq!(value(src), "3.521.752");
}

/// Recursion through `fn` values is bounded by `--stack`, as direct recursion.
#[test]
fn deep_recursion_through_values_is_a_stack_overflow() {
    let t = std::thread::Builder::new().stack_size(4 * kio_host::MAX_STACK);
    t.spawn(|| {
        let src = "fn spin(f: fn(Int) -> Int, n: Int) -> Int { if n == 0 { 0 } else { let g = |x: Int| spin(f, x); 1 + g(n - 1) } }
            fn main() -> Int { spin(|x| x, 10000000) }";
        let e = Kio::new(Limits::default()).unwrap().run(src, vec![]).unwrap_err();
        assert_eq!(e.to_string(), "stack overflow (limit 2048 KB; raise it with --stack)");
        assert_eq!(value(&src.replace("10000000", "300")), "300");
    })
    .unwrap()
    .join()
    .unwrap();
}

/// C9b: `map`, `filter`, `any` and `all` pass each element +1, which the function consumes:
/// the list keeps its own reference and is intact afterwards.
#[test]
fn list_builtins_pass_each_element_owned() {
    let src = r#"fn b(x: Bool) -> Str { if x { "true" } else { "false" } }
        fn main() -> Str {
            let xs = ["a" + "", "bb" + "", "ccc" + ""];
            let n = xs.map(|s| s.len());
            let any = xs.any(|s| s == "zz");
            let all = xs.all(|s| s.len() > 0);
            let f = xs.filter(|s| s.len() == 9);
            xs[0] + xs[1] + xs[2] + n[2].to_str() + b(any) + b(all) + f.len().to_str()
        }"#;
    assert_eq!(value(src), "abbccc3falsetrue0");
}

/// `filter` keeps an element with a reference of its own (the call consumed the one it was
/// given): both lists outlive each other's drop.
#[test]
fn filter_keeps_its_own_reference() {
    let src = r#"fn keep(xs: List<Str>) -> List<Str> { xs.filter(|s| s.len() > 1) }
        fn main() -> Str {
            let mut ys = ["x" + ""];
            if true { let xs = ["a" + "", "bb" + "", "ccc" + ""]; ys = keep(xs); }
            let zs = ys.filter(|s| true);
            ys = [];
            zs[0] + zs[1] + zs.len().to_str()
        }"#;
    assert_eq!(value(src), "bbccc2");
    let src = "fn main() -> List<List<Int>> { let xs = [[1], [2, 3], []]; xs.filter(|l| l.len() > 0) }";
    assert_eq!(value(src), "[[1], [2, 3]]");
}

/// `map`'s new list takes each result: nothing is left over when it is dropped.
#[test]
fn map_results_go_into_the_new_list() {
    let src = r#"fn main() -> Str {
            let xs = [1, 2, 3];
            let ys = xs.map(|x| x.to_str() + "!");
            let zs = ys.map(|s| [s, s]);
            ys[2] + zs[0][1] + zs.len().to_str()
        }"#;
    assert_eq!(value(src), "3!1!3");
}

/// The receiver and the function value are released on every path of every method: an empty
/// list, a short-circuiting `any`/`all`, each variant of `Option` and `Result`, and a receiver
/// made fresh for the call.
#[test]
fn receiver_and_function_are_released() {
    let src = r#"fn b(x: Bool) -> Str { if x { "true" } else { "false" } }
        fn main() -> Str {
            let k = "k" + "";
            let e: List<Str> = [];
            let xs = ["a" + "", "b" + ""];
            let mut out = "";
            out += e.map(|s| s + k).len().to_str() + e.filter(|s| s == k).len().to_str();
            out += b(e.any(|s| s == k)) + b(e.all(|s| s == k));
            out += b(xs.any(|s| s + k == "ak")) + b(xs.all(|s| s + k == "bk"));
            out += ["x" + ""].map(|s| s + k)[0] + ["y" + ""].filter(|s| s != k)[0];
            let some = Some("s" + "");
            let none: Option<Str> = None;
            out += some.map(|s| s + k).unwrap_or("") + none.map(|s| s + k).unwrap_or("-");
            out += some.and_then(|s| Some(s + k)).unwrap_or("") + none.and_then(|s| Some(s + k)).unwrap_or("-");
            let ok: Result<Str, Str> = Ok("o" + "");
            let err: Result<Str, Str> = Err("e" + "");
            out += ok.map(|s| s + k).unwrap_or("") + err.map(|s| s + k).unwrap_or("-");
            out += ok.map_err(|s| s + k).unwrap_or("") + err.map_err(|s| s + k).err().unwrap_or("");
            out += ok.and_then(|s| Ok(s + k)).unwrap_or("") + err.and_then(|s| Ok(s + k)).unwrap_or("-");
            out += Some("t" + "").map(|s| s + k).unwrap_or("");
            out
        }"#;
    assert_eq!(value(src), "00falsetruetruefalsexkysk-sk-ok-oekok-tk");
}

/// The variant a built-in does not call on is rebuilt as the result's type: the pinned `None`
/// of `Option<Str>`; a new `Err` of the result's descriptor holding its own reference to the
/// error; the receiver itself when the types are equal.
#[test]
fn untouched_variants_are_rebuilt() {
    let src = r#"fn main() -> List<Option<Str>> {
            let n: Option<Int> = None;
            let m = n.map(|x| x.to_str());
            [m, n.and_then(|x| Some(x.to_str())), m]
        }"#;
    assert_eq!(value(src), "[None, None, None]");
    let src = r#"fn main() -> Str {
            let mut out = "";
            let r: Result<Int, Str> = Err("bad" + "");
            let a = r.map(|x| x.to_str());
            let b = r.map(|x| x + 1);
            let c = r.and_then(|x| Ok([x]));
            if true { let d = r; }
            let s: Result<Int, Str> = Ok(4);
            let t = s.map_err(|e| e.len());
            out += a.err().unwrap_or("") + b.err().unwrap_or("") + c.err().unwrap_or("");
            match t { Ok(v) => { out += v.to_str(); } Err(_) => {} }
            out
        }"#;
    assert_eq!(value(src), "badbadbad4");
    let src = r#"fn main() -> Result<List<Int>, Str> { let r: Result<Int, Str> = Err("x" + ""); r.map(|x| [x]) }"#;
    assert_eq!(both(src), Ok("x".to_string()));
    // The reused `Err` outlives the receiver's local; the objects made after it would take
    // its memory if it had no reference of its own.
    let src = r#"fn g(n: Int) -> Result<Int, Str> { let r: Result<Int, Str> = Err(n.to_str() + "!"); r.map(|x| x + 1) }
        fn main() -> Str {
            let a = g(1);
            let b = g(2);
            let mut out = "";
            match a { Err(e) => { out += e; } Ok(_) => {} }
            match b { Err(e) => { out += e; } Ok(_) => {} }
            out
        }"#;
    assert_eq!(value(src), "1!2!");
}

/// A closure capturing a `Str`, called by `map` on 1,000 elements: each call gets its own
/// reference to the capture, and nothing leaks.
#[test]
fn map_with_a_capturing_closure_over_many_elements() {
    let src = r#"fn main() -> Int {
            let s = "abc" + "";
            let mut xs: List<Int> = [];
            for i in 0..1000 { xs.push(i); }
            let ys = xs.map(|x| s + x.to_str());
            let zs = ys.filter(|y| y.len() == 5).map(|y| y.len() + s.len());
            ys.len() + zs.len() + zs[0] + s.len()
        }"#;
    assert_eq!(value(src), "1101");
}

/// The interpreter's order: the receiver, then the function value once, then the calls in list
/// order; `any`/`all` stop early; an error inside the function has the function's span.
#[test]
fn functions_given_to_builtins_run_in_order() {
    assert_eq!(value("fn main() -> Bool { [1, 0].any(|x| 10 / x > 0) }"), "true");
    assert_eq!(value("fn main() -> Bool { [-1, 0].all(|x| 10 / x > 0) }"), "false");
    let src = "fn main() -> Bool { [0, 1].all(|x| 10 / x > 0) }";
    let (m, start, end) = both(src).unwrap_err();
    assert_eq!((m.as_str(), &src[start..end]), ("division by zero", "10 / x"));
    let src = "fn main() -> Int {
            let mut n = 1;
            let xs = [{ n = n * 10; n }, 0];
            xs.map({ n = n + 1; |x: Int| x + n })[0]
        }";
    assert_eq!(value(src), "21");
    // `?` in the function returns from the function; an exit while the function value is
    // evaluated releases the receiver.
    let src = r#"fn half(x: Int) -> Result<Int, Str> { if x % 2 == 0 { Ok(x / 2) } else { Err("odd" + "") } }
        fn f(r: Result<Int, Str>, stop: Bool) -> Result<Int, Str> {
            let xs = ["a" + ""];
            let ys = xs.map(if stop { return Err("stop" + ""); } else { |s: Str| s });
            r.and_then(|x| { let h = half(x)?; Ok(h + ys.len()) })
        }
        fn main() -> Str {
            let a = f(Ok(4), false);
            let b = f(Ok(3), false);
            let c = f(Ok(4), true);
            a.unwrap_or(0).to_str() + b.err().unwrap_or("") + c.err().unwrap_or("")
        }"#;
    assert_eq!(value(src), "3oddstop");
}
