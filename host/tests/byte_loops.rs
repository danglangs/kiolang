//! C25: byte loops. `b[i]` without its bounds check where `0 <= i < b.len()` is known,
//! `b.len()` read once for `while i < b.len()`, `&&` and `||` on plain comparisons without a
//! branch. Every program runs compiled and interpreted and must agree, out-of-bounds errors at
//! the same span.

use kio_host::{Interp, Kio, Limits};

type Run = Result<String, (String, usize, usize)>;

fn compiled(src: &str) -> Run {
    match Kio::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn both(src: &str) -> Run {
    let c = compiled(src);
    assert_eq!(c, interpreted(src), "compiled vs interpreted: {src}");
    c
}

fn program(body: &str) -> String {
    format!(
        r#"fn at(b: Bytes, i: Int) -> Int {{ if i < b.len() {{ b[i] }} else {{ -1 }} }}
fn main() -> Int {{ let mut b = "a b\"c\n".to_bytes(); let c = "xy".to_bytes(); let mut n = 0; {body} n }}"#
    )
}

/// The shapes whose check is dropped give the interpreter's values.
#[test]
fn unchecked_reads_agree() {
    for (body, want) in [
        ("let mut i = 0; while i < b.len() { n += b[i]; i += 1; }", "370"),
        (
            "let mut i = 0; while b.len() > i { n = n * 3 + b[i]; i += 1; }",
            "29422",
        ),
        ("let mut i = 0; while i < b.len() && b[i] != 32 { i += 1; } n = i;", "1"),
        (
            "for i in 0..b.len() { if b[i] == 34 || b[i] == 32 || b[i] == 10 { n += 1; } }",
            "3",
        ),
        (
            "for i in 2..b.len() { let mut j = i; while j < b.len() { n += b[j] - b[i]; j += 2; } }",
            "-23",
        ),
        (
            "let mut i = 0; while at(b, i) != -1 { let x = at(b, i); if x == 34 || x == 10 { n += 1; } i += 1; }",
            "2",
        ),
        (
            "let mut i = 0; while i < b.len() { if i < c.len() && b[i] > c[i] { n += 1; } i += 1; }",
            "0",
        ),
        (
            "let mut i = 0; while i < b.len() { let mut q = 0; while q < 2 { n += b[i]; q += 1; } i += 2; }",
            "588",
        ),
        (
            "let mut i = 0; while i < b.len() { if b[i] == 32 { b.push(33); } i += 1; } n = b.len();",
            "7",
        ),
        (
            "let mut i = 0; while i < b.len() { if b[i] == 99 { break; } i += 1; continue; } n = i;",
            "4",
        ),
        (
            "let mut i = 0; while i < b.len() { i += 1; if i == 3 { continue; } n += b[i - 1]; }",
            "272",
        ),
    ] {
        assert_eq!(both(&program(body)), Ok(want.to_string()), "{body}");
    }
}

/// The near misses keep their check: the same error, at the same span, both ways.
#[test]
fn near_misses_still_fail() {
    let oob = |body: &str, index: i64, len: i64| {
        let src = program(body);
        let (msg, start, end) = both(&src).expect_err(body);
        assert_eq!(msg, format!("index {index} out of bounds for length {len}"), "{body}");
        assert_eq!(&src[start..end], "b[i]", "{body}");
    };
    oob("let mut i = 0; while i < b.len() { i += 1; n += b[i]; }", 6, 6);
    oob("let mut i = 0; while i <= b.len() { n += b[i]; i += 1; }", 6, 6);
    oob("let mut i = -1; while i < b.len() { n += b[i]; i += 1; }", -1, 6);
    oob("let mut i = 2; while i < b.len() { n += b[i]; i -= 1; }", -1, 6);
    oob(
        "let mut i = 0; while i < c.len() { b = \"\".to_bytes(); n += b[i]; i += 1; }",
        0,
        0,
    );
    oob(
        "let mut i = 0; if i < b.len() { n = 1; } else { n = 2; } i = 9; n += b[i];",
        9,
        6,
    );
    oob(
        "let mut i = 0; while i < b.len() || n < 1000 { n += b[i]; i += 1; }",
        6,
        6,
    );
    oob(
        "let mut i = 0; while i < b.len() { let mut q = 0; while q < 4 { n += b[i]; i += 1; q += 1; } }",
        6,
        6,
    );
    // The inlined helper's own `b[i]` with a negative argument: `i < b.len()` holds, `i >= 0` not.
    let src = program("n = at(b, -2);");
    let (msg, start, end) = both(&src).unwrap_err();
    assert_eq!(msg, "index -2 out of bounds for length 6");
    assert_eq!(&src[start..end], "b[i]");
}
