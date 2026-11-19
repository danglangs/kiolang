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

/// H30: `kio run` escapes a run-time error's message, and `main`'s `Err`, as `kio test` does.
#[test]
fn run_errors_are_escaped_in_text() {
    let esc = "let mut b = \"\".to_bytes(); b.push(27);";
    let forge = "\"x\\nok: forged\\r\" + b.to_str() + \"[2J\"";
    let c = Case::new(&format!("fn main() {{ {esc}\n  assert(false, {forge}); }}\n"));
    let tmp = c.dir.path().join("tmp");
    let (code, _, err) = kio_in(&tmp, &["run", &c.app]);
    assert_eq!(code, 1);
    let want = "x\\nok: forged\\r\\u{1b}[2J";
    assert_eq!(err, format!("{}:2:3: error: assertion failed: {want}\n", c.app));
    let (_, out, _) = kio_in(&tmp, &["run", "--json", &c.app]);
    let j: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(j["errors"][0]["message"], "assertion failed: x\nok: forged\r\u{1b}[2J");

    let e = c.file(
        "e.kio",
        &format!("fn main() -> Result<(), Str> {{ {esc}\n  Err({forge}) }}\n"),
    );
    let (code, _, err) = kio_in(&tmp, &["run", &e]);
    assert_eq!(code, 1);
    assert_eq!(err, format!("error: {want}\n"));

    // A diagnostic quoting a source character escapes it too.
    let l = c.file("l.kio", "fn main() -> Int { 1 \u{1b}[2J }\n");
    let (code, _, err) = kio_in(&tmp, &["check", &l]);
    assert_eq!(code, 1);
    assert!(err.contains("unexpected character `\\u{1b}`"), "{err:?}");
}

