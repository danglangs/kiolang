//! R1a: `kio review NEW --base OLD` puts the authority diff, both sides' tests, resource changes,
//! what kio does not check, and the identity of what was read on one page, each line labelled.
use std::path::Path;
use std::process::Command;

/// `(exit code, stdout, stderr)` of `kio args..`, with `tmp` as the system's temporary directory.
fn kio(tmp: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(args)
        .env("TMPDIR", tmp)
        .output()
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

/// `old/app.kio` and `new/app.kio` in a directory of their own, with its own temporary directory.
struct Sides {
    dir: tempfile::TempDir,
    old: String,
    new: String,
}

impl Sides {
    fn new(old: &str, new: &str) -> Sides {
        let dir = tempfile::tempdir().unwrap();
        for d in ["old", "new", "tmp"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        let old = write(dir.path(), "old/app.kio", old);
        let new = write(dir.path(), "new/app.kio", new);
        Sides { dir, old, new }
    }

    fn review(&self, args: &[&str]) -> (i32, String, String) {
        let mut all = vec!["review", self.new.as_str(), "--base", self.old.as_str()];
        all.extend(args);
        kio(&self.dir.path().join("tmp"), &all)
    }

    /// The lines of section `title`, up to the next title (a line that is not indented).
    fn section(out: &str, title: &str) -> Vec<String> {
        let mut lines = out.lines().skip_while(|l| *l != title);
        assert!(lines.next().is_some(), "no section {title}:\n{out}");
        lines.take_while(|l| l.starts_with(' ')).map(String::from).collect()
    }
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

const OLD: &str = r#"fn count(n: Int) -> Int { let mut i = 0; let mut s = 0; while i < n { s += i; i += 1; } s }

fn main(d: Dir) -> Result<(), IoErr> { let _ = d.read("a.txt")?; Ok(()) }

#[test]
fn counts() { assert_eq(count(1000), 499500); }

#[test]
fn kept() { assert_eq(count(3), 3); }

#[test]
fn gone() { assert_eq(count(0), 0); }

#[test]
fn small(n: Int) { assert(count(n % 10) >= 0, "non-negative"); }
"#;

/// OLD with a `write` on `d`, `gone` removed, `fresh` added and `counts`' loop doubled.
const NEW: &str = r#"fn count(n: Int) -> Int { let mut i = 0; let mut s = 0; while i < n { s += i; i += 1; } s }

fn main(d: Dir) -> Result<(), IoErr> { d.write("b.txt", "x".to_bytes())?; let _ = d.read("a.txt")?; Ok(()) }

#[test]
fn counts() { assert_eq(count(2000), 1999000); }

#[test]
fn kept() { assert_eq(count(3), 3); }

#[test]
fn fresh() { assert_eq(count(2), 1); }

#[test]
fn small(n: Int) { assert(count(n % 10) >= 0, "non-negative"); }
"#;

const DEFAULT_GRANTS: &str = "  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)";

const SAME_AUTHOR: &str =
    "  not checked  tests and properties were written by the same author as the code unless stated";

#[test]
fn a_change_on_one_page() {
    let s = Sides::new(OLD, NEW);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    let titles: Vec<&str> = out.lines().filter(|l| !l.starts_with(' ')).collect();
    assert_eq!(titles, ["Authority", "Tests", "Resources", "Not checked", "Identity"]);
    assert_eq!(Sides::section(&out, "Authority"), ["  enforced     ~ d: Dir (+write)"]);
    assert_eq!(
        Sides::section(&out, "Tests"),
        [
            DEFAULT_GRANTS,
            "  tested       old: 4 passed, 0 failed",
            "  tested       new: 4 passed, 0 failed",
            "  tested       added: fresh",
            "  not checked  removed: gone",
            "  tested       small: 100 cases compiled; seed 1",
        ]
    );
    // Only `counts` changed; the total is over the three tests on both sides.
    let resources = Sides::section(&out, "Resources");
    assert_eq!(resources.len(), 2, "{out}");
    assert!(resources[0].starts_with("  measured     counts: fuel "), "{out}");
    // Fuel is deterministic, but its exact count depends on the runtime build.
    assert!(resources[0].contains("%), peak memory "), "{out}");
    assert!(resources[0].contains(" fuel ") && resources[0].contains("(+"), "{out}");
    assert!(
        resources[1].starts_with("  measured     total over 3 tests on both sides: fuel "),
        "{out}"
    );
    assert_eq!(
        Sides::section(&out, "Not checked"),
        ["  not checked  d: Dir write: what is written or removed", SAME_AUTHOR]
    );
    let identity = Sides::section(&out, "Identity");
    assert!(identity[0].starts_with("  kio "), "{out}");
    let hash = |text: &str| lang::digest::to_hex(&lang::digest::sha256(text.as_bytes()));
    assert_eq!(identity[1], format!("  old {} sha256:{}", s.old, hash(OLD)));
    assert_eq!(identity[2], format!("  new {} sha256:{}", s.new, hash(NEW)));
    assert_eq!(identity[3], "  test seed 1");
    for word in ["approved", "safe", "proved"] {
        assert!(!out.contains(word), "{word}: {out}");
    }
}

#[test]
fn no_change() {
    let s = Sides::new(NEW, NEW);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        ["  enforced     no change in authority"]
    );
    let resources = Sides::section(&out, "Resources");
    assert_eq!(resources[0], "  measured     no test changed by more than 2%");
    assert!(
        resources[1].starts_with("  measured     total over 4 tests on both sides: "),
        "{out}"
    );
    assert!(resources[1].contains("(+0.00%), largest peak memory"), "{out}");
    assert_eq!(resources.len(), 2);
    let not_checked = Sides::section(&out, "Not checked");
    assert_eq!(not_checked.last().unwrap(), SAME_AUTHOR);
}

#[test]
fn a_failing_test_in_new() {
    let new = NEW.replace(
        r#"assert(count(n % 10) >= 0, "non-negative")"#,
        r#"assert(n < 5, "small")"#,
    );
    let s = Sides::new(OLD, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 1, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    assert_eq!(tests[2], "  tested       new: 3 passed, 1 failed", "{out}");
    let at = tests
        .iter()
        .position(|l| l.starts_with("  error        FAIL small  ("))
        .expect(&out);
    assert_eq!(tests[at + 1], "                 minimal input: n = 5");
    assert!(
        tests[at + 3].ends_with("app.kio:15:20: assertion failed: small"),
        "{out}"
    );
    // Fewer cases ran than in OLD: the property tested less.
    assert!(
        tests.iter().any(|l| l.starts_with("  not checked  removed: small (")),
        "{out}"
    );
}

#[test]
fn a_program_that_does_not_check() {
    let s = Sides::new(OLD, "fn main() -> Int { \"x\" }\n");
    let (code, out, _) = s.review(&[]);
    assert_eq!(code, 1, "{out}");
    let authority = Sides::section(&out, "Authority");
    assert_eq!(
        authority[0],
        format!("  error        the new program {} does not check", s.new)
    );
    assert!(authority[1].contains("app.kio:1:20: "), "{out}");
    assert_eq!(
        Sides::section(&out, "Tests"),
        ["  error        not run: a program does not check (Authority)"]
    );
}

#[test]
fn requirements_diff() {
    let s = Sides::new(NEW, NEW);
    let root = s.dir.path();
    write(
        root,
        "old/README.md",
        "# app\n\none\ntwo\nthree\n4\n5\n6\n7\n8\n9\nten\n",
    );
    write(
        root,
        "new/README.md",
        "# app\n\none\n2\nthree\n4\n5\n6\n7\n8\n9\nten\neleven\n",
    );
    let (code, out, err) = s.review(&["--requirements", "README.md"]);
    assert_eq!(code, 0, "{out}{err}");
    let dir = root.to_str().unwrap();
    let mut want: Vec<String> = [
        format!("--- {dir}/old/README.md"),
        format!("+++ {dir}/new/README.md"),
        "@@ -1,7 +1,7 @@".into(),
        " # app".into(),
        " ".into(),
        " one".into(),
        "-two".into(),
        "+2".into(),
        " three".into(),
        " 4".into(),
        " 5".into(),
        "@@ -10,3 +10,4 @@".into(),
        " 8".into(),
        " 9".into(),
        " ten".into(),
        "+eleven".into(),
    ]
    .iter()
    .map(|l| format!("               {l}"))
    .collect();
    // The label once, on the line that says what the diff is.
    want.insert(0, "  not checked  what the change claims; kio does not check it".into());
    assert_eq!(Sides::section(&out, "Requirements"), want);
    assert!(out.starts_with("Requirements\n"), "{out}");
    // Too large: refused with a hint, before anything runs.
    write(root, "new/README.md", &"x\n".repeat(600_000));
    let (code, out, err) = s.review(&["--requirements", "README.md"]);
    assert_eq!((code, out.as_str()), (2, ""), "{err}");
    assert!(err.contains("1200000 bytes; kio diffs files up to 1 MB"), "{err}");
    assert!(err.contains("hint: name a shorter file"), "{err}");
    let (code, _, err) = s.review(&["--requirements", &format!("{dir}/new/README.md")]);
    assert_eq!(code, 2);
    assert!(err.contains("an absolute path names one file for both sides"), "{err}");
    assert!(
        err.contains("hint: give a path relative to each program's directory"),
        "{err}"
    );
    let (code, _, err) = s.review(&["--requirements", "NOPE.md"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("hint: the path is relative to each program's directory"),
        "{err}"
    );
}

#[test]
fn json_fields() {
    let s = Sides::new(OLD, NEW);
    write(s.dir.path(), "old/README.md", "a\n");
    write(s.dir.path(), "new/README.md", "b\n");
    let (code, out, err) = s.review(&["--json", "--requirements", "README.md"]);
    assert_eq!(code, 3, "{err}");
    assert_eq!(out.lines().count(), 1, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "authority",
            "identity",
            "not_checked",
            "requirements",
            "resources",
            "tests"
        ]
    );
    assert_eq!(
        v["requirements"]["diff"].as_array().unwrap()[2..],
        ["@@ -1 +1 @@", "-a", "+b"]
    );
    assert_eq!(v["authority"]["grants"], serde_json::json!(["~ d: Dir (+write)"]));
    assert_eq!(v["authority"]["added"], true);
    assert_eq!(v["tests"]["added"], serde_json::json!(["fresh"]));
    assert_eq!(v["tests"]["removed"], serde_json::json!(["gone"]));
    assert_eq!(v["tests"]["new"]["passed"], 4);
    assert_eq!(v["tests"]["properties"][0]["cases"], 100);
    let rows = v["resources"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "counts");
    assert_eq!(v["resources"]["tests"], 3);
    assert_eq!(v["not_checked"]["methods"][0]["methods"], serde_json::json!(["write"]));
    assert_eq!(v["identity"]["seed"], 1);
    assert_eq!(v["identity"]["new"][0]["path"], s.new.as_str());
}

#[test]
fn grants_bind_to_new_main() {
    let s = Sides::new(NEW, NEW);
    let g = write(s.dir.path(), "g.grants", "--dir e=.\n");
    let t = write(s.dir.path(), "t.grants", "--fuel 100000000\n");
    // `--grants` that does not bind NEW's main is an error: exit 1.
    let (code, out, _) = s.review(&["--grants", &g, "--test-grants", &t]);
    assert_eq!(code, 1, "{out}");
    let authority = Sides::section(&out, "Authority");
    assert_eq!(
        authority[1],
        format!("  error        --grants {g} does not bind to {}'s main:", s.new)
    );
    assert!(authority.iter().any(|l| l.contains("hint: ")), "{out}");
    let hash = |b: &[u8]| lang::digest::to_hex(&lang::digest::sha256(b));
    assert!(
        out.contains(&format!("  grants {g} sha256:{}\n", hash(b"--dir e=.\n"))),
        "{out}"
    );
    assert!(
        out.contains(&format!("  test grants {t} sha256:{}\n", hash(b"--fuel 100000000\n"))),
        "{out}"
    );
    // One that binds: nothing under Authority but the diff. The tests take `--test-grants`.
    let g = write(s.dir.path(), "g.grants", "--dir d=.\n");
    let (code, out, _) = s.review(&["--grants", &g, "--test-grants", &t]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        ["  enforced     no change in authority"]
    );
    assert!(out.contains(&format!("  tested       grants: {t}\n")), "{out}");
}

/// A test that writes to its `Dir`.
const WRITES: &str = r#"fn main(d: Dir) -> Result<(), IoErr> { d.write("x", "main".to_bytes()) }

#[test]
fn writes(d: Dir) -> Result<(), IoErr> { d.write("from_test", "t".to_bytes()) }
"#;

#[test]
fn tests_never_get_mains_grants() {
    let s = Sides::new(WRITES, WRITES);
    let prod = s.dir.path().join("prod");
    std::fs::create_dir(&prod).unwrap();
    let g = write(s.dir.path(), "g.grants", &format!("--dir d={}\n", prod.display()));
    let (code, out, err) = s.review(&["--grants", &g]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(Sides::section(&out, "Tests")[0], DEFAULT_GRANTS);
    assert_eq!(
        std::fs::read_dir(&prod).unwrap().count(),
        0,
        "a test wrote into main's dir"
    );
}

#[test]
fn test_grants_that_carry_writes_are_refused() {
    let s = Sides::new(WRITES, WRITES);
    for (line, ok) in [
        ("--dir d=.", false),
        ("--kv k=store.kv", false),
        ("--db b=app.db", false),
        ("--dir d=.:ro", true),
        ("--kv k=:memory:", true),
    ] {
        let t = write(s.dir.path(), "t.grants", &format!("{line}\n"));
        let (code, out, err) = s.review(&["--test-grants", &t]);
        if ok {
            assert_ne!(code, 2, "{line}: {out}{err}");
            continue;
        }
        assert_eq!((code, out.as_str()), (2, ""), "{line}: {err}");
        assert!(err.contains("would carry OLD's test writes into NEW's"), "{err}");
        assert!(err.contains("hint: grant it `:ro`, use `:memory:`"), "{err}");
    }
}

#[test]
fn a_property_made_an_example_test() {
    let new = NEW.replace(
        r#"fn small(n: Int) { assert(count(n % 10) >= 0, "non-negative"); }"#,
        r#"fn small() { assert(count(3) >= 0, "non-negative"); }"#,
    );
    let s = Sides::new(NEW, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    assert!(
        tests.contains(&"  not checked  removed: small (now an example test; 100 cases in old)".to_string()),
        "{out}"
    );
}

#[test]
fn tests_that_do_not_run_list_nothing_as_added() {
    let s = Sides::new(OLD, NEW);
    // `kio test` refuses a server flag: neither side's tests run.
    let t = write(s.dir.path(), "t.grants", "--listen 127.0.0.1:8080\n");
    let (code, out, _) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 1, "{out}");
    let tests = Sides::section(&out, "Tests");
    assert_eq!(tests[1], "  error        old: the tests do not run:", "{out}");
    assert!(tests[2].contains("`--listen` is not for kio test"), "{out}");
    assert!(
        !tests.iter().any(|l| l.contains("added:") || l.contains("removed:")),
        "{out}"
    );
    assert_eq!(
        Sides::section(&out, "Resources"),
        ["  measured     no test ran on both sides"]
    );
}

#[test]
fn a_server_grants_file_binds() {
    let app = "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> { let _ = req.body(); res.send(200, [], [], \"ok\".to_bytes()) }\n";
    let s = Sides::new(app, app);
    let g = write(s.dir.path(), "app.grants", "--listen 127.0.0.1:8080\n--timeout 20\n");
    let t = write(s.dir.path(), "test.grants", "--timeout 20\n");
    let (code, out, err) = s.review(&["--grants", &g, "--test-grants", &t, "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["authority"]["grant_errors"], serde_json::json!([]));
}

#[test]
fn a_base_without_tests() {
    let old = "fn main(d: Dir) -> Result<(), IoErr> { let _ = d.read(\"a.txt\")?; Ok(()) }\n";
    let s = Sides::new(old, NEW);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    assert_eq!(tests[1], "  tested       old: 0 tests");
    assert_eq!(tests[2], "  tested       new: 4 passed, 0 failed");
    assert_eq!(tests[3], "  tested       added: counts");
    // No test on both sides: no total.
    assert_eq!(
        Sides::section(&out, "Resources"),
        ["  measured     no test ran on both sides"]
    );
}

#[test]
fn usage() {
    let s = Sides::new(NEW, NEW);
    let (code, _, err) = kio(s.dir.path(), &["review", &s.new]);
    assert_eq!(code, 2);
    assert!(err.contains("hint: add `--base old.kio`"), "{err}");
    let (code, _, err) = s.review(&["--base", &s.old]);
    assert_eq!(code, 2);
    assert!(err.contains("`--base` is given twice"), "{err}");
    let (code, _, err) = s.review(&["--seed", "2"]);
    assert_eq!(code, 2);
    assert!(err.contains("usage: kio review"), "{err}");
}
