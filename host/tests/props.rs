//! T2b: `kio test` runs a property (a test with data parameters) on generated cases, each
//! compiled in a fresh instance with fresh grants, and shrinks a failure. This test build also
//! runs each case in the interpreter (the oracle) and fails on a disagreement.
use std::path::Path;
use std::process::Command;

/// A directory with `app.kio` holding `src`, and its own temporary directory.
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

    /// `(exit code, stdout, stderr)` of `kio test app.kio args..`, with `env` set.
    fn kio_env(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_kio"))
            .args(["test", self.app.as_str()])
            .args(args)
            .env("TMPDIR", self.dir.path().join("tmp"))
            .envs(env.iter().copied())
            .output()
            .unwrap();
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
    }

    fn kio(&self, args: &[&str]) -> (i32, String, String) {
        self.kio_env(args, &[])
    }

    /// `--json`'s object for the one test `kio test --json args..` runs.
    fn json(&self, args: &[&str]) -> (i32, serde_json::Value) {
        let mut all = vec!["--json"];
        all.extend(args);
        let (code, out, err) = self.kio(&all);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}{err}"));
        (code, v["tests"][0].clone())
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

const PROPS: &str = r#"struct Order { id: Str, total: Int }

#[test]
fn reverse_twice(xs: List<Int>) {
    let mut ys = xs;
    ys.reverse();
    ys.reverse();
    assert_eq(ys, xs);
}

#[test]
fn small(n: Int, o: Order) {
    assert(n <= 5 || o.total > 1000000000000000000, "too big");
}

#[test]
fn sevens(n: Int) { assert(n % 10 != 7, "a seven"); }

#[test]
fn bytes_round_trip(b: Bytes, bs: List<Bytes>) {
    assert_eq(b.to_hex().from_hex(), Some(b));
    assert(bs.len() <= 32, "short");
}

fn main() {}
"#;

#[test]
fn a_passing_property() {
    let c = Case::new(PROPS);
    let (code, out, err) = c.kio(&["--filter", "reverse", "--cases", "20"]);
    assert_eq!(
        (code, out.as_str()),
        (
            0,
            "ok   reverse_twice  (20 cases compiled; seed 1)\n1 passed, 0 failed\n"
        ),
        "{err}"
    );
    let (code, t) = c.json(&["--filter", "reverse", "--cases", "7", "--seed", "9"]);
    assert_eq!(code, 0);
    let fields = ["cases", "compiled", "seed", "discarded", "shrinks"].map(|k| t[k].clone());
    assert_eq!(fields, [7, 7, 9, 0, 0].map(serde_json::Value::from), "{t}");
    assert_eq!(t["input"], serde_json::Value::Null);
}

#[test]
fn a_failing_property_shrinks() {
    let c = Case::new(PROPS);
    let (code, out, _) = c.kio(&["--filter", "small"]);
    assert_eq!(code, 1, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("FAIL small  ("), "{out}");
    assert!(lines[0].ends_with(" compiled; seed 1)"), "{out}");
    assert_eq!(lines[1], "  minimal input: n = 6, o = Order { id: \"\", total: 0 }");
    assert!(
        lines[2].starts_with("  shrunk in ") && lines[2].ends_with(" compiled runs"),
        "{out}"
    );
    assert!(lines[3].ends_with("app.kio:13:5: assertion failed: too big"), "{out}");
    assert_eq!(lines[4], format!("  rerun: kio test {} --filter small --seed 1", c.app));
    let (_, t) = c.json(&["--filter", "small"]);
    assert_eq!(
        t["input"],
        serde_json::json!({"n": "6", "o": "Order { id: \"\", total: 0 }"})
    );
    assert_eq!(t["error"]["kind"], "runtime");
    assert_eq!(t["cases"], t["compiled"]);
}

#[test]
fn runs_are_reproducible_and_seeded() {
    let c = Case::new(PROPS);
    let run = |seed: &str| {
        let (code, t) = c.json(&["--filter", "sevens", "--seed", seed]);
        assert_eq!(code, 1);
        (t["cases"].clone(), t["input"].clone(), t["error"]["message"].clone())
    };
    assert_eq!(run("1"), run("1"));
    // Another seed finds another failing case.
    let (one, five) = (run("1"), run("5"));
    assert_ne!(one, five);
    for input in [one.1, five.1] {
        let n: i64 = input["n"].as_str().unwrap().parse().unwrap();
        assert_eq!(n % 10, 7, "{n}");
    }
}

