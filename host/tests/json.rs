//! D2: `kio run --json` and `kio check --json` print one JSON object on stdout, with every field,
//! an error `kind` per outcome, and nothing else on stdout or stderr. Exit codes are unchanged.
use kio_host::{Kio, Limits};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// `(exit code, the one JSON line on stdout, stderr)` of `kio args..` in `cwd`.
fn kio(cwd: &Path, args: &[&str]) -> (i32, Value, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.ends_with('\n') && stdout.lines().count() == 1, "{stdout:?}");
    let v: Value = serde_json::from_str(&stdout).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "audit",
            "diff",
            "errors",
            "hidden",
            "ok",
            "report",
            "report_tests",
            "used",
            "value"
        ]
    );
    assert_eq!(v["ok"], json!(v["errors"].as_array().unwrap().is_empty()));
    let stderr = String::from_utf8(out.stderr).unwrap();
    (out.status.code().unwrap(), v, stderr)
}

/// A temp dir holding `files` as `(name, text)`.
fn dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let p = t.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    t
}

/// The only error of `v`, which must have exactly one.
fn only_error(v: &Value) -> &Value {
    let errors = v["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{v}");
    &errors[0]
}

/// An error without a span.
fn unplaced(kind: &str, message: &str, hint: Option<&str>) -> Value {
    json!({"kind": kind, "message": message, "hint": hint, "file": null, "line": null, "col": null})
}

/// `main`'s value as `--json` shows it, run with no grants.
fn value_of(src: &str) -> Value {
    let t = dir(&[("p.kio", src)]);
    let (code, v, err) = kio(t.path(), &["run", "p.kio", "--json"]);
    assert_eq!((code, err.as_str()), (0, ""), "{v}");
    v["value"].clone()
}

