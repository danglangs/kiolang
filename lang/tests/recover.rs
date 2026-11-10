//! L23: every parse error reported, not the first.
use lang::parse::parse;
use lang::{check, check_files, CapTable, Diag};

/// Each diagnostic as (message, the source it points at, hint).
fn errors(src: &str) -> Vec<(String, String, String)> {
    let diags = check(src, &CapTable::default()).expect_err("should be rejected");
    diags.iter().map(|d| shown(src, d)).collect()
}

fn shown(src: &str, d: &Diag) -> (String, String, String) {
    let at = src[d.span.start..d.span.end].to_string();
    (d.msg.clone(), at, d.hint.clone().unwrap_or_default())
}

fn rows(want: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    want.iter()
        .map(|(m, a, h)| (m.to_string(), a.to_string(), h.to_string()))
        .collect()
}

const RUSTY: [&str; 3] = [
    "fn greet(name: &str) -> Int {\n    println!(\"hi {}\", name);\n    0\n}",
    "fn total(xs: Vec<i64>) -> Int {\n    let mut t = 0;\n    for x in xs { t += x as Int; }\n    t\n}",
    "fn pair() -> Int {\n    let p = (1, 2);\n    p\n}",
];

#[test]
fn every_rust_form_in_source_order() {
    let src = RUSTY.join("\n");
    let as_hint = "kio has one integer type, `Int`; convert to and from `Float` with `.to_float()` and `.to_int()`";
    let want = [
        (
            "kio has no references",
            "&",
            "pass by value; use `Str` instead of `&str`",
        ),
        ("macros are not supported", "println!", "return the value from `main`"),
        ("unknown type `Vec`", "Vec<i64>", "use `List<T>`"),
        ("unknown type `i64`", "i64", "use `Int`"),
        ("`as` is not supported in kio", "as", as_hint),
        ("kio has no tuples", ",", "use a struct"),
    ];
    assert_eq!(errors(&src), rows(&want));
}

/// Every error reported is one the item alone gives, and each item's first error (as the
/// parser that stops at it finds it) is reported.
#[test]
fn no_error_a_single_parse_would_not_give() {
    let programs: &[&[&str]] = &[
        &RUSTY,
        &[
            "fn f() {\n    println!(\"a\";\n}",
            "fn g() -> Int {\n    let x = 1\n    x\n}",
            "fn h() { let (a, b) = p; let c = 1 as Float; vec![1]; }",
        ],
        &[
            "use std::io;",
            "#[derive(Debug)]\nstruct P {\n    a: Int,\n}",
            "impl P {\n    fn new() -> P { P { a: 1 } }\n}",
            "fn main() {\n    let p = P::new();\n}",
            "fn k(a: Int) -> (Int, Int) {\n    println!(\"x\");\n}",
        ],
        &[
            "fn f<'a>(x: &'a Str) -> Str { 'c' }",
            "fn g(x: &mut List<Int>) { x & y; }",
            "/* note */ fn h() { format!(\"a\"); }",
        ],
    ];
    for items in programs {
        let src = items.join("\n");
        let all = check(&src, &CapTable::default()).expect_err("should be rejected");
        let mut alone = Vec::new();
        for item in *items {
            let start = src.find(item).unwrap();
            if let Err(d) = parse(item) {
                assert!(
                    all.iter()
                        .any(|a| a.span.start == d.span.start + start && a.msg == d.msg),
                    "first error of {item:?} not reported: {d:?}\n{all:?}"
                );
            }
            if let Err(ds) = check(item, &CapTable::default()) {
                alone.extend(ds.iter().map(|d| (d.span.start + start, d.msg.clone())));
            }
        }
        for d in &all {
            // The checker stops at a function's first unknown type: a later one is checked alone.
            let (msg, at, _) = shown(&src, d);
            let unknown = msg == format!("unknown type `{at}`")
                && check(&format!("fn f(a: {at}) {{}}"), &CapTable::default()).is_err_and(|ds| ds[0].msg == msg);
            assert!(
                unknown || alone.contains(&(d.span.start, msg.clone())),
                "{:?} is not an error of its item alone, in {src}",
                shown(&src, d)
            );
        }
    }
}

#[test]
fn lexer_errors_step_over_without_cascades() {
    let src = "fn f<'a>(x: &'a Str) -> Str { 'c' }\nfn g(x: &mut List<Int>) { x & y; }\n/* note */ fn main() {}";
    let msgs: Vec<String> = errors(src).into_iter().map(|e| e.0).collect();
    let lifetime = "kio has no chars or lifetimes";
    let reference = "kio has no references";
    let want = [
        lifetime,
        reference,
        lifetime,
        lifetime,
        reference,
        reference,
        "kio has no block comments",
    ];
    assert_eq!(msgs, want);
}

#[test]
fn at_most_twenty() {
    let src: String = (0..25).map(|i| format!("fn f{i}() {{ println!(\"x\"); }}\n")).collect();
    let all = errors(&src);
    assert_eq!(all.len(), 21);
    assert_eq!(all[20].0, "5 more errors not shown");
    assert_eq!(all[20].2, "fix the errors above first");
}

#[test]
fn every_file_of_a_program() {
    let files = [
        ("main", "mod util;\nfn main() { println!(\"a\"); }"),
        ("util.kio", "pub fn f(x: i64) -> Int { x as Int }"),
    ];
    let diags = check_files(&files, &CapTable::default()).expect_err("should be rejected");
    let got: Vec<(u32, String)> = diags.iter().map(|d| (d.span.file, d.msg.clone())).collect();
    let want = [
        (0, "macros are not supported"),
        (1, "unknown type `i64`"),
        (1, "`as` is not supported in kio"),
    ];
    assert_eq!(got, want.map(|(f, m)| (f, m.to_string())));
}

/// A Rust type name is reported only where the checker would report it.
#[test]
fn rust_type_names_as_the_checker_sees_them() {
    let src =
        "struct Vec { a: Int }\nfn f<i64>(v: Vec, x: i64, s: &str, t: String, u: String) -> bool { println!(\"a\"); }";
    let want = [
        (
            "kio has no references",
            "&",
            "pass by value; use `Str` instead of `&str`",
        ),
        ("unknown type `String`", "String", "use `Str`"),
        ("unknown type `bool`", "bool", "use `Bool`"),
        ("macros are not supported", "println!", "return the value from `main`"),
    ];
    assert_eq!(errors(src), rows(&want));
}

/// The parser that stops at the first error is unchanged.
#[test]
fn parse_still_stops_at_the_first_error() {
    assert_eq!(
        parse("fn f() { println!(\"a\"); }\nfn g() { 1 as Int }")
            .unwrap_err()
            .msg,
        "macros are not supported"
    );
    assert_eq!(
        parse("fn f() { a b } fn g(x: &Str) {}").unwrap_err().msg,
        "kio has no references"
    );
}