#[test]
fn discards() {
    let c = Case::new(
        "#[test]\nfn picky(n: Int) { assume(n == 1); }\n#[test]\nfn some(n: Int) { assume(n != 0); assert(n != 0, \"zero\"); }\nfn main() {}",
    );
    let (code, out, _) = c.kio(&["--filter", "picky", "--cases", "20"]);
    assert_eq!(code, 1, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("FAIL picky  (20 cases compiled; seed 1; "),
        "{out}"
    );
    assert!(
        lines[1].starts_with("  `picky` discarded ") && lines[1].ends_with(" of 20 inputs"),
        "{out}"
    );
    assert_eq!(lines[2], "  hint: loosen `assume`, or build the inputs inside the test");
    let (code, t) = c.json(&["--filter", "some", "--cases", "40"]);
    assert_eq!(code, 0, "{t}");
    assert!(t["discarded"].as_u64().unwrap() > 0, "{t}");
}

#[test]
fn a_property_gets_a_fresh_dir_per_case() {
    let c = Case::new(
        "#[test]\nfn writes(d: Dir, s: Str) -> Result<(), IoErr> {\n  assert_eq(d.list(\"\")?, []);\n  d.create(\"a.txt\", s.to_bytes())?;\n  assert_eq(d.read(\"a.txt\")?, s.to_bytes());\n  Ok(())\n}\nfn main() {}",
    );
    let (code, out, err) = c.kio(&["--cases", "10"]);
    assert_eq!(code, 0, "{out}{err}");
    // Every directory was removed.
    assert_eq!(std::fs::read_dir(c.dir.path().join("tmp")).unwrap().count(), 0);
}

#[test]
fn effects_are_refused() {
    let src = "#[test]\nfn calls(api: Http, n: Int) { let _ = api.get(\"/\"); }\n#[test]\nfn writes(d: Dir, n: Int) -> Result<(), IoErr> { d.write(\"x\", \"y\".to_bytes()) }\n#[test]\nfn reads(fx: Dir, n: Int) -> Result<(), IoErr> { assert_eq(fx.read(\"in.txt\")?.to_str(), \"hi\"); Ok(()) }\n#[test]\nfn stores(k: Kv, n: Int) {}\n#[test]\nfn logs(log: Log, n: Int) { let _ = log.write(\"x\"); }\nfn main() {}";
    let c = Case::new(src);
    let fx = c.dir.path().join("fx");
    std::fs::create_dir(&fx).unwrap();
    std::fs::write(fx.join("in.txt"), "hi").unwrap();
    let out_dir = c.dir.path().join("out");
    std::fs::create_dir(&out_dir).unwrap();
    let grants = format!(
        "--http api=http://127.0.0.1:9\n--dir d={}\n--dir fx={}:ro\n--kv k={}\n",
        out_dir.display(),
        fx.display(),
        c.dir.path().join("k.kv").display()
    );
    let g = c.file("t.grants", &grants);
    let log_grants = c.file(
        "l.grants",
        &format!("--log log={}\n", c.dir.path().join("t.log").display()),
    );
    let error = |filter: &str| {
        let (_, t) = c.json(&["--grants", &g, "--filter", filter, "--cases", "3"]);
        (
            t["error"]["kind"].clone(),
            t["error"]["message"].clone(),
            t["error"]["hint"].clone(),
        )
    };
    let e = error("calls");
    assert_eq!(e.0, "grant");
    assert_eq!(
        e.1,
        "a property runs many times; `api: Http` would repeat real requests"
    );
    assert_eq!(e.2, "test the call in an example test, or pass the data as a parameter");
    let hint = "grant it `:ro`, or leave it out of the grants file for a fresh one per case";
    assert_eq!(
        error("writes"),
        (
            "grant".into(),
            "a property runs many times; `d: Dir` from the grants file would share its writes across cases".into(),
            hint.into()
        )
    );
    let (_, t) = c.json(&["--grants", &log_grants, "--filter", "logs", "--cases", "3"]);
    assert_eq!(
        t["error"]["message"],
        "a property runs many times; `log: Log` from the grants file would share one file across cases"
    );
    assert_eq!(
        t["error"]["hint"],
        "use `--log log=-`, or leave it out of the grants file for a buffer per case"
    );
    let stderr_log = c.file("e.grants", "--log log=-\n");
    let (code, t) = c.json(&["--grants", &stderr_log, "--filter", "logs", "--cases", "3"]);
    assert_eq!(code, 0, "{t}");
    assert_eq!(
        error("stores").1,
        "a property runs many times; `k: Kv` from the grants file would share its writes across cases"
    );
    let (code, t) = c.json(&["--grants", &g, "--filter", "reads", "--cases", "3"]);
    assert_eq!((code, t["ok"].clone()), (0, true.into()), "{t}");
    assert_eq!(std::fs::read_dir(&out_dir).unwrap().count(), 0);
}