/// L28 1 and 5: a diagnostic is `file:line:col:`, and a `\` in it is shown once: in a quoted
/// escape, in an `assert_eq` value already written as a literal, in an `assert` message. A
/// newline and ESC are still escaped (H30).
#[test]
fn backslashes_are_not_doubled() {
    let c = Case::new("fn main() -> Str {\n  \"\\u{feff}\" }\n");
    let tmp = c.dir.path().join("tmp");
    let (code, _, err) = kio_in(&tmp, &["check", &c.app]);
    assert_eq!(code, 1);
    let want = "error: unknown escape `\\u`\n  hint: escapes are \\n \\r \\t \\0 \\\" \\\\\n";
    assert_eq!(err, format!("{}:2:4: {want}", c.app));

    let src = "#[test]\nfn quotes() { assert_eq(\"x\\\"y\", \"x\\\\y\"); }\n#[test]\nfn says() { let mut b = \"\".to_bytes(); b.push(27);\n  assert(false, \"a\\\\b\\n\" + b.to_str()); }\n";
    let t = Case::new(src);
    let (code, out, _) = t.kio(&[]);
    assert_eq!(code, 1);
    let want = format!(
        "FAIL quotes\n  {0}:2:15: assert_eq failed: left `\"x\\\"y\"`, right `\"x\\\\y\"`\nFAIL says\n  {0}:5:3: assertion failed: a\\b\\n\\u{{1b}}\n0 passed, 2 failed\n",
        t.app
    );
    assert_eq!(out, want);
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

/// T3: `--json` gives each test's fuel and peak memory, `--verbose` shows them, and an example
/// test over `--listen`'s per-request fuel gets a note.
#[test]
fn fuel_and_peak_memory() {
    let c = Case::new(
        r#"#[test]
fn heavy() { let mut i = 0; while i < 500000 { i += 1; } assert_eq(i, 500000); }

#[test]
fn heavy_fails() { let mut i = 0; while i < 500000 { i += 1; } assert(false, "late"); }

#[test]
fn prop(n: Int) { assert_eq(n, n); }

#[test]
fn small() { assert_eq(1 + 1, 2); }

#[test]
fn ungranted(api: Http) {}
"#,
    );
    let (code, j) = c.json(&["--cases", "3"]);
    assert_eq!(code, 1, "{j}");
    let t = &j["tests"];
    let used = |i: usize| {
        (
            t[i]["fuel"].as_u64().unwrap(),
            t[i]["peak_memory"].as_u64().unwrap() >> 10,
        )
    };
    let (heavy, fails, prop, small) = (used(0), used(1), used(2), used(3));
    assert!(heavy.0 > 10_000_000 && fails.0 > 10_000_000, "{j}");
    assert!(prop.0 > 0 && small.0 < 10_000_000 && small.1 > 0, "{j}");
    // A test whose run never started used nothing.
    assert_eq!((t[4]["fuel"].is_null(), t[4]["peak_memory"].is_null()), (true, true));

    let commas = |n: u64| {
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    };
    let note = |name: &str, f: u64| {
        format!(
            "note: {name} used {} fuel; --listen gives each request 10,000,000 (raise it with --fuel)\n",
            commas(f)
        )
    };
    let app = &c.app;
    let failures = format!(
        "FAIL heavy_fails\n  {app}:5:64: assertion failed: late\n{}",
        note("heavy_fails", fails.0)
    );
    let ungranted = "FAIL ungranted\n  test `ungranted` needs `api: Http`\n  hint: name it in a --grants file\n";
    let (code, out, _) = c.kio(&["--cases", "3"]);
    assert_eq!(code, 1);
    let want = format!(
        "ok   heavy\n{}{failures}ok   prop  (3 cases compiled; seed 1)\nok   small\n{ungranted}3 passed, 2 failed\n",
        note("heavy", heavy.0)
    );
    assert_eq!(out, want);

    let (code, out, _) = c.kio(&["--cases", "3", "--verbose"]);
    assert_eq!(code, 1);
    let v = |(f, kb): (u64, u64)| format!("  (fuel {}; peak memory {kb} KB)", commas(f));
    let failures = failures.replacen("heavy_fails\n", &format!("heavy_fails{}\n", v(fails)), 1);
    let want = format!(
        "ok   heavy{}\n{}{failures}ok   prop  (3 cases compiled; seed 1){}\nok   small{}\n{ungranted}3 passed, 2 failed\n",
        v(heavy),
        note("heavy", heavy.0),
        v(prop),
        v(small)
    );
    assert_eq!(out, want);
}

/// T4a: what `hook: Http` answers in the fake tests below.
const HOOK: &str = "# hook.http: one entry per response\nPOST /v1/events\n503\n---\nPOST /v1/events\n200\nContent-Type: application/json\nx-two: b\n\n{\"ok\":true}\n---\nGET /v1/status?full=1\n200\n\nup\n---\nGET /v1/down\n503\n---\nGET /moved\n302\nlocation: /v1/status?full=1\n---\nPOST /v1/slow\ntimeout\n";

const FAKE: &str = r#"fn post(hook: Http, p: Str) -> Result<HttpResponse, HttpErr> { hook.send("POST", p, [], "x".to_bytes(), 0) }

#[test]
fn answers(hook: Http) -> Result<(), HttpErr> {
    assert_eq(post(hook, "/v1/events")?.status, 503);
    let r = post(hook, "/v1/events")?;
    assert_eq(r.status, 200);
    assert_eq(r.headers, ["content-type: application/json", "x-two: b"]);
    assert_eq(r.body.to_str(), "{\"ok\":true}");
    assert_eq(post(hook, "/v1/events")?.status, 200);
    assert_eq(hook.get("/v1/status?full=1")?.to_str(), "up");
    assert_eq(hook.get("/v1/down"), Err(HttpErr::Status(503)));
    assert_eq(hook.get("/moved"), Err(HttpErr::Status(302)));
    assert_eq(hook.send("GET", "/moved", [], "".to_bytes(), 0)?.status, 302);
    Ok(())
}

#[test]
fn slow(hook: Http) { assert_eq(post(hook, "/v1/slow"), Err(HttpErr::Timeout)); }

#[test]
fn typo(hook: Http) -> Result<(), HttpErr> {
    assert_eq(post(hook, "/v1/events")?.status, 503);
    assert_eq(post(hook, "/v1/evnts"), Err(HttpErr::Denied));
    assert(false, "after");
    Ok(())
}

#[test]
fn handled(hook: Http) { assert_eq(post(hook, "/nowhere"), Err(HttpErr::Denied)); }

#[test]
fn put(hook: Http) {
    let m = "PU" + "T";
    assert_eq(hook.send(m, "/v1/events", [], "".to_bytes(), 0), Err(HttpErr::Denied));
    assert(false, "after put");
}
"#;

#[test]
fn a_fake_answers_from_its_file() {
    let c = Case::new(FAKE);
    let hook = c.file("hook.http", HOOK);
    let g = c.file(
        "t.grants",
        &format!("--http hook=fake:{hook}\n--http-methods hook=POST\n"),
    );
    let (code, out, err) = c.kio(&["--grants", &g]);
    assert_eq!(code, 1, "{err}");
    let app = &c.app;
    // An unmatched request is `Denied`, and listed when its test fails; a method not granted is
    // `Denied` before any lookup, so it is not listed.
    let want = format!(
        "ok   answers\nok   handled\nFAIL put\n  {app}:36:5: assertion failed: after put\nok   slow\nFAIL typo\n  {app}:25:5: assertion failed: after\n  hook: 2 requests, 1 unmatched: POST /v1/evnts\n3 passed, 2 failed\n"
    );
    assert_eq!(out, want, "{err}");
    let (_, j) = c.json(&["--grants", &g]);
    let t = &j["tests"];
    assert_eq!(
        t[4]["unmatched"],
        serde_json::json!(["hook: 2 requests, 1 unmatched: POST /v1/evnts"])
    );
    assert!(t[1]["unmatched"].is_null() && t[2]["unmatched"].is_null(), "{j}");
    // `timeout` answers at once: under 100 ms more than `handled`, which also sends one request.
    let ms = |i: usize| t[i]["ms"].as_f64().unwrap();
    assert!(ms(3) < ms(1) + 100.0, "{j}");
}

/// A test grants file's fake is refused everywhere else, by name.
#[test]
fn a_fake_is_refused_outside_kio_test() {
    let t = tempfile::tempdir().unwrap();
    let hook = write(t.path(), "hook.http", HOOK);
    let spec = format!("hook=fake:{hook}");
    let refusal = format!(
        "kio: --http {spec}: a fake answers tests only\n  hint: grant a host (--http hook=https://hooks.example); a fake belongs in the grants file kio test reads\n"
    );
    let main = write(t.path(), "main.kio", "fn main(hook: Http) -> Int { 1 }\n");
    let (code, out, err) = kio_in(t.path(), &["run", &main, "--http", &spec]);
    assert_eq!((code, out.as_str(), err.as_str()), (2, "", refusal.as_str()));
    let g = write(t.path(), "g.grants", &format!("--http {spec}\n"));
    let (code, _, err) = kio_in(t.path(), &["run", &main, "--grants", &g]);
    assert_eq!((code, err.as_str()), (2, refusal.as_str()));
    let (code, _, err) = kio_in(t.path(), &["check", &main, "--grants", &g]);
    assert_eq!((code, err.as_str()), (2, refusal.as_str()));
    let app = write(
        t.path(),
        "app.kio",
        "fn main(req: Incoming, res: Outgoing, hook: Http) -> Result<(), WebErr> { res.send(200, [], [], \"ok\".to_bytes()) }\n",
    );
    let (code, _, err) = kio_in(t.path(), &["run", &app, "--listen", "127.0.0.1:0", "--http", &spec]);
    assert_eq!((code, err.as_str()), (2, refusal.as_str()));
    let (code, out, err) = kio_in(t.path(), &["review", &main, "--base", &main, "--grants", &g]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("a fake answers tests only"), "{out}{err}");
    assert!(
        out.contains("a fake belongs in the grants file kio test reads"),
        "{out}{err}"
    );
}

#[test]
fn a_fake_file_error_names_its_line() {
    let c = Case::new("#[test]\nfn t(hook: Http) {}\n");
    let methods = "GET, HEAD, POST, PUT, PATCH, DELETE or OPTIONS";
    let cases = [
        (
            "POST /a\n200\n---\nFETCH /b\n200\n",
            format!("4: expected METHOD /path ({methods})"),
        ),
        ("# c\n\nPOST a\n200\n", format!("3: expected METHOD /path ({methods})")),
        ("POST /a\n200\n---\n", format!("4: expected METHOD /path ({methods})")),
        ("POST /a\n2000\n", "2: expected a status 100-599 or timeout".into()),
        ("POST /a\n", "2: expected a status 100-599 or timeout".into()),
        ("POST /a\n200\nok\n\nbody\n", "3: expected name: value".into()),
        ("POST /a\ntimeout\n\nbody\n", "4: expected --- after timeout".into()),
        ("", format!("1: expected METHOD /path ({methods})")),
        ("POST /a\n+200\n", "2: expected a status 100-599 or timeout".into()),
        // A path no request could carry would never match.
        ("POST //a\n200\n", format!("1: expected METHOD /path ({methods})")),
        ("POST /a#b\n200\n", format!("1: expected METHOD /path ({methods})")),
        // A terminal escape in FILE is named by its line, never printed.
        (
            "POST /a\n200\n---\nPOST /x\u{1b}[31m\n200\n",
            format!("4: expected METHOD /path ({methods})"),
        ),
    ];
    for (text, want) in cases {
        let hook = c.file("hook.http", text);
        let g = c.file(
            "t.grants",
            &format!("--http hook=fake:{hook}\n--http-methods hook=POST\n"),
        );
        let (code, out, err) = c.kio(&["--grants", &g]);
        assert!(!err.contains('\u{1b}'), "{err:?}");
        assert_eq!(
            (code, out.as_str(), err),
            (2, "", format!("kio: {hook}:{want}\n")),
            "{text:?}"
        );
    }
    // An entry for a method the grant does not allow.
    let hook = c.file("hook.http", "POST /a\n200\n---\nPUT /x\n200\n");
    let g = c.file(
        "t.grants",
        &format!("--http hook=fake:{hook}\n--http-methods hook=post\n"),
    );
    let (code, _, err) = c.kio(&["--grants", &g]);
    assert_eq!(
        (code, err),
        (2, format!("kio: {hook}:4: PUT /x: --http hook grants only GET, POST\n  hint: add PUT to --http-methods hook, or drop the entry\n"))
    );
}

#[test]
fn a_fake_file_follows_the_grants_file_rules() {
    let c = Case::new("#[test]\nfn t(hook: Http, d: Dir) {}\n");
    let data = c.dir.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let inner = write(&data, "hook.http", HOOK);
    let spec = format!("hook=fake:{inner}");
    let g = c.file("t.grants", &format!("--http {spec}\n--dir d={}\n", data.display()));
    let (code, _, err) = c.kio(&["--grants", &g]);
    assert_eq!(
        (code, err),
        (2, format!("kio: --http {spec}: `{inner}` is inside `--dir d={}`; the program could rewrite its next run's answers\n  hint: give kio a fake file of its own\n", data.display()))
    );
    // Read-only, the program cannot change it.
    let g = c.file(
        "t.grants",
        &format!(
            "--http {spec}\n--http-methods hook=POST\n--dir d={}:ro\n",
            data.display()
        ),
    );
    let (code, out, err) = c.kio(&["--grants", &g]);
    assert_eq!(code, 0, "{out}{err}");
    let hook = c.file("hook.http", HOOK);
    std::fs::hard_link(&hook, c.dir.path().join("other")).unwrap();
    let spec = format!("hook=fake:{hook}");
    let g = c.file("t.grants", &format!("--http {spec}\n"));
    let (code, _, err) = c.kio(&["--grants", &g]);
    assert_eq!(
        (code, err),
        (2, format!("kio: --http {spec}: `{hook}` has other hard links; the program could rewrite its next run's answers through one\n  hint: give kio a fake file of its own\n"))
    );
    let g = c.file("t.grants", "--http hook=fake:missing.http\n");
    let (code, _, err) = c.kio(&["--grants", &g]);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("kio: --http hook=fake:missing.http: cannot read: "),
        "{err}"
    );
    let dir = c.dir.path().display().to_string();
    let g = c.file("t.grants", &format!("--http hook=fake:{dir}\n"));
    let (code, _, err) = c.kio(&["--grants", &g]);
    assert_eq!(
        (code, err),
        (2, format!("kio: --http hook=fake:{dir}: not a regular file\n"))
    );
    // A database's sidecar is its file too.
    let wal = c.file("app.db-wal", HOOK);
    let db = c.dir.path().join("app.db").display().to_string();
    let spec = format!("hook=fake:{wal}");
    let g = c.file("t.grants", &format!("--http {spec}\n--db db={db}\n"));
    let (code, _, err) = c.kio(&["--grants", &g]);
    assert_eq!(
        (code, err),
        (2, format!("kio: --http {spec}: `{wal}` is a file of `--db db`; the program could rewrite its next run's answers\n  hint: give kio a fake file of its own\n"))
    );
}

