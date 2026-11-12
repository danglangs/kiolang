//! T1a: `#[test]` functions, `assert` and `assert_eq`.
use lang::lower::{lower, lower_entry};
use lang::{check, check_files, report, run, run_entry, Caps, RunErr, Value};

mod common;

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

/// The single error for `src`: message, hint and the source it points at.
fn reject(src: &str) -> (String, String, String) {
    let diags = check(src, &common::caps()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    let hint = d.hint.clone().unwrap_or_default();
    (d.msg.clone(), hint, src[d.span.start..d.span.end].to_string())
}

fn rejects(src: &str, msg: &str, hint: &str, at: &str) {
    assert_eq!(
        reject(src),
        (msg.to_string(), hint.to_string(), at.to_string()),
        "{src}"
    );
}

fn ok(src: &str) -> lang::Program {
    check(src, &common::caps()).unwrap_or_else(|d| panic!("{src}\n{d:?}"))
}

/// Runs function `entry` of `src`: its value, or the error and the source it points at.
fn exec(src: &str, entry: &str) -> Result<Value, (String, String)> {
    let p = ok(src);
    let entry = entry.to_string();
    let r: Result<Value, RunErr> = common::big_stack(move || run_entry(&p, &common::caps(), &entry, &mut NoCaps));
    r.map_err(|e| (e.msg, src[e.span.start..e.span.end].to_string()))
}

fn fails(src: &str, entry: &str, msg: &str, at: &str) {
    assert_eq!(exec(src, entry), Err((msg.to_string(), at.to_string())), "{src}");
}

#[test]
fn test_functions_are_listed() {
    let p = ok("#[test]\nfn b() {}\n#[test] pub fn a(d: Dir, log: Log) -> Result<(), Str> { Ok(()) }\nfn main() {}");
    let tests = p.tests();
    let names: Vec<_> = tests.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(tests[0].module, "");
    let params = [
        ("d".to_string(), "Dir".to_string()),
        ("log".to_string(), "Log".to_string()),
    ];
    assert_eq!(tests[0].params, params);
    assert!(tests[1].params.is_empty());
}

#[test]
fn test_in_a_module() {
    let files = [
        ("app", "mod web;\nfn main() { web::f(); }"),
        ("web", "pub fn f() {}\n#[test]\nfn t() { assert_eq(1 + 1, 2); }"),
    ];
    let p = check_files(&files, &common::caps()).unwrap_or_else(|d| panic!("{d:?}"));
    let tests = p.tests();
    assert_eq!((tests[0].name.as_str(), tests[0].module.as_str()), ("web::t", "web"));
    assert_eq!(tests[0].span.file, 1);
    let r = common::big_stack(move || run_entry(&p, &common::caps(), "web::t", &mut NoCaps));
    assert_eq!(r, Ok(Value::Unit));
}

#[test]
fn rust_assert_macros() {
    let hint = "they are functions in kio: `assert(cond, \"why\")`, `assert_eq(a, b)`";
    rejects(
        "fn main() { assert!(true); }",
        "macros are not supported",
        hint,
        "assert!",
    );
    rejects(
        "fn main() { assert_eq!(1, 1); }",
        "macros are not supported",
        hint,
        "assert_eq!",
    );
}

#[test]
fn unknown_attribute() {
    let hint = "kio has one attribute, `#[test]`";
    rejects(
        "#[derive(Debug)]\nstruct P { a: Int }\nfn main() {}",
        "unknown attribute `#[derive]`",
        hint,
        "#[derive(Debug)]",
    );
    rejects(
        "#![allow(x)]\nfn main() {}",
        "unknown attribute `#![allow]`",
        hint,
        "#![allow(x)]",
    );
    rejects(
        "#[test]\n#[ignore]\nfn t() {}\nfn main() {}",
        "unknown attribute `#[ignore]`",
        hint,
        "#[ignore]",
    );
    rejects(
        "fn main() { #[allow(x)] let a = 1; }",
        "unknown attribute `#[allow]`",
        hint,
        "#[allow(x)]",
    );
}

#[test]
fn test_attribute_marks_a_top_level_function() {
    let msg = "`#[test]` marks a top-level function";
    let hint = "put it on the line before a function at the top level of a file";
    rejects("#[test]\nstruct S { a: Int }\nfn main() {}", msg, hint, "#[test]");
    rejects("fn main() {}\n#[test]", msg, hint, "#[test]");
    rejects("fn main() { #[test] let a = 1; }", msg, hint, "#[test]");
}

#[test]
fn test_signature_rules() {
    rejects(
        "#[test]\nfn t<T>() {}\nfn main() {}",
        "a test cannot be generic",
        "remove `<..>`: `kio test` calls a test with no type arguments",
        "T",
    );
    let msg = "a test returns `()` or `Result<(), E>`";
    let hint = "return `()` and fail with `assert`, or return `Result<(), E>` and fail with `Err`";
    rejects("#[test]\nfn t() -> Int { 1 }\nfn main() {}", msg, hint, "Int");
    rejects(
        "#[test]\nfn t() -> Result<Int, Str> { Ok(1) }\nfn main() {}",
        msg,
        hint,
        "Result<Int, Str>",
    );
    rejects(
        "#[test]\nfn t(d: Dir, n: Int) {}\nfn main() {}",
        "a test's parameters are capabilities the runner grants",
        "build the value inside the test",
        "n: Int",
    );
    rejects(
        "#[test]\nfn main() {}",
        "`main` cannot be a test",
        "rename the test",
        "#[test]\nfn main() {}",
    );
    ok("#[test]\nfn t(d: Dir, k: Kv) -> Result<(), IoErr> { Ok(()) }\nfn main() {}");
}

#[test]
fn main_is_still_required() {
    rejects("#[test]\nfn t() {}", "no `main` function", "add `fn main() { .. }`", "");
}

#[test]
fn code_cannot_call_a_test() {
    let msg = "`t` is a test; only `kio test` runs it";
    let hint = "move the code both need into a function without `#[test]`";
    rejects("#[test]\nfn t() {}\nfn main() { t(); }", msg, hint, "t");
    rejects(
        "#[test]\nfn t() {}\n#[test]\nfn u() { t(); }\nfn main() {}",
        msg,
        hint,
        "t",
    );
    rejects("#[test]\nfn t() {}\nfn main() { let f = t; f(); }", msg, hint, "t");
    let files = [
        ("app", "mod web;\nfn main() { web::t(); }"),
        ("web", "#[test]\npub fn t() {}"),
    ];
    let d = check_files(&files, &common::caps()).expect_err("calls a test");
    assert_eq!(d[0].msg, "`web::t` is a test; only `kio test` runs it");
}

#[test]
fn assert_names_are_reserved() {
    rejects(
        "fn assert(c: Bool) {}\nfn main() {}",
        "`assert` is built in",
        "choose another name",
        "fn assert(c: Bool) {}",
    );
    rejects(
        "fn main() {}\nfn assert_eq() {}",
        "`assert_eq` is built in",
        "choose another name",
        "fn assert_eq() {}",
    );
    let files = [("app", "mod web;\nfn main() {}"), ("web", "fn assert() {}")];
    let d = check_files(&files, &common::caps()).expect_err("reserved");
    assert_eq!(d[0].msg, "`assert` is built in");
}

#[test]
fn assert_types() {
    ok("fn main() { assert(1 < 2, \"m\"); assert_eq([1], [1]); assert_eq(\"a\", \"a\"); }");
    // The side without an expected type gives the other its type.
    ok("fn main() { let xs: List<Int> = []; assert_eq(xs, []); assert_eq([], xs); assert_eq(None, Some(1)); }");
    ok("fn same<T: Eq>(a: T, b: T) { assert_eq(a, b); }\nfn main() { same(1, 1); }");
    rejects("fn main() { assert(1, \"m\"); }", "expected Bool, found Int", "", "1");
    rejects("fn main() { assert(true, 1); }", "expected Str, found Int", "", "1");
    rejects(
        "fn main() { assert(true); }",
        "`assert` takes 2 arguments, found 1",
        "`assert` is declared as `fn assert(cond: Bool, msg: Str)`",
        "assert(true)",
    );
    rejects(
        "fn main() { assert_eq(1, 2, 3); }",
        "`assert_eq` takes 2 arguments, found 3",
        "`assert_eq` is declared as `fn assert_eq<T: Eq>(a: T, b: T)`",
        "assert_eq(1, 2, 3)",
    );
    rejects(
        "fn main() { assert_eq(1, \"a\"); }",
        "`assert_eq` cannot be applied to Int and Str",
        "`==` works on two values of one type that holds no capability",
        "assert_eq(1, \"a\")",
    );
    rejects(
        "fn f() {}\nfn main() { assert_eq(f, f); }",
        "cannot compare `fn()` values",
        "functions have no equality",
        "assert_eq(f, f)",
    );
    rejects(
        "fn same<T>(a: T, b: T) { assert_eq(a, b); }\nfn main() {}",
        "cannot compare `T` values",
        "add the bound: `T: Eq`",
        "assert_eq(a, b)",
    );
    rejects(
        "fn main() { let f = assert; }",
        "`assert` is built in and can only be called",
        "write `assert(..)`",
        "assert",
    );
}

#[test]
fn a_local_hides_assert() {
    let src = "fn main() -> Int { let assert = |n: Int| n + 1; assert(1) }";
    assert_eq!(exec(src, "main"), Ok(Value::Int(2)));
}

#[test]
fn assert_failures_stop_the_run_at_the_call() {
    assert_eq!(
        exec("fn main() { assert(1 < 2, \"fine\"); assert_eq(2, 2); }", "main"),
        Ok(Value::Unit)
    );
    fails(
        "fn main() { let n = 3;\n  assert(n == 2, \"n is \" + n.to_str()); }",
        "main",
        "assertion failed: n is 3",
        "assert(n == 2, \"n is \" + n.to_str())",
    );
    fails(
        "fn main() { assert_eq([1, 2], [1, 3]); }",
        "main",
        "assert_eq failed: left `[1, 2]`, right `[1, 3]`",
        "assert_eq([1, 2], [1, 3])",
    );
    fails(
        "struct P { x: Int, s: Str }\nfn main() { assert_eq(P { x: 1, s: \"a\" }, P { x: 1, s: \"b\" }); }",
        "main",
        "assert_eq failed: left `P { x: 1, s: \"a\" }`, right `P { x: 1, s: \"b\" }`",
        "assert_eq(P { x: 1, s: \"a\" }, P { x: 1, s: \"b\" })",
    );
    fails(
        "fn main() { let x: Option<Str> = None; assert_eq(x, Some(\"a\")); }",
        "main",
        "assert_eq failed: left `None`, right `Some(\"a\")`",
        "assert_eq(x, Some(\"a\"))",
    );
    // Both arguments run first, as in any call: an error in the message stops the run there.
    fails(
        "fn main() { let xs = [1]; assert(true, xs[2].to_str()); }",
        "main",
        "index 2 out of bounds for length 1",
        "xs[2]",
    );
}

#[test]
fn run_entry_runs_a_test() {
    let src = "fn add(a: Int, b: Int) -> Int { a + b }
#[test]
fn adds() { assert_eq(add(1, 2), 3); }
#[test]
fn adds_wrong() { assert_eq(add(1, 2), 4); }
#[test]
fn errs() -> Result<(), Str> { Err(\"no\") }
#[test]
fn reads(d: Dir) {}
fn main() -> Int { add(1, 1) }";
    assert_eq!(exec(src, "main"), Ok(Value::Int(2)));
    assert_eq!(exec(src, "adds"), Ok(Value::Unit));
    fails(
        src,
        "adds_wrong",
        "assert_eq failed: left `3`, right `4`",
        "assert_eq(add(1, 2), 4)",
    );
    let p = ok(src);
    let err = common::big_stack(move || {
        let v = run_entry(&p, &common::caps(), "errs", &mut NoCaps).unwrap();
        let m = lower_entry(&p, &common::caps(), "errs");
        lang::eval::debug(&m, &v)
    });
    assert_eq!(err, "Err(\"no\")");
    fails(src, "reads", "no grant for `d: Dir`", "d: Dir");
}

#[test]
fn tests_are_lowered_only_as_the_entry() {
    let p = ok("#[test]\nfn t() { assert(true, \"m\"); }\nfn main() {}");
    let names = |m: &lang::ir::Module| m.funcs.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    let m = lower(&p, &common::caps());
    assert_eq!(names(&m), ["main"]);
    let t = lower_entry(&p, &common::caps(), "t");
    assert_eq!(names(&t), ["main", "t"]);
    assert_eq!(t.funcs[t.main.0 as usize].name, "t");
    assert_eq!(common::big_stack(move || run(&p, &mut NoCaps)), Ok(Value::Unit));
}

#[test]
fn report_leaves_tests_out() {
    let base = r#"fn load(d: Dir) -> Result<Bytes, IoErr> { d.read("a") }
fn main(input: Dir) -> Result<(), IoErr> { let _b = load(input)?; Ok(()) }"#;
    let tests = r#"
#[test]
fn reads(d: Dir, out: Dir) -> Result<(), IoErr> { let _b = load(d)?; out.write("x", "y".to_bytes()) }
#[test]
fn pure() { assert_eq(1, 1); }"#;
    let with = format!("{base}{tests}");
    assert_eq!(report(&ok(&with)), report(&ok(base)));
    let caps = common::caps();
    let p = |src: &str| check_files(&[("app", src)], &caps).expect("checks");
    assert_eq!(lang::report_diff(&p(base), &p(&with)), lang::AuthorityDiff::default());
    assert_eq!(lang::report_diff(&p(&with), &p(base)), lang::AuthorityDiff::default());
}