#[test]
fn compiled_and_interpreted_runs_must_agree() {
    let c = Case::new(PROPS);
    // The hook makes each compiled case pass without calling the test.
    let (code, out, _) = c.kio_env(&["--filter", "small", "--cases", "50"], &[("KIO_TEST_SKEW_CASE", "1")]);
    assert_eq!(code, 1, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[1].starts_with("  input: n = "), "{out}");
    assert_eq!(
        lines[2],
        "  compiled and interpreted runs disagree: compiled passed, interpreted assertion failed: too big at 13:5"
    );
    let (code, _, _) = c.kio_env(&["--filter", "reverse", "--cases", "5"], &[("KIO_TEST_SKEW_CASE", "1")]);
    assert_eq!(code, 0, "agreeing cases pass");
}

#[test]
fn bad_case_flags() {
    let c = Case::new(PROPS);
    let (code, _, err) = c.kio(&["--cases", "0"]);
    assert_eq!((code, err.trim()), (2, "kio: --cases 0: expected 1 to 100000"));
    let (code, _, err) = c.kio(&["--seed", "-1"]);
    assert_eq!(
        (code, err.trim()),
        (2, "kio: --seed -1: expected a non-negative integer")
    );
}

/// The docs' property examples and this file's, run as kio's CI runs them: each case compiled
/// and, in this test build, interpreted too, so a disagreement on any generated input fails.
#[test]
fn properties_agree_with_the_interpreter() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs");
    let block = |file: &str, heading: &str| {
        let text = std::fs::read_to_string(docs.join(file)).unwrap();
        let from = &text[text.find(heading).unwrap_or_else(|| panic!("{heading}"))..];
        from.split("```\n").nth(1).unwrap().to_string()
    };
    let primer = block("primer.md", "## Tests: `#[test]`");
    let language = block("language.md", "### Properties") + "fn main() {}\n";
    for (src, failing) in [(primer.as_str(), 0), (language.as_str(), 0), (PROPS, 2)] {
        let c = Case::new(src);
        let (_, out, err) = c.kio(&["--cases", "60"]);
        assert!(!out.contains("disagree"), "{out}{err}");
        assert!(out.ends_with(&format!(" passed, {failing} failed\n")), "{out}{err}");
    }
}

#[test]
fn a_long_input_is_clipped() {
    // A limit is not shrunk, so the input shown is the generated one: about 15 KB as a literal
    // per parameter. The line, and the `--json` literals together, stop at 4,096 bytes.
    let ty = (0..9).fold("Int".to_string(), |t, _| format!("Pair<{t}>"));
    let c = Case::new(&format!(
        "struct Pair<T> {{ a: T, b: T }}\n#[test]\nfn spins(p: {ty}, q: {ty}) {{ loop {{}} }}\nfn main() {{}}"
    ));
    let (code, t) = c.json(&["--cases", "2", "--fuel", "100000"]);
    assert_eq!((code, t["error"]["kind"].clone()), (1, "fuel".into()), "{t}");
    let (p, q) = (t["input"]["p"].as_str().unwrap(), t["input"]["q"].as_str().unwrap());
    assert!(p.len() == 4099 && p.ends_with("..."), "{}", p.len());
    assert_eq!(q, "...");
    let (_, out, _) = c.kio(&["--cases", "2", "--fuel", "100000"]);
    let line = out.lines().find(|l| l.starts_with("  input: p = ")).unwrap();
    assert_eq!(line.len(), "  input: ".len() + 4099);
}