/// A CRLF FILE answers as an LF one; a property may not take a fake `Http`.
#[test]
fn a_fake_in_crlf_and_in_a_property() {
    let c = Case::new(
        r#"#[test]
fn crlf(hook: Http) -> Result<(), HttpErr> {
    let r = hook.send("GET", "/c", [], "".to_bytes(), 0)?;
    assert_eq(r.headers, ["x-a: 1"]);
    assert_eq(r.body.to_str(), "one\ntwo");
    Ok(())
}

#[test]
fn prop(hook: Http, n: Int) { assert_eq(n, n); }
"#,
    );
    let hook = c.file("hook.http", "GET /c\r\n200\r\nx-a: 1\r\n\r\none\r\ntwo\r\n");
    let g = c.file("t.grants", &format!("--http hook=fake:{hook}\n"));
    let (code, j) = c.json(&["--grants", &g]);
    assert_eq!(code, 1, "{j}");
    assert!(j["tests"][0]["ok"].as_bool().unwrap(), "{j}");
    assert_eq!(
        j["tests"][1]["error"]["message"],
        "a property may not take `hook: Http` yet; a fake's answers are counted per test, not per case"
    );
}

/// `--http-header` applies to a fake as to a real grant, and its value reaches nothing.
#[test]
fn a_fakes_header_value_appears_nowhere() {
    let c = Case::new(
        r#"#[test]
fn sends(hook: Http) -> Result<(), HttpErr> {
    assert_eq(hook.send("POST", "/v1/events", ["Authorization: mine"], "".to_bytes(), 0), Err(HttpErr::Invalid));
    assert_eq(hook.send("POST", "/v1/events", [], "".to_bytes(), 0)?.status, 503);
    assert_eq(hook.send("POST", "/v1/nope", [], "".to_bytes(), 0), Err(HttpErr::Denied));
    assert(false, "show everything");
    Ok(())
}
"#,
    );
    let hook = c.file("hook.http", HOOK);
    let g = c.file(
        "t.grants",
        &format!(
            "--http hook=fake:{hook}\n--http-methods hook=POST\n--http-header hook=Authorization:env:KIO_T4A_SECRET\n"
        ),
    );
    let secret = "t4a-secret-value-81f2";
    let tmp = c.dir.path().join("tmp");
    for json in [false, true] {
        let mut args = vec!["test", c.app.as_str(), "--grants", g.as_str()];
        args.extend(json.then_some("--json"));
        let o = Command::new(env!("CARGO_BIN_EXE_kio"))
            .args(&args)
            .env("TMPDIR", &tmp)
            .env("KIO_T4A_SECRET", secret)
            .output()
            .unwrap();
        let (out, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert_eq!(o.status.code(), Some(1), "{out}{err}");
        assert!(
            out.contains("hook: 2 requests, 1 unmatched: POST /v1/nope"),
            "{out}{err}"
        );
        assert!(!out.contains(secret) && !err.contains(secret), "{out}{err}");
    }
    let mut stack = vec![c.dir.path().to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            match p.is_dir() {
                true => stack.push(p),
                false => assert!(
                    !String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains(secret),
                    "{p:?}"
                ),
            }
        }
    }
}