#[test]
fn an_ok_run_has_its_value_report_audit_and_use() {
    let t = dir(&[
        (
            "job.kio",
            r#"fn main(input: Dir) -> Result<Map<Str, Int>, Str> {
  let mut m: Map<Str, Int> = Map::new();
  match input.read("a.txt") { Ok(b) => m.insert(b.to_str(), 1), Err(_) => return Err("no file") }
  Ok(m)
}
"#,
        ),
        ("in/a.txt", "alice"),
    ]);
    let (code, v, err) = kio(
        t.path(),
        &["run", "job.kio", "--dir", "input=in", "--audit", "a.log", "--json"],
    );
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(v["value"], json!(r#"{"alice": 1}"#));
    assert_eq!(v["report"], json!(["main : input read"]));
    assert_eq!((&v["audit"], &v["diff"]), (&json!("a.log"), &Value::Null));
    let used = &v["used"];
    assert_eq!((&used["calls"], &used["io"]), (&json!(1), &json!(5)));
    assert!(used["fuel"].as_u64().unwrap() > 0 && used["peak_memory"].as_u64().unwrap() > 0);
    assert!(used["ms"].is_f64());
    assert!(std::fs::read_to_string(t.path().join("a.log"))
        .unwrap()
        .contains(r#""outcome":"ok""#));
}

#[test]
fn a_result_shows_its_ok_payload_and_a_str_unquoted() {
    // `j.to_json()` returned in `Ok` parses as the JSON it is.
    let v =
        value_of(r#"fn main() -> Result<Str, Str> { Ok(Json::Arr([Json::Str("a\"b\n"), Json::Num(1)]).to_json()) }"#);
    let parsed: Value = serde_json::from_str(v.as_str().unwrap()).unwrap();
    assert_eq!(parsed, json!(["a\"b\n", 1]));
    assert_eq!(
        value_of(r#"fn main() -> Result<Str, Str> { Ok("tab\t é\0") }"#),
        json!("tab\t é\0")
    );
    // A control character the runtime prints as `\u{7}`.
    let bell = r#"fn main() -> Result<Str, Str> { Ok("\"\\u0007x\"".parse_json()?.as_str().unwrap_or("")) }"#;
    assert_eq!(value_of(bell), json!("\u{7}x"));
    assert_eq!(value_of(r#"fn main() -> Result<Int, Str> { Ok(3) }"#), json!("3"));
    assert_eq!(
        value_of(r#"fn main() -> Result<List<Str>, Str> { Ok(["a"]) }"#),
        json!(r#"["a"]"#)
    );
    assert_eq!(value_of(r#"fn main() -> Result<(), Str> { Ok(()) }"#), Value::Null);
    assert_eq!(value_of("fn main() {}"), Value::Null);
    // Other types keep their printed text; a `Str` that reads `()` is still a value.
    assert_eq!(value_of(r#"fn main() -> Str { "()" }"#), json!("()"));
    assert_eq!(
        value_of(r#"fn main() -> Option<Str> { Some("x") }"#),
        json!(r#"Some("x")"#)
    );
    assert_eq!(
        value_of(r#"fn main() -> Bytes { "ab".to_bytes() }"#),
        json!("<2 bytes>")
    );
}

#[test]
fn main_returning_err_is_kind_err() {
    let t = dir(&[("p.kio", r#"fn main() -> Result<Int, Str> { Err("bad \"input\"") }"#)]);
    let (code, v, err) = kio(t.path(), &["run", "p.kio", "--json"]);
    assert_eq!((code, err.as_str(), &v["value"]), (1, "", &Value::Null));
    assert_eq!(only_error(&v), &unplaced("err", "bad \"input\"", None));
    assert!(v["used"].is_object());
}

#[test]
fn check_lists_every_diagnostic_with_its_place() {
    let src = "fn a() -> Int { \"s\" }\nfn b() -> Str { 1 }\nfn main() -> Int { z }\n";
    let t = dir(&[("bad.kio", src)]);
    for args in [&["check", "--json", "bad.kio"][..], &["run", "bad.kio", "--json"]] {
        let (code, v, err) = kio(t.path(), args);
        assert_eq!((code, err.as_str()), (1, ""));
        let at: Vec<_> = (v["errors"].as_array().unwrap().iter())
            .map(|e| {
                (
                    e["kind"].clone(),
                    e["file"].clone(),
                    e["line"].clone(),
                    e["col"].clone(),
                )
            })
            .collect();
        assert_eq!(at.len(), 3, "{v}");
        assert!(at.iter().all(|(k, f, ..)| k == "check" && f == "bad.kio"));
        assert_eq!((&at[2].2, &at[2].3), (&json!(3), &json!(20)));
        assert_eq!(v["errors"][2]["hint"], json!("declare it with `let`"));
        assert_eq!((&v["report"], &v["used"]), (&json!([]), &Value::Null));
    }
}

#[test]
fn a_module_error_names_the_module_path_and_counts_characters() {
    let t = dir(&[
        ("app/main.kio", "mod util;\nfn main() -> Int { util::f() }\n"),
        ("app/util.kio", "pub fn f() -> Int {\n  let s = \"éé\"; zz\n}\n"),
    ]);
    let (code, v, _) = kio(t.path(), &["check", "app/main.kio", "--json"]);
    assert_eq!(code, 1);
    let e = only_error(&v);
    assert_eq!(
        (&e["file"], &e["line"], &e["col"]),
        (&json!("app/util.kio"), &json!(2), &json!(17))
    );
}

#[test]
fn a_clean_check_has_the_report() {
    let t = dir(&[("p.kio", "fn main(d: Dir) -> Result<Bytes, IoErr> { d.read(\"x\") }\n")]);
    let (code, v, err) = kio(t.path(), &["check", "p.kio", "--json"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        (&v["report"], &v["value"], &v["used"]),
        (&json!(["main : d read"]), &Value::Null, &Value::Null)
    );
}

#[test]
fn a_check_reports_test_lines_and_hidden_counts_apart() {
    // P8: `report` is `main`'s section only; no blank line, heading or footer in it.
    let app = "mod tests;\nfn pure(n: Int) -> Int { n }\nfn wipe(d: Dir) { let _r = d.remove(\"a\"); }\nfn main(d: Dir) -> Int { let _r = d.read(\"x\"); pure(1) }\n";
    let tests = "fn peek(d: Dir) { let _r = d.list(\"\"); }\n#[test]\nfn t(d: Dir) { peek(d); }\n";
    let t = dir(&[("p.kio", app), ("tests.kio", tests)]);
    let (code, v, err) = kio(t.path(), &["check", "p.kio", "--json"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(v["report"], json!(["main        : d read"]));
    assert_eq!(v["report_tests"], json!(["tests::peek : d list"]));
    let hidden = json!({"no_capability": 1, "unreached": 1, "unreached_with_capability": 1});
    assert_eq!(v["hidden"], hidden);
    // `--all`: every function in `report`, nothing hidden.
    let (_, v, _) = kio(t.path(), &["check", "p.kio", "--all", "--json"]);
    assert_eq!(v["report"].as_array().map(Vec::len), Some(4));
    assert_eq!((&v["report_tests"], &v["hidden"]), (&json!([]), &Value::Null));
}

#[test]
fn check_base_has_the_diff_and_keeps_exit_3() {
    let t = dir(&[
        ("old.kio", "fn main() -> Int { 1 }\n"),
        (
            "new.kio",
            "fn main(out: Dir) -> Result<(), IoErr> { out.write(\"a\", \"x\".to_bytes()) }\n",
        ),
    ]);
    let (code, v, err) = kio(t.path(), &["check", "new.kio", "--base", "old.kio", "--json"]);
    assert_eq!((code, err.as_str()), (3, ""));
    let diff = json!({"grants": ["+ out: Dir (write)"], "funcs": ["+ main: out write"], "added": true});
    assert_eq!((&v["ok"], &v["diff"]), (&json!(true), &diff));
    assert_eq!(v["report"], json!(["main : out write"]));
    let (code, v, _) = kio(
        t.path(),
        &["check", "old.kio", "--base", "old.kio", "--detail", "--json"],
    );
    assert_eq!(
        (code, &v["diff"]),
        (0, &json!({"grants": [], "funcs": [], "added": false}))
    );
    // The base fails to check: its diagnostics, no diff.
    std::fs::write(t.path().join("old.kio"), "fn main() -> Int { x }\n").unwrap();
    let (code, v, err) = kio(t.path(), &["check", "new.kio", "--base", "old.kio", "--json"]);
    assert_eq!((code, err.as_str(), &v["diff"]), (1, "", &Value::Null));
    assert_eq!(
        (&only_error(&v)["kind"], &only_error(&v)["file"]),
        (&json!("check"), &json!("old.kio"))
    );
}

const WRITER: &str = "fn main(out: Dir) -> Result<(), IoErr> { out.write(\"a\", \"x\".to_bytes()) }\n";

#[test]
fn startup_refusals_are_kind_grant_exit_2() {
    let t = dir(&[
        ("w.kio", WRITER),
        ("put.kio", "fn main(api: Http) -> Result<HttpResponse, HttpErr> { api.send(\"PUT\", \"/x\", [], \"a\".to_bytes(), 0) }\n"),
        ("app/self.kio", WRITER),
    ]);
    std::fs::create_dir(t.path().join("out")).unwrap();
    let cases: [(&[&str], &str, &str); 3] = [
        (
            &["run", "w.kio", "--dir", "out=out:ro", "--json"],
            "`out` is granted `:ro` but the program can write it (`out write`)",
            "drop `:ro`, or remove the write",
        ),
        (
            &["run", "put.kio", "--http", "api=http://127.0.0.1:9", "--json"],
            "`api` sends PUT but `--http api` grants only GET",
            "add `--http-methods api=PUT`, or remove the call",
        ),
        (
            &["run", "app/self.kio", "--dir", "out=app", "--json"],
            "`--dir out=app` contains the program's own source (`app/self.kio`); it could rewrite itself for the next run",
            "keep the program outside every granted directory",
        ),
    ];
    for (args, message, hint) in cases {
        let (code, v, err) = kio(t.path(), args);
        assert_eq!((code, err.as_str()), (2, ""), "{args:?}");
        assert_eq!(only_error(&v), &unplaced("grant", message, Some(hint)), "{args:?}");
        assert_eq!((&v["used"], &v["value"]), (&Value::Null, &Value::Null));
    }
}

#[test]
fn a_grant_that_does_not_bind_is_kind_grant_exit_1() {
    let t = dir(&[("w.kio", WRITER), ("g.grants", "# nothing granted\n")]);
    for args in [
        &["run", "w.kio", "--json"][..],
        &["check", "w.kio", "--grants", "g.grants", "--json"],
    ] {
        let (code, v, err) = kio(t.path(), args);
        assert_eq!((code, err.as_str()), (1, ""), "{args:?}");
        let errors = v["errors"].as_array().unwrap();
        assert!(
            errors
                .iter()
                .all(|e| e["kind"] == "grant" && e["file"] == "w.kio" && e["line"] == 1),
            "{v}"
        );
        assert!(errors.iter().any(|e| e["message"] == "no grant for `out: Dir`"), "{v}");
        assert_eq!((&v["report"], &v["used"]), (&json!(["main : out write"]), &Value::Null));
    }
}

#[test]
fn a_runtime_error_is_placed() {
    let t = dir(&[("p.kio", "fn main() -> Int {\n  let xs = [1];\n  xs[5]\n}\n")]);
    let (code, v, err) = kio(t.path(), &["run", "p.kio", "--json"]);
    assert_eq!((code, err.as_str()), (1, ""));
    let e = only_error(&v);
    assert_eq!(
        (&e["kind"], &e["message"]),
        (&json!("runtime"), &json!("index 5 out of bounds for length 1"))
    );
    assert_eq!((&e["file"], &e["line"]), (&json!("p.kio"), &json!(3)));
    assert!(v["used"].is_object());
}

const SPIN: &str = "fn main() -> Int { let mut i = 0; while true { i = i + 1; } i }\n";

#[test]
fn each_limit_is_its_own_kind_with_a_hint() {
    let t = dir(&[
        ("spin.kio", SPIN),
        (
            "grow.kio",
            "fn main() -> Int { let mut xs: List<Str> = []; while true { xs.push(\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"); } 0 }\n",
        ),
        ("tick.kio", "fn main(c: Clock) -> Int { let mut i = 0; while true { c.now_ms(); i = i + 1; } i }\n"),
        ("deep.kio", "fn f(n: Int) -> Int { if n == 0 { 0 } else { 1 + f(n - 1) } }\nfn main() -> Int { f(10000000) }\n"),
        (
            "big.kio",
            "fn main(out: Dir) -> Result<(), IoErr> { let mut s = \"x\"; let mut i = 0; while i < 21 { s = s + s; i = i + 1; } out.write(\"a\", s.to_bytes()) }\n",
        ),
    ]);
    std::fs::create_dir(t.path().join("out")).unwrap();
    let cases: [(&[&str], &str, &str, &str); 6] = [
        (
            &["run", "spin.kio", "--fuel", "100000000", "--json"],
            "fuel",
            "out of fuel (limit 100000000)",
            "--fuel",
        ),
        (
            &["run", "grow.kio", "--memory", "32", "--json"],
            "memory",
            "out of memory (limit 32 MB)",
            "--memory",
        ),
        (
            &[
                "run",
                "spin.kio",
                "--fuel",
                "1000000000000000",
                "--timeout",
                "1",
                "--json",
            ],
            "timeout",
            "timed out after 1 s",
            "--timeout",
        ),
        (
            &["run", "tick.kio", "--clock", "c", "--calls", "5", "--json"],
            "calls",
            "too many host calls (limit 5)",
            "--calls",
        ),
        (
            &["run", "deep.kio", "--stack", "64", "--json"],
            "stack",
            "stack overflow (limit 64 KB)",
            "--stack",
        ),
        (
            &["run", "big.kio", "--dir", "out=out", "--io", "1", "--json"],
            "io",
            "too much I/O (limit 1 MB)",
            "--io",
        ),
    ];
    for (args, kind, message, flag) in cases {
        let (code, v, err) = kio(t.path(), args);
        assert_eq!((code, err.as_str()), (1, ""), "{args:?}");
        let hint = format!("raise it with {flag}");
        assert_eq!(only_error(&v), &unplaced(kind, message, Some(&hint)), "{args:?}");
        assert!(v["used"].is_object(), "{args:?}");
    }
}

/// Fuel that ran out while the instance started used to escape the limit message (D2); the
/// limit message holds wherever fuel runs out.
#[test]
fn fuel_spent_at_instantiation_is_kind_fuel() {
    let t = dir(&[("spin.kio", SPIN)]);
    let (code, v, _) = kio(t.path(), &["run", "spin.kio", "--fuel", "1000", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(
        only_error(&v),
        &unplaced("fuel", "out of fuel (limit 1000)", Some("raise it with --fuel"))
    );
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .current_dir(t.path())
        .args(["run", "spin.kio", "--fuel", "1000"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "out of fuel (limit 1000; raise it with --fuel)\n"
    );
}

/// Fuel counts the program's work, not Wasmtime's memory initialization, which is free on Linux
/// and a page-padded copy elsewhere: an empty program used 6,906 on Linux and 88,827 on macOS.
#[test]
fn fuel_is_the_same_on_every_os() {
    let t = dir(&[("empty.kio", "fn main() -> Int { 0 }\n")]);
    let (code, v, _) = kio(t.path(), &["run", "empty.kio", "--json"]);
    assert_eq!(code, 0);
    let fuel = v["used"]["fuel"].as_u64().unwrap();
    assert!(fuel < 20_000, "{fuel}");
}

/// A call the host stops is `denied`: here one the report lacks (test hook; the CLI never
/// produces one unless the checker is wrong).
#[test]
fn a_call_the_host_stops_is_kind_denied() {
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.grant_sources(kio_host::Sources {
        clocks: vec![("c".into(), kio_host::ClockSpec::parse(None).unwrap())],
        ..Default::default()
    });
    kio.hide_from_report("c", "now_ms");
    let e = kio.run("fn main(c: Clock) -> Int { c.now_ms() }", vec![]).unwrap_err();
    assert_eq!(kio_host::stop_kind(&kio.explain(&e).unwrap()), "denied");
    assert!(kio.used().is_some());
}

#[test]
fn usage_errors_are_one_json_line_too() {
    let t = dir(&[("p.kio", "fn main() -> Int { 1 }\n"), ("g.grants", "--json\n")]);
    let cases: [(&[&str], &str, Option<&str>); 4] = [
        (
            &["run", "p.kio", "--fuel", "0", "--json"],
            "--fuel 0: expected a positive integer",
            None,
        ),
        (
            &["run", "p.kio", "--json", "--listen", "127.0.0.1:0"],
            "`--json` reports one run; `--listen` serves many",
            Some("drop --json, or drop --listen"),
        ),
        (
            &["mcp", "--json"],
            "`--json` reports one run or check; `kio mcp` serves many",
            Some("drop --json"),
        ),
        (&["run", "missing.kio", "--json"], "", None),
    ];
    for (args, message, hint) in cases {
        let (code, v, err) = kio(t.path(), args);
        assert_eq!((code, err.as_str()), (2, ""), "{args:?}");
        let e = only_error(&v);
        assert_eq!(e["kind"], "usage", "{args:?}");
        if !message.is_empty() {
            assert_eq!(e, &unplaced("usage", message, hint), "{args:?}");
        }
        assert_eq!((&v["report"], &v["used"]), (&json!([]), &Value::Null));
    }
    // `--json` is not a grant: a grants file cannot hold it.
    let (code, v, _) = kio(t.path(), &["run", "p.kio", "--grants", "g.grants", "--json"]);
    assert_eq!(code, 2);
    assert!(
        only_error(&v)["message"]
            .as_str()
            .unwrap()
            .contains("unknown flag `--json`"),
        "{v}"
    );
}

#[test]
fn json_anywhere_after_the_command_and_log_lines_stay_on_stderr() {
    let t = dir(&[(
        "p.kio",
        "fn main(l: Log) -> Result<Int, LogErr> { l.write(\"hello\")?; Ok(7) }\n",
    )]);
    for args in [
        &["run", "--json", "p.kio", "--log", "l=-"][..],
        &["run", "p.kio", "--log", "l=-", "--json"],
    ] {
        let (code, v, err) = kio(t.path(), args);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["value"], json!("7"));
        assert!(err.contains("hello") && !err.contains("main :"), "{err:?}");
    }
}
