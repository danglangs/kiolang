//! T1b: `kio test` runs every `#[test]` function compiled, each in a fresh instance with the
//! grants file's grants or the defaults, and reports in text or `--json`.
use std::path::Path;
use std::process::Command;

/// `(exit code, stdout, stderr)` of `kio args..`, with `tmp` as the system's temporary directory.
fn kio_in(tmp: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(args)
        .env("TMPDIR", tmp)
        .output()
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

/// A directory for one case, with `app.kio` holding `src`, and its own temporary directory.
struct Case {
    dir: tempfile::TempDir,
    app: String,
}

impl Case {
    fn new(src: &str) -> Case {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tmp")).unwrap();
        let app = write(dir.path(), "app.kio", src);
        Case { dir, app }
    }

    fn kio(&self, args: &[&str]) -> (i32, String, String) {
        let mut all = vec!["test", self.app.as_str()];
        all.extend(args);
        kio_in(&self.dir.path().join("tmp"), &all)
    }

    /// `--json`'s object.
    fn json(&self, args: &[&str]) -> (i32, serde_json::Value) {
        let mut all = vec!["--json"];
        all.extend(args);
        let (code, out, err) = self.kio(&all);
        assert_eq!(out.lines().count(), 1, "{out}{err}");
        (code, serde_json::from_str(&out).unwrap())
    }

    fn file(&self, name: &str, text: &str) -> String {
        write(self.dir.path(), name, text)
    }
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

const MIXED: &str = r#"fn add(a: Int, b: Int) -> Int { a + b }

#[test]
fn adds() { assert_eq(add(1, 2), 3); }

#[test]
fn adds_wrong() { assert_eq(add(1, 2), 4); }

#[test]
fn asserts() { assert(add(1, 1) == 3, "one and one"); }

#[test]
fn errs() -> Result<(), Str> { Err("no") }

#[test]
fn indexes() { let xs = [1]; let _ = xs[3]; }

#[test]
fn loops() { let mut i = 0; loop { i += 1; } }

fn main() -> Int { add(1, 1) }
"#;

#[test]
fn passing_and_failing_tests_in_text() {
    let c = Case::new(MIXED);
    let (code, out, err) = c.kio(&["--fuel", "1000000"]);
    assert_eq!(code, 1, "{err}");
    let app = &c.app;
    let want = format!(
        "ok   adds
FAIL adds_wrong
  {app}:7:19: assert_eq failed: left `3`, right `4`
FAIL asserts
  {app}:10:16: assertion failed: one and one
FAIL errs
  Err(\"no\")
FAIL indexes
  {app}:16:38: index 3 out of bounds for length 1
FAIL loops
  out of fuel (limit 1000000)
  hint: raise it with --fuel
1 passed, 5 failed
"
    );
    assert_eq!(out, want);
}

#[test]
fn json_results() {
    let c = Case::new(MIXED);
    let (code, j) = c.json(&["--fuel", "1000000"]);
    assert_eq!(code, 1);
    assert_eq!(
        (j["ok"].as_bool(), j["passed"].as_u64(), j["failed"].as_u64()),
        (Some(false), Some(1), Some(5))
    );
    let keys: Vec<_> = j.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["failed", "ok", "passed", "tests"]);
    let tests = j["tests"].as_array().unwrap();
    let names: Vec<_> = tests.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["adds", "adds_wrong", "asserts", "errs", "indexes", "loops"]);
    let t = &tests[0];
    assert_eq!(
        (t["ok"].as_bool(), t["error"].is_null(), t["log"].as_str()),
        (Some(true), true, Some(""))
    );
    assert!(t["ms"].as_f64().unwrap() > 0.0);
    let e = &tests[1]["error"];
    assert_eq!(e["kind"], "runtime");
    assert_eq!(e["message"], "assert_eq failed: left `3`, right `4`");
    assert_eq!(
        (e["file"].as_str(), e["line"].as_u64(), e["col"].as_u64()),
        (Some(c.app.as_str()), Some(7), Some(19))
    );
    assert!(e["hint"].is_null());
    assert_eq!(
        (
            tests[3]["error"]["kind"].as_str(),
            tests[3]["error"]["message"].as_str()
        ),
        (Some("err"), Some("Err(\"no\")"))
    );
    assert_eq!(tests[5]["error"]["kind"], "fuel");
}

#[test]
fn filter_and_exit_codes() {
    let c = Case::new(MIXED);
    let (code, out, err) = c.kio(&["--filter", "adds"]);
    assert_eq!(code, 1, "{err}");
    assert!(out.starts_with("ok   adds\nFAIL adds_wrong\n"), "{out}");
    assert!(out.ends_with("1 passed, 1 failed\n"), "{out}");
    let pass = Case::new("#[test]\nfn a() {}\n#[test]\nfn b() -> Result<(), Str> { Ok(()) }\n");
    // No `main`: `kio test` does not need one.
    let (code, out, err) = pass.kio(&[]);
    assert_eq!(
        (code, out.as_str()),
        (0, "ok   a\nok   b\n2 passed, 0 failed\n"),
        "{err}"
    );
    // A filter that matches nothing is a usage error; a file with no tests passes.
    let (code, out, err) = pass.kio(&["--filter", "zzz"]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("no test matches `zzz`"), "{err}");
    let (code, j) = pass.json(&["--filter", "zzz"]);
    assert_eq!((code, j["errors"][0]["kind"].as_str()), (2, Some("usage")));
    let none = Case::new("fn main() {}\n");
    let (code, out, _) = none.kio(&[]);
    assert_eq!((code, out.as_str()), (0, "0 passed, 0 failed\n"));
    // A bad limit is a usage error before any test runs.
    for bad in [["--fuel", "abc"], ["--memory", "0"], ["--stack", "99999999"]] {
        let (code, out, err) = pass.kio(&bad);
        assert_eq!((code, out.as_str()), (2, ""), "{bad:?}");
        assert!(err.contains(bad[0]), "{err}");
    }
    let g = pass.file("bad.grants", "--timeout x\n");
    let (code, _, err) = pass.kio(&["--grants", &g]);
    assert_eq!(code, 2, "{err}");
    // A check error, a usage error and a grant flag on the command line: exit 2.
    let bad = Case::new("#[test]\nfn a() { assert(1, \"m\"); }\n");
    let (code, out, err) = bad.kio(&[]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("expected Bool, found Int"), "{err}");
    let (code, j) = bad.json(&[]);
    assert_eq!(
        (code, j["ok"].as_bool(), j["errors"][0]["kind"].as_str()),
        (2, Some(false), Some("check"))
    );
    let (code, _, err) = pass.kio(&["--bogus", "1"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = pass.kio(&["--dir", "d=."]);
    assert_eq!(code, 2);
    assert!(err.contains("`--dir` is not a flag of kio test"), "{err}");
    // `kio run` and `kio check` still need `main`.
    let (code, _, err) = kio_in(pass.dir.path(), &["check", &pass.app]);
    assert_eq!(code, 1);
    assert!(err.contains("no `main` function"), "{err}");
}

#[test]
fn default_grants() {
    let src = r#"
#[test]
fn dir(d: Dir) -> Result<(), IoErr> {
  assert_eq(d.list("")?, []);
  d.write("a", "x".to_bytes())?;
  assert_eq(d.read("a")?.to_str(), "x");
  Ok(())
}

#[test]
fn dir_again(d: Dir) -> Result<(), IoErr> { assert_eq(d.list("")?, []); Ok(()) }

#[test]
fn stores(k: Kv, db: Db) -> Result<(), Str> {
  k.set("a", "1".to_bytes()).map_err(|e| "kv")?;
  assert_eq(k.keys("")
    .map_err(|e| "kv")?, ["a"]);
  db.exec("CREATE TABLE t (x INTEGER)", []).map_err(|e| "db")?;
  Ok(())
}

#[test]
fn clock(c: Clock) { assert_eq(c.now_ms(), 1767225600000); }

#[test]
fn rng(r: Rng) { assert_eq(r.int(0, 1000000), 0); }

#[test]
fn logs(log: Log) { let _ = log.write("one"); let _ = log.write("two"); assert(false, "after the log"); }

#[test]
fn quiet(log: Log) { let _ = log.write("not shown"); }
"#;
    let c = Case::new(src);
    let (_, j) = c.json(&[]);
    let tests = j["tests"].as_array().unwrap();
    let by = |n: &str| tests.iter().find(|t| t["name"] == n).unwrap();
    for n in ["dir", "dir_again", "stores", "clock", "quiet"] {
        assert!(by(n)["ok"].as_bool().unwrap(), "{n}: {}", by(n));
    }
    // Seed 1, as `kio run --rng r=seed:1` gives.
    let msg = by("rng")["error"]["message"].as_str().unwrap();
    let run = Case::new("fn main(r: Rng) -> Int { r.int(0, 1000000) }");
    let (_, out, _) = kio_in(run.dir.path(), &["run", &run.app, "--rng", "r=seed:1"]);
    assert_eq!(msg, format!("assert_eq failed: left `{}`, right `0`", out.trim()));
    assert_eq!(by("logs")["log"], "one\ntwo\n");
    assert_eq!(by("quiet")["log"], "not shown\n");
    let (_, out, _) = c.kio(&["--filter", "logs"]);
    assert!(out.contains("  log:\n    one\n    two\n"), "{out}");
    let (_, out, _) = c.kio(&["--filter", "quiet"]);
    assert_eq!(out, "ok   quiet\n1 passed, 0 failed\n");
    // Every temporary directory is gone.
    let left: Vec<_> = std::fs::read_dir(c.dir.path().join("tmp")).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn grants_file_overrides_a_default() {
    let c = Case::new(
        r#"#[test]
fn reads(fx: Dir) -> Result<(), IoErr> { assert_eq(fx.read("in.txt")?.to_str(), "hello"); Ok(()) }

#[test]
fn writes(fx: Dir) -> Result<(), IoErr> { fx.write("x", "y".to_bytes()) }

#[test]
fn fresh(d: Dir) -> Result<(), IoErr> { assert_eq(d.list("")?, []); Ok(()) }
"#,
    );
    let fx = c.dir.path().join("fixtures");
    std::fs::create_dir(&fx).unwrap();
    std::fs::write(fx.join("in.txt"), "hello").unwrap();
    // `--clock other` names no parameter of these tests: left out, not an error.
    let g = c.file(
        "t.grants",
        &format!("--dir fx={}:ro\n--clock other=fixed:5\n", fx.display()),
    );
    let (code, j) = c.json(&["--grants", &g]);
    assert_eq!(code, 1);
    let tests = j["tests"].as_array().unwrap();
    assert!(tests[0]["ok"].as_bool().unwrap(), "{}", tests[0]);
    assert!(tests[1]["ok"].as_bool().unwrap(), "{}", tests[1]);
    let e = &tests[2]["error"];
    assert_eq!(e["kind"], "grant");
    assert_eq!(
        e["message"],
        "`fx` is granted `:ro` but the program can write it (`fx write`)"
    );
    assert_eq!(std::fs::read_dir(&fx).unwrap().count(), 1);
}

#[test]
fn grants_file_checks_still_apply() {
    let c = Case::new("#[test]\nfn t(d: Dir) {}\n");
    // A writable dir holding the program's source (H28).
    let g = c.file("src.grants", &format!("--dir d={}\n", c.dir.path().display()));
    let (code, j) = c.json(&["--grants", &g]);
    assert_eq!(code, 1);
    let msg = j["tests"][0]["error"]["message"].as_str().unwrap();
    assert!(msg.contains("contains the program's own source"), "{msg}");
    // The grants file inside a granted dir.
    let inner = c.dir.path().join("data");
    std::fs::create_dir(&inner).unwrap();
    let g = write(&inner, "in.grants", &format!("--dir d={}\n", inner.display()));
    let (_, j) = c.json(&["--grants", &g]);
    let msg = j["tests"][0]["error"]["message"].as_str().unwrap();
    assert!(msg.contains("is inside `--dir d="), "{msg}");
    // Server grants are not for tests.
    let g = c.file("srv.grants", "--listen 127.0.0.1:0\n");
    let (code, _, err) = c.kio(&["--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("`--listen` is not for kio test"), "{err}");
}

#[test]
fn http_and_hmac_only_from_the_grants_file() {
    let c = Case::new(
        "#[test]\nfn t(api: Http) {}\n#[test]\nfn h(key: Hmac) {}\n#[test]\nfn s(req: Incoming, res: Outgoing) {}\n",
    );
    let (code, j) = c.json(&[]);
    assert_eq!(code, 1);
    let e = |i: usize| &j["tests"][i]["error"];
    assert_eq!(
        (e(0)["message"].as_str(), e(0)["hint"].as_str()),
        (Some("test `h` needs `key: Hmac`"), Some("name it in a --grants file"))
    );
    assert_eq!(e(1)["message"], "test `s` needs `req: Incoming`");
    assert_eq!(
        (e(2)["message"].as_str(), e(2)["kind"].as_str()),
        (Some("test `t` needs `api: Http`"), Some("grant"))
    );
    let g = c.file("t.grants", "--http api=example.com\n");
    let (_, j) = c.json(&["--grants", &g, "--filter", "t"]);
    assert!(j["tests"][0]["ok"].as_bool().unwrap(), "{j}");
}

#[test]
fn a_test_in_a_module() {
    let c = Case::new("mod util;\n#[test]\nfn root() { assert_eq(util::double(2), 4); }\n");
    c.file(
        "util.kio",
        "pub fn double(n: Int) -> Int { n * 2 }\n#[test]\nfn doubles() { assert_eq(double(3), 7); }\n",
    );
    let (code, out, err) = c.kio(&[]);
    assert_eq!(code, 1, "{err}");
    let util = c.dir.path().join("util.kio");
    let want = format!(
        "ok   root\nFAIL util::doubles\n  {}:3:16: assert_eq failed: left `6`, right `7`\n1 passed, 1 failed\n",
        util.display()
    );
    assert_eq!(out, want);
}

#[test]
fn messages_are_escaped_in_text() {
    // A message that tries to print a forged result line and clear the screen.
    let src = "#[test]\nfn forges() { let mut b = \"\".to_bytes(); b.push(27);\n  assert(false, \"x\\nok   other\\n9 passed, 0 failed\\r\" + b.to_str() + \"[2J\"); }\n";
    let c = Case::new(src);
    let (code, out, _) = c.kio(&[]);
    assert_eq!(code, 1);
    let want = format!(
        "FAIL forges\n  {}:3:3: assertion failed: x\\nok   other\\n9 passed, 0 failed\\r\\u{{1b}}[2J\n0 passed, 1 failed\n",
        c.app
    );
    assert_eq!(out, want);
    // `--json` carries the message as it is.
    let (_, j) = c.json(&[]);
    assert_eq!(
        j["tests"][0]["error"]["message"],
        "assertion failed: x\nok   other\n9 passed, 0 failed\r\u{1b}[2J"
    );
}

#[test]
fn grants_match_by_name_and_type() {
    let c = Case::new(
        r#"#[test]
fn signs(k: Hmac) { assert(k.sign("m".to_bytes()).is_ok(), "signs"); }

#[test]
fn sets(k: Kv) -> Result<(), KvErr> { k.set("a", "1".to_bytes()) }
"#,
    );
    let key = c.file("key", "secret");
    let g = c.file("t.grants", &format!("--hmac k=file:{key}\n"));
    let (code, j) = c.json(&["--grants", &g]);
    assert_eq!(code, 0, "{j}");
    assert_eq!(j["passed"], 2);
}