/// T4b: a fake's requests, recorded into the test's `sent: Dir`.
const RECORD: &str = r#"#[test]
fn records(hook: Http, sent: Dir) -> Result<(), IoErr> {
    let bin = match "00ff0d0a2d2d2d0a80".from_hex() { Some(b) => b, None => "".to_bytes() };
    let rs = hook.send_all([
        HttpRequest { method: "POST", path: "/v1/events?x=1", headers: ["X-Id: 7"], body: "{\"id\":1}".to_bytes(), timeout_ms: 0 },
        HttpRequest { method: "POST", path: "/bin", headers: [], body: bin, timeout_ms: 0 },
    ]);
    assert_eq(rs.len(), 2);
    assert_eq(sent.list("")?, ["hook-1", "hook-2"]);
    assert_eq(sent.read("hook-1")?.to_str(), "POST /v1/events?x=1\nx-id: 7\nauthorization: <injected>\n\n{\"id\":1}");
    let mut want = "POST /bin\nauthorization: <injected>\n\n".to_bytes();
    want.extend(bin);
    assert_eq(sent.read("hook-2")?, want);
    // Refused by the host before the lookup: no record. Unmatched: recorded, it was sent.
    assert_eq(hook.send("PU" + "T", "/v1/events", [], "".to_bytes(), 0), Err(HttpErr::Denied));
    assert_eq(hook.get("/nowhere"), Err(HttpErr::Denied));
    assert_eq(sent.list("")?.len(), 3);
    assert_eq(sent.read("hook-3")?.to_str(), "GET /nowhere\nauthorization: <injected>\n\n");
    Ok(())
}

#[test]
fn numbers_per_test(hook: Http, sent: Dir) -> Result<(), IoErr> {
    let _ = hook.get("/v1/status");
    assert_eq(sent.list("")?, ["hook-1"]);
    Ok(())
}

#[test]
fn taken(hook: Http, sent: Dir) -> Result<(), IoErr> {
    sent.create("hook-1", "mine".to_bytes())?;
    let _ = hook.get("/v1/status");
    Ok(())
}

#[test]
fn without_sent(hook: Http, out: Dir) -> Result<(), IoErr> {
    assert_eq(hook.get("/v1/status"), Ok("up".to_bytes()));
    assert_eq(out.list("")?.len(), 0);
    Ok(())
}
"#;

#[test]
fn a_fake_records_what_it_was_sent() {
    let c = Case::new(RECORD);
    let hook = c.file(
        "hook.http",
        "POST /v1/events?x=1\n200\n---\nPOST /bin\n201\n---\nGET /v1/status\n200\n\nup\n",
    );
    let key = c.file("key.txt", "Bearer s3cret-t4b");
    let g = c.file(
        "t.grants",
        &format!("--http hook=fake:{hook}\n--http-methods hook=POST\n--http-header hook=Authorization:file:{key}\n--http-record hook=sent\n"),
    );
    let (code, out, err) = c.kio(&["--grants", &g]);
    assert_eq!(
        (code, out.as_str()),
        (
            1,
            "ok   numbers_per_test\nok   records\nFAIL taken\n  --http-record hook: cannot write `hook-1`: File exists (os error 17); a record is a new file, so leave `hook-*` names to the host\nok   without_sent\n3 passed, 1 failed\n"
        ),
        "{err}"
    );
}

#[test]
fn a_record_is_refused_outside_its_rules() {
    let c = Case::new("#[test]\nfn t(hook: Http, sent: Dir) {}\n");
    let hook = c.file("hook.http", "GET /a\n200\n");
    let fake = format!("--http hook=fake:{hook}\n");
    let data = c.dir.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let cases = [
        (
            "--http hook=example.com\n--http-record hook=sent\n".to_string(),
            "kio: --http-record hook=sent: hook is not a fake\n  hint: a real grant's requests are in the audit log (--audit)\n".to_string(),
        ),
        (
            format!("{fake}--http-record other=sent\n"),
            "kio: --http-record other=sent: other is not a fake\n  hint: a real grant's requests are in the audit log (--audit)\n".into(),
        ),
        (
            format!("{fake}--http-record hook=sent\n--dir sent={}\n", data.display()),
            "kio: --http-record hook=sent: the file grants `--dir sent`, and records go to a fresh temporary Dir\n  hint: remove --dir sent\n".into(),
        ),
        (
            format!("{fake}--http-record hook=../sent\n"),
            "kio: --http-record hook=../sent: expected grant=dir\n  hint: name a fake Http grant and a Dir parameter: `--http-record hook=sent`\n".into(),
        ),
        (
            format!("{fake}--http-record hook=sent\n--http-record hook=other\n"),
            "kio: --http-record hook=other: `hook` is recorded twice\n  hint: give one --http-record hook\n".into(),
        ),
    ];
    for (grants, want) in cases {
        let g = c.file("t.grants", &grants);
        let (code, out, err) = c.kio(&["--grants", &g]);
        assert_eq!((code, out.as_str(), err), (2, "", want), "{grants}");
    }
    let (code, _, err) = c.kio(&["--http-record", "hook=sent"]);
    assert_eq!(
        (code, err.as_str()),
        (2, "kio: `--http-record` is not a flag of kio test\n  hint: name grants in a --grants file, one flag per line\n")
    );
    // Every other command refuses it, on the command line and in a grants file.
    let refusal = "kio: `--http-record` is not a flag of kio run\n  hint: it records a fake's requests for kio test\n";
    let t = c.dir.path();
    let main = write(t, "main.kio", "fn main(hook: Http) -> Int { 1 }\n");
    let (code, _, err) = kio_in(t, &["run", &main, "--http-record", "hook=sent"]);
    assert_eq!((code, err.as_str()), (2, refusal));
    let g = write(t, "g.grants", "--http-record hook=sent\n");
    let (code, _, err) = kio_in(t, &["run", &main, "--grants", &g]);
    assert_eq!((code, err.as_str()), (2, refusal));
    let (code, _, err) = kio_in(t, &["check", &main, "--grants", &g]);
    assert_eq!((code, err.as_str()), (2, refusal));
    let app = write(
        t,
        "server.kio",
        "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> { res.send(200, [], [], \"ok\".to_bytes()) }\n",
    );
    let (code, _, err) = kio_in(
        t,
        &["run", &app, "--listen", "127.0.0.1:0", "--http-record", "hook=sent"],
    );
    assert_eq!((code, err.as_str()), (2, refusal));
    let (code, out, err) = kio_in(t, &["review", &main, "--base", &main, "--grants", &g]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("`--http-record` is not a flag of kio run"), "{out}{err}");
}
