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
    assert_eq!(
        Sides::section(&out, "Authority"),
        ["  enforced     ~ d: Dir (+write)", "  derived        write in main"]
    );
    assert_eq!(
        Sides::section(&out, "Tests"),
        [
            DEFAULT_GRANTS,
            "  tested       old: 4 passed, 0 failed",
            "  tested       new: 4 passed, 0 failed",
            "  tested       added: fresh",
            "  not checked  removed: gone",
            "  not checked  changed: counts",
            "  not checked  changed, reached by no test: main",
            "  tested       small: 100 cases compiled; seed 1",
        ]
    );
    // Only `counts` changed; `fresh` is added; the total is over the three tests on both sides.
    let resources = Sides::section(&out, "Resources");
    assert_eq!(resources.len(), 3, "{out}");
    assert!(resources[1].starts_with("  measured     added: fresh: fuel "), "{out}");
    assert!(resources[1].ends_with(" KB"), "{out}");
    assert!(resources[0].starts_with("  measured     counts: fuel "), "{out}");
    // Fuel is deterministic, but its exact count depends on the runtime build.
    assert!(resources[0].contains("%), peak memory "), "{out}");
    assert!(resources[0].contains(" fuel ") && resources[0].contains("(+"), "{out}");
    assert!(
        resources[2].starts_with("  measured     total over 3 tests on both sides: fuel "),
        "{out}"
    );
    assert_eq!(
        Sides::section(&out, "Not checked"),
        [
            "  not checked  d: Dir write: what is written or removed",
            "  not checked  main: d (changed)",
            SAME_AUTHOR
        ]
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
    // R1a3: it stopped at the failing case, so its case count and fuel say nothing.
    assert!(!tests.iter().any(|l| l.contains("removed: small")), "{out}");
    assert!(tests.iter().any(|l| l.starts_with("  error        small: ")), "{out}");
    assert!(!tests.iter().any(|l| l.starts_with("  tested       small: ")), "{out}");
    let resources = Sides::section(&out, "Resources");
    assert_eq!(
        resources[0], "  measured     1 test failed in new; left out of Resources",
        "{out}"
    );
    assert!(!resources.iter().any(|l| l.contains(" small: ")), "{out}");
    assert!(
        resources
            .last()
            .unwrap()
            .starts_with("  measured     total over 2 tests on both sides: "),
        "{out}"
    );
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["tests"]["fewer_cases"], serde_json::json!([]));
    assert_eq!(v["resources"]["failed"], 1);
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
    // R1d5: on neither side, a not-checked line instead of a refusal.
    let (code, out, err) = s.review(&["--requirements", "NOPE.md"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Requirements"),
        ["  not checked  no requirements file (NOPE.md on neither side)"]
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
        authority[1..3],
        [
            format!("  not checked  {g}: outside the program's directory; not compared with the base"),
            format!("  error        --grants {g} does not bind to {}'s main:", s.new)
        ]
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
    // One that binds says so (R1a2). The tests take `--test-grants`.
    let g = write(s.dir.path(), "g.grants", "--dir d=.\n");
    let (code, out, _) = s.review(&["--grants", &g, "--test-grants", &t]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        [
            "  enforced     no change in authority".to_string(),
            format!("  not checked  {g}: outside the program's directory; not compared with the base"),
            format!("  enforced     --grants {g} binds {}'s main: d=.", s.new),
        ]
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
    // OLD's not running backs nothing (R1d2); NEW's is the error.
    assert_eq!(tests[1], "  not checked  old: the tests do not run:", "{out}");
    assert!(tests[2].contains("`--listen` is not for kio test"), "{out}");
    assert_eq!(tests[4], "  error        new: the tests do not run:", "{out}");
    assert!(
        !tests.iter().any(|l| l.contains("added:") || l.contains("removed:")),
        "{out}"
    );
    assert_eq!(
        Sides::section(&out, "Resources"),
        [
            "  measured     old: the tests do not run; left out of Resources",
            "  measured     no test ran on both sides"
        ]
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
    // No test on both sides: each added one's numbers, no total.
    let resources = Sides::section(&out, "Resources");
    assert_eq!(resources.len(), 5, "{out}");
    for (line, name) in resources.iter().zip(["counts", "fresh", "kept", "small"]) {
        assert!(
            line.starts_with(&format!("  measured     added: {name}: fuel ")),
            "{out}"
        );
    }
    assert_eq!(resources[4], "  measured     no test ran on both sides");
}

/// R1a2: a function line is the checker's attribution (`derived`); exit 3 with no grant added
/// names the functions that add.
#[test]
fn a_new_function_with_no_new_grant() {
    let new = NEW.replace("let _ = d.read(\"a.txt\")?;", "let _ = load(d)?;")
        + "\nfn load(d: Dir) -> Result<Bytes, IoErr> { d.read(\"a.txt\") }\n";
    let s = Sides::new(NEW, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        [
            "  derived      grants unchanged; new functions reach: load",
            "  derived      + load: d read",
        ],
        "{out}"
    );
}

/// R1a2: an added test that fails is an `error` on its `added:` line.
#[test]
fn an_added_test_that_fails() {
    let s = Sides::new(OLD, &NEW.replace("count(2), 1", "count(2), 2"));
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 1, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    assert!(tests.contains(&"  error        added: fresh".to_string()), "{out}");
    assert!(
        tests.iter().any(|l| l.starts_with("  error        FAIL fresh")),
        "{out}"
    );
}

/// R1a2: OLD's own failures back nothing: `not checked`, and no exit 1.
#[test]
fn a_base_that_fails_its_own_tests() {
    let s = Sides::new(&NEW.replace("count(2), 1", "count(2), 2"), NEW);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    assert_eq!(
        tests[1..3],
        [
            "  not checked  old: 3 passed, 1 failed; the base fails its own tests",
            "  tested       new: 4 passed, 0 failed",
        ],
        "{out}"
    );
    assert!(
        tests.contains(&"  not checked  failed in old: fresh".to_string()),
        "{out}"
    );
}

/// R1a2: test grants whose secret cannot be read are one error, before any test runs.
#[test]
fn test_grants_that_cannot_be_applied() {
    let s = Sides::new(OLD, NEW);
    let t = write(s.dir.path(), "t.grants", "--hmac k=env:KIO_R1A2_UNSET\n");
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 1, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Tests"),
        [
            format!("  error        --test-grants {t} cannot be applied; no test ran:"),
            "                 --hmac k: `KIO_R1A2_UNSET` is not set".into(),
            "                 hint: export KIO_R1A2_UNSET in the environment that runs kio, or use file:PATH".into(),
        ]
    );
    assert_eq!(
        Sides::section(&out, "Resources"),
        ["  measured     no test ran on both sides"]
    );
    let (_, out, _) = s.review(&["--test-grants", &t, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["tests"]["setup_error"].as_str().unwrap().starts_with("--hmac k: "),
        "{v}"
    );
    // T is the change author's: its text reaches the page escaped.
    let t = write(s.dir.path(), "t2.grants", "--hmac k=env:NOPE\u{1b}[2J\r  enforced  x\n");
    let (code, out, _) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 1);
    assert!(!out.contains('\u{1b}') && !out.contains('\r'), "{out:?}");
    assert!(out.contains("NOPE\\u{1b}[2J\\r  enforced"), "{out}");
}

/// A second entry beside the root, sharing its module.
const WORKER: &str = "mod lib;\n\nfn main(d: Dir) -> Result<(), IoErr> { lib::touch(d) }\n";
const LIB: &str = "pub fn touch(d: Dir) -> Result<(), IoErr> { let _ = d.read(\"a\")?; Ok(()) }\n";

/// R1a3: an entry OLD does not have is new: all its authority is `+`, with its grants, Not
/// checked lines and files.
#[test]
fn an_added_entry() {
    let s = Sides::new(NEW, NEW);
    let root = s.dir.path();
    write(root, "new/worker.kio", WORKER);
    write(root, "new/lib.kio", LIB);
    let w = write(root, "w.grants", "--dir d=.:ro\n");
    let (code, out, err) = s.review(&["--entry", "worker.kio", "--entry-grants", &w]);
    assert_eq!(code, 3, "{out}{err}");
    let dir = root.to_str().unwrap();
    assert_eq!(
        Sides::section(&out, "Authority"),
        [
            "  enforced     no change in authority".to_string(),
            "  enforced     worker.kio: + d: Dir (read)".into(),
            "  derived      worker.kio:   read in lib::touch".into(),
            format!("  not checked  worker.kio: {w}: outside the program's directory; not compared with the base"),
            format!("  enforced     worker.kio: --entry-grants {w} binds {dir}/new/worker.kio's main: d=.:ro"),
        ]
    );
    // Tests stay the root's.
    assert!(out.contains("  tested       new: 4 passed, 0 failed\n"), "{out}");
    let identity = Sides::section(&out, "Identity");
    let hash = |text: &str| lang::digest::to_hex(&lang::digest::sha256(text.as_bytes()));
    for line in [
        format!("  new {dir}/new/lib.kio sha256:{}", hash(LIB)),
        format!("  new {dir}/new/worker.kio sha256:{}", hash(WORKER)),
        format!("  worker.kio: entry grants {w} sha256:{}", hash("--dir d=.:ro\n")),
    ] {
        assert!(identity.contains(&line), "{line}\n{out}");
    }
    // OLD has no file there: absent, and checked to stay so.
    assert!(
        identity.contains(&format!("  old {dir}/old/worker.kio absent")),
        "{out}"
    );
    let (_, out, _) = s.review(&["--entry", "worker.kio", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let e = &v["authority"]["entries"][0];
    assert_eq!(e["entry"], "worker.kio");
    assert_eq!(e["new_entry"], true);
    assert_eq!(e["grants"], serde_json::json!(["+ d: Dir (read)"]));
    // A grants file that does not bind is an error: exit 1.
    let w = write(root, "w.grants", "--dir e=.\n");
    let (code, out, _) = s.review(&["--entry", "worker.kio", "--entry-grants", &w]);
    assert_eq!(code, 1, "{out}");
    let authority = Sides::section(&out, "Authority");
    assert_eq!(
        authority[4],
        format!("  error        worker.kio: --entry-grants {w} does not bind to {dir}/new/worker.kio's main:")
    );
    assert!(authority.iter().any(|l| l.contains("hint: ")), "{out}");
}

/// R1a3: an entry on both sides is diffed like the root, its lines after its name.
#[test]
fn an_entry_gaining_a_grant() {
    let s = Sides::new(NEW, NEW);
    let root = s.dir.path();
    for side in ["old", "new"] {
        write(root, &format!("{side}/lib.kio"), LIB);
    }
    write(root, "old/worker.kio", WORKER);
    let gains = "mod lib;\n\nfn main(d: Dir, h: Http) -> Result<(), IoErr> { let _ = h.get(\"/\"); lib::touch(d) }\n";
    write(root, "new/worker.kio", gains);
    let (code, out, err) = s.review(&["--entry", "./worker.kio"]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        [
            "  enforced     no change in authority",
            "  enforced     ./worker.kio: + h: Http (get)",
            "  derived      ./worker.kio:   get in main",
        ]
    );
    assert_eq!(
        Sides::section(&out, "Not checked")[..3],
        [
            "  not checked  d: Dir write: what is written or removed",
            "  not checked  ./worker.kio: h: Http get: the paths, headers and bodies sent",
            "  not checked  ./worker.kio: main: h (changed)",
        ],
        "{out}"
    );
    // The same entry on both sides: no line but "no change".
    write(root, "new/worker.kio", WORKER);
    let (code, out, _) = s.review(&["--entry", "worker.kio"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("  enforced     worker.kio: no change in authority\n"),
        "{out}"
    );
    // An entry that does not check is an error: exit 1; the root's tests still run.
    write(root, "new/worker.kio", "fn main() -> Int { \"x\" }\n");
    let (code, out, _) = s.review(&["--entry", "worker.kio"]);
    assert_eq!(code, 1, "{out}");
    let dir = root.to_str().unwrap();
    assert!(
        out.contains(&format!(
            "  error        worker.kio: the new program {dir}/new/worker.kio does not check\n"
        )),
        "{out}"
    );
    assert!(out.contains("  tested       new: 4 passed, 0 failed\n"), "{out}");
}

/// R1a3: `--entry` names the same place on both sides, inside each.
#[test]
fn entry_usage() {
    let s = Sides::new(NEW, NEW);
    write(s.dir.path(), "worker.kio", WORKER);
    let abs = format!("{}/worker.kio", s.dir.path().display());
    for bad in ["../worker.kio", abs.as_str(), "", "a\u{1b}[2J.kio", "x/../worker.kio"] {
        let (code, out, err) = s.review(&["--entry", bad]);
        assert_eq!((code, out.as_str()), (2, ""), "{bad}: {err}");
        assert!(
            err.contains("not a path inside the program's directory"),
            "{bad}: {err}"
        );
        assert!(err.contains("hint: give a relative path without `..`"), "{err}");
        assert!(!err.contains('\u{1b}'), "{err:?}");
    }
    let (code, _, err) = s.review(&["--entry", "nope.kio"]);
    assert_eq!(code, 2);
    assert!(err.contains("new/nope.kio does not exist"), "{err}");
    assert!(err.contains("hint: the path is relative to the directory of"), "{err}");
    let (code, _, err) = s.review(&["--entry-grants", "w.grants"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("hint: `--entry worker.kio --entry-grants worker.grants`"),
        "{err}"
    );
    let (code, _, err) = s.review(&["--entry", "app.kio"]);
    assert_eq!(code, 2);
    assert!(err.contains("given twice, or it is the root"), "{err}");
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
    let (code, _, err) = s.review(&["--cases", "2"]);
    assert_eq!(code, 2);
    assert!(err.contains("usage: kio review"), "{err}");
}

/// R1a4: a test on both sides whose text changed is listed; whitespace alone is no change.
#[test]
fn a_changed_test() {
    let new = NEW
        .replace(
            "fn kept() { assert_eq(count(3), 3); }",
            "fn kept()  {\n    assert_eq(count(3),  3);\n}",
        )
        .replace("count(2), 1", "count(2), 1); assert_eq(count(1), 0");
    let s = Sides::new(NEW, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    let changed: Vec<&String> = tests.iter().filter(|l| l.contains("changed:")).collect();
    assert_eq!(changed, ["  not checked  changed: fresh"], "{out}");
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["tests"]["changed"], serde_json::json!(["fresh"]));
}

/// A program whose helper `save` uses `d`.
const SAVES: &str = r#"fn save(d: Dir) -> Result<(), IoErr> { let _ = d.read("a")?; Ok(()) }

fn shown(r: Dir) -> Result<(), IoErr> { let _ = r.list(".")?; Ok(()) }

fn main(d: Dir, r: Dir) -> Result<(), IoErr> { save(d)?; shown(r) }
"#;

/// R1a4: under a grant line, each gained method and the functions that newly use it; the Not
/// checked line those functions reach is marked when their text changed.
#[test]
fn where_a_gained_method_is_used() {
    let new = SAVES.replace("Ok(()) }\n\nfn shown", "d.write(\"b\", \"x\".to_bytes()) }\n\nfn shown");
    let s = Sides::new(SAVES, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        ["  enforced     ~ d: Dir (+write)", "  derived        write in save"],
        "{out}"
    );
    assert_eq!(
        Sides::section(&out, "Not checked"),
        [
            "  not checked  d: Dir write: what is written or removed",
            "  not checked  save: d (changed)",
            SAME_AUTHOR
        ]
    );
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["authority"]["gained"],
        serde_json::json!([{"grant": "d", "method": "write", "functions": ["save"]}])
    );
    assert_eq!(v["not_checked"]["methods"][0]["changed"], serde_json::json!(["save"]));
    // `main` changed, `save` did not: the line is unmarked.
    let moved = SAVES.replace("save(d)?; shown(r)", "save(d)?;\n    shown(r)");
    let s = Sides::new(
        &new,
        &moved.replace("Ok(()) }\n\nfn shown", "d.write(\"b\", \"x\".to_bytes()) }\n\nfn shown"),
    );
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Not checked")[0],
        "  not checked  d: Dir write: what is written or removed"
    );
}

/// R1a4: the grants file is part of the change: its flags against the file at the same path
/// beside OLD's program, which Identity hashes.
#[test]
fn the_grants_file_is_part_of_the_change() {
    let s = Sides::new(SAVES, SAVES);
    let root = s.dir.path();
    std::fs::create_dir(root.join("new/rejects")).unwrap();
    let old_g = write(root, "old/app.grants", "--dir d=.\n# reports\n--dir r=.:ro\n");
    let g = write(root, "new/app.grants", "--dir d=.\n--dir r=rejects\n");
    let (code, out, err) = s.review(&["--grants", &g]);
    assert_eq!(code, 0, "{out}{err}");
    let authority = Sides::section(&out, "Authority");
    assert_eq!(
        authority[1..],
        [
            "  enforced     app.grants: + --dir r=rejects".to_string(),
            "  enforced     app.grants: - --dir r=.:ro".into(),
            format!("  enforced     --grants {g} binds {}'s main: d=., r=rejects", s.new),
        ],
        "{out}"
    );
    let hash = |b: &str| lang::digest::to_hex(&lang::digest::sha256(b.as_bytes()));
    let line = format!(
        "  old grants {old_g} sha256:{}",
        hash("--dir d=.\n# reports\n--dir r=.:ro\n")
    );
    assert!(Sides::section(&out, "Identity").contains(&line), "{out}");
    let (_, out, _) = s.review(&["--grants", &g, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["authority"]["grants_file"]["diff"],
        serde_json::json!(["+ --dir r=rejects", "- --dir r=.:ro"])
    );
    assert_eq!(v["identity"]["old_grants"]["path"], old_g.as_str());
    // The same flags (comments aside): unchanged.
    write(root, "old/app.grants", "# all\n--dir d=.\n--dir r=rejects\n");
    let (_, out, _) = s.review(&["--grants", &g]);
    assert!(out.contains("  enforced     app.grants: unchanged\n"), "{out}");
    // None beside OLD: a new file.
    std::fs::remove_file(root.join("old/app.grants")).unwrap();
    let (_, out, _) = s.review(&["--grants", &g]);
    assert!(out.contains("  enforced     app.grants: new file\n"), "{out}");
    // Its text reaches the page escaped.
    write(root, "old/app.grants", "--dir d=.\n--dir r=x\u{1b}[2J\n");
    let (_, out, _) = s.review(&["--grants", &g]);
    assert!(
        out.contains("  enforced     app.grants: - --dir r=x\\u{1b}[2J\n"),
        "{out}"
    );
}

/// R1a4: every test that failed in NEW is counted, an added one and a property included.
#[test]
fn every_failed_test_is_counted() {
    let new = NEW
        .replace(
            r#"assert(count(n % 10) >= 0, "non-negative")"#,
            r#"assert(n < 5, "small")"#,
        )
        .replace("count(2), 1", "count(2), 2");
    let s = Sides::new(OLD, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 1, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Resources")[0],
        "  measured     2 tests failed in new; left out of Resources",
        "{out}"
    );
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["resources"]["failed"], 2);
}

/// Review: OLD's tree may be anyone's. A grants file beside OLD's program that is a symlink, a
/// device, a FIFO, a hard link or under a symlinked directory is not read: one line says so, and
/// neither the page nor `--json` shows a hash or a line of what it points at.
#[test]
fn an_old_grants_file_that_is_not_a_regular_file_inside_its_directory() {
    let s = Sides::new(SAVES, SAVES);
    let root = s.dir.path();
    let secret = write(root, "secret", "--dir d=/etc\n");
    let hash = lang::digest::to_hex(&lang::digest::sha256(b"--dir d=/etc\n"));
    let g = write(root, "new/app.grants", "--dir d=.\n--dir r=.\n");
    let why = "app.grants: the base's app.grants is not a regular file inside its directory; not compared";
    let check = |what: &str| {
        let (code, out, err) = s.review(&["--grants", &g]);
        assert_eq!(code, 0, "{what}: {out}{err}");
        assert_eq!(
            Sides::section(&out, "Authority")[1],
            format!("  not checked  {why}"),
            "{what}: {out}"
        );
        let (_, json, _) = s.review(&["--grants", &g, "--json"]);
        for text in [&out, &json] {
            assert!(!text.contains(&hash) && !text.contains("/etc"), "{what}: {text}");
        }
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["identity"]["old_grants"], serde_json::Value::Null);
        assert_eq!(
            v["authority"]["grants_file"]["not_compared"].as_str(),
            why.strip_prefix("app.grants: ")
        );
    };
    let old_g = root.join("old/app.grants");
    std::os::unix::fs::symlink(&secret, &old_g).unwrap();
    check("symlink");
    std::fs::remove_file(&old_g).unwrap();
    std::os::unix::fs::symlink("/dev/zero", &old_g).unwrap();
    check("device");
    std::fs::remove_file(&old_g).unwrap();
    assert!(Command::new("mkfifo").arg(&old_g).status().unwrap().success());
    check("fifo");
    std::fs::remove_file(&old_g).unwrap();
    // A hard link could be to any file on the same filesystem.
    std::fs::hard_link(&secret, &old_g).unwrap();
    check("hard link");
    std::fs::remove_file(&old_g).unwrap();
    // A directory on the way that is a symlink out of OLD's tree.
    std::fs::create_dir(root.join("new/conf")).unwrap();
    let g = write(root, "new/conf/app.grants", "--dir d=.\n--dir r=.\n");
    std::fs::create_dir(root.join("elsewhere")).unwrap();
    write(root, "elsewhere/app.grants", "--dir d=/etc\n");
    std::os::unix::fs::symlink(root.join("elsewhere"), root.join("old/conf")).unwrap();
    let (code, out, err) = s.review(&["--grants", &g]);
    assert_eq!(code, 0, "{out}{err}");
    let line = "  not checked  conf/app.grants: the base's conf/app.grants is not a regular file inside its directory; not compared\n";
    assert!(out.contains(line), "{out}");
    assert!(!out.contains(&hash) && !out.contains("/etc"), "{out}");
}

/// Review: an OLD grants file over 64 KiB is not compared; a NEW one over 1 MiB, or one that is
/// not a regular file, is refused, as `kio run --grants` refuses one over 1 MiB.
#[test]
fn grants_files_have_a_size_cap() {
    let s = Sides::new(SAVES, SAVES);
    let root = s.dir.path();
    let g = write(root, "new/app.grants", "--dir d=.\n--dir r=.\n");
    write(root, "old/app.grants", &format!("--dir d=.\n{}", "#\n".repeat(40_000)));
    let (code, out, err) = s.review(&["--grants", &g]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority")[1],
        "  not checked  app.grants: the base's app.grants is over 64 KiB; not compared"
    );
    assert!(!out.contains("old grants"), "{out}");
    let big = write(root, "new/big.grants", &format!("--dir d=.\n{}", "#\n".repeat(600_000)));
    let (code, _, err) = s.review(&["--grants", &big]);
    assert_eq!(code, 2);
    assert!(err.contains(&format!("--grants {big}: over 1 MiB\n  hint: ")), "{err}");
    let (code, _, err) = s.review(&["--test-grants", "/dev/zero"]);
    assert_eq!(code, 2);
    assert!(err.contains("--test-grants /dev/zero: not a regular file"), "{err}");
    let (code, _, err) = kio(&root.join("tmp"), &["run", &s.new, "--grants", &big]);
    assert_eq!(code, 2);
    assert!(err.contains(&format!("--grants {big}: over 1 MiB\n  hint: ")), "{err}");
}

/// Review: when OLD's program sits in NEW's directory, the grants file beside it is NEW's own:
/// not compared, never `unchanged`.
#[test]
fn a_base_in_the_same_directory() {
    let s = Sides::new(SAVES, SAVES);
    let root = s.dir.path();
    let old = write(root, "new/app.old.kio", SAVES);
    let g = write(root, "new/app.grants", "--dir d=.\n--dir r=.\n");
    let tmp = root.join("tmp");
    let (code, out, err) = kio(&tmp, &["review", &s.new, "--base", &old, "--grants", &g]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority")[1],
        "  not checked  app.grants: the base shares this directory; not compared"
    );
}

/// Review: a test whose own text is unchanged but that reaches a changed function is changed;
/// whitespace inside a string literal is a change, outside one and in comments it is not.
#[test]
fn a_test_reaching_a_changed_function() {
    let new = OLD
        .replace("s += i; i += 1;", "s += i;\n    // step\n    i += 1;")
        .replace("fn main(d: Dir)", "fn main(d: Dir)  ");
    let s = Sides::new(OLD, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(!out.contains("changed:"), "{out}");
    let new = OLD
        .replace("s += i; i += 1;", "s += i + 0; i += 1;")
        .replace("\"non-negative\"", "\"non- negative\"");
    let s = Sides::new(OLD, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let changed: Vec<String> = (Sides::section(&out, "Tests").into_iter())
        .filter(|l| l.contains("changed:"))
        .collect();
    assert_eq!(
        changed,
        [
            "  not checked  changed: counts (reaches changed count)",
            "  not checked  changed: gone (reaches changed count)",
            "  not checked  changed: kept (reaches changed count)",
            "  not checked  changed: small (reaches changed count)",
        ],
        "{out}"
    );
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["tests"]["changed"],
        serde_json::json!(["counts", "gone", "kept", "small"])
    );
}

/// A program whose writer takes its path from `target`.
const TARGETS: &str = r#"fn target() -> Str { "out.txt" }

fn save(d: Dir) -> Result<(), IoErr> { d.write(target(), "x".to_bytes()) }

fn main(d: Dir) -> Result<(), IoErr> { save(d) }
"#;

/// Review: the Not checked line of a method is marked when a function using it reaches a changed
/// function, though its own text is unchanged.
#[test]
fn a_changed_path_function_marks_the_writer() {
    let s = Sides::new(TARGETS, &TARGETS.replace("\"out.txt\"", "\"../out.txt\""));
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Not checked")[..2],
        [
            "  not checked  d: Dir write: what is written or removed",
            "  not checked  target: d (changed)",
        ],
        "{out}"
    );
}

/// Review: with two grants of one type, a function is listed under the grant passed to it only.
#[test]
fn two_grants_of_one_type() {
    let old = r#"fn wa(a: Dir) -> Result<(), IoErr> { let _ = a.read("x")?; Ok(()) }

fn wb(b: Dir) -> Result<(), IoErr> { b.write("y", "y".to_bytes()) }

fn main(a: Dir, b: Dir) -> Result<(), IoErr> { wa(a)?; wb(b) }
"#;
    let new = old.replace("let _ = a.read(\"x\")?; Ok(())", "a.write(\"x\", \"x\".to_bytes())");
    let s = Sides::new(old, &new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        ["  enforced     ~ a: Dir (+write -read)", "  derived        write in wa"],
        "{out}"
    );
    assert_eq!(
        Sides::section(&out, "Not checked")[..3],
        [
            "  not checked  a: Dir write: what is written or removed",
            "  not checked  b: Dir write: what is written or removed",
            "  not checked  wa: a (changed)",
        ],
        "{out}"
    );
}

/// Review: an error's file name reaches the page escaped.
#[test]
fn an_error_path_is_escaped() {
    let s = Sides::new(SAVES, SAVES);
    let dir = s.dir.path().join("n\u{202e}ew");
    std::fs::create_dir(&dir).unwrap();
    let new = write(&dir, "app.kio", "fn main() -> Int { nope }\n");
    let (code, out, err) = kio(&s.dir.path().join("tmp"), &["review", &new, "--base", &s.old]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(!out.contains('\u{202e}'), "{out}");
    assert!(out.contains("n\\u{202e}ew/app.kio:1:"), "{out}");
}

/// R1a5: a changed function no test of the root's reaches is named, per program. An entry's
/// function counts as reached when the root's tests reach one of its key with the same tokens.
#[test]
fn a_changed_function_no_test_reaches() {
    let touches = format!("{NEW}\nmod lib;\n\n#[test]\nfn touches(d: Dir) {{ let _ = lib::touch(d); }}\n");
    let s = Sides::new(&touches, &touches);
    let root = s.dir.path();
    for side in ["old", "new"] {
        write(root, &format!("{side}/lib.kio"), LIB);
    }
    write(root, "new/worker.kio", WORKER);
    std::fs::create_dir(root.join("new/sub")).unwrap();
    write(root, "new/sub/worker.kio", WORKER);
    write(root, "new/sub/lib.kio", &LIB.replace("\"a\"", "\"b\""));
    let entries = ["--entry", "worker.kio", "--entry", "sub/worker.kio"];
    let (code, out, err) = s.review(&entries);
    assert_eq!(code, 3, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    let unreached: Vec<&String> = tests.iter().filter(|l| l.contains("reached by no test")).collect();
    assert_eq!(
        unreached,
        [
            "  not checked  changed, reached by no test: worker.kio: main",
            "  not checked  changed, reached by no test: sub/worker.kio: lib::touch, main",
        ],
        "{out}"
    );
    let (_, out, _) = s.review(&[&entries[..], &["--json"]].concat());
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["tests"]["unreached_changed"],
        serde_json::json!([
            {"entry": "worker.kio", "function": "main"},
            {"entry": "sub/worker.kio", "function": "lib::touch"},
            {"entry": "sub/worker.kio", "function": "main"},
        ])
    );
    // The root's changed `main` (no test calls it): no prefix.
    let s = Sides::new(OLD, NEW);
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["tests"]["unreached_changed"],
        serde_json::json!([{"entry": null, "function": "main"}])
    );
}

/// R1a5: a dependency added or changed exits 3, its code not being on the page (#280); one
/// removed does not.
#[test]
fn a_changed_dependency_exits_3() {
    let app = "mod web;\n\nfn main(d: Dir) -> Result<Int, IoErr> { web::look(d) }\n";
    let v1 = "pub fn look(d: Dir) -> Result<Int, IoErr> { Ok(d.list(\"\")?.len()) }\n";
    let v2 = "pub fn look(d: Dir) -> Result<Int, IoErr> { Ok(d.list(\"\")?.len() + 1) }\n";
    let s = Sides::new(app, app);
    let root = s.dir.path();
    let sha = |t: &str| lang::digest::to_hex(&lang::digest::sha256(t.as_bytes()));
    let pin = |lib: &str| format!("web lib/web.kio sha256:{}\n", sha(lib));
    for (side, lib) in [("old", v1), ("new", v2)] {
        std::fs::create_dir(root.join(side).join("lib")).unwrap();
        write(root, &format!("{side}/lib/web.kio"), lib);
        write(root, &format!("{side}/kio.deps"), &pin(lib));
    }
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        ["  enforced     no change in authority"]
    );
    let line = format!(
        "  not checked  dependency web changed (sha256:{}.. -> sha256:{}..): its code is not shown; its authority is above",
        &sha(v1)[..12],
        &sha(v2)[..12]
    );
    assert!(Sides::section(&out, "Not checked").contains(&line), "{out}");
    // Added: OLD has the same file as a module of its own.
    std::fs::remove_file(root.join("old/kio.deps")).unwrap();
    write(root, "old/web.kio", v2);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    assert!(
        out.contains(&format!("dependency web added (sha256:{}..)", &sha(v2)[..12])),
        "{out}"
    );
    // Removed: NEW has it as a module of its own, OLD as a dependency.
    std::fs::remove_file(root.join("new/kio.deps")).unwrap();
    write(root, "new/web.kio", v2);
    std::fs::remove_file(root.join("old/web.kio")).unwrap();
    write(root, "old/lib/web.kio", v2);
    write(root, "old/kio.deps", &pin(v2));
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("  not checked  dependency web removed ("), "{out}");
}

/// R1a5: the binds line lists what the grants file bound, each part escaped and, when it holds a
/// separator, quoted; a secret by its source only.
#[test]
fn the_binds_line_lists_what_it_bound() {
    let app = r#"fn main(db: Db, hook: Http, key: Hmac, tok: Secret, d: Dir, r: Dir) -> Result<(), Str> {
  let _ = hook.send("POST", "/", [], "x".to_bytes(), 1000);
  let _ = db.query("SELECT 1", []);
  let _ = key.verify("x".to_bytes(), "y".to_bytes());
  let _ = tok.matches("x");
  let _ = d.read("a");
  let _ = r.read("a");
  Ok(())
}
"#;
    let s = Sides::new(app, app);
    let grants = "--db db=var/inv.db:ro\n--http hook=http://127.0.0.1:9099\n--http-methods hook=POST\n\
        --http-header hook=Authorization:env:HOOK_AUTH\n--hmac key=file:keys/k:verify\n\
        --secret tok=env:R1A5_TOKEN\n--dir d=we ird,x\n--dir r=a\u{202e}b\n--fuel 1000000\n";
    let g = write(s.dir.path(), "new/app.grants", grants);
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["review", &s.new, "--base", &s.old, "--grants", &g])
        .env("TMPDIR", s.dir.path().join("tmp"))
        .env("R1A5_TOKEN", "hunter2-value")
        .env("HOOK_AUTH", "Bearer hunter3-value")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{text}{err}");
    let binds = format!(
        "  enforced     --grants {g} binds {}'s main: db=var/inv.db:ro, hook=http://127.0.0.1:9099 \
         (POST; Authorization:env:HOOK_AUTH), key=file:keys/k:verify, tok=env:R1A5_TOKEN, \"d=we ird,x\", \
         r=a\\u{{202e}}b",
        s.new
    );
    assert!(Sides::section(&text, "Authority").contains(&binds), "{text}");
    assert!(!text.contains("hunter") && !text.contains('\u{202e}'), "{text}");
}

/// R1a5: a test only in NEW has its fuel and peak memory under Resources, no old value.
#[test]
fn an_added_tests_resources() {
    let old = "fn main() {}\n\n#[test]\nfn one() { assert_eq(1, 1); }\n";
    let new = format!("{old}\n#[test]\nfn two() {{ assert_eq(2, 2); }}\n");
    let s = Sides::new(old, &new);
    let (code, out, err) = s.review(&["--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let added = &v["resources"]["added"];
    assert_eq!(added.as_array().unwrap().len(), 1, "{out}");
    assert_eq!(added[0]["name"], "two");
    let fuel = added[0]["fuel"].as_u64().unwrap();
    let peak = added[0]["peak_memory"].as_u64().unwrap();
    let (_, out, _) = s.review(&[]);
    assert_eq!(
        Sides::section(&out, "Resources")[1],
        format!("  measured     added: two: fuel {fuel}, peak memory {} KB", peak >> 10),
        "{out}"
    );
}

/// Each program's `kio.deps` is compared on its own: an entry may pin a name the root pins too,
/// to another file.
#[test]
fn an_entrys_dependency_of_a_name_the_root_has() {
    let app = "mod web;\n\nfn main(d: Dir) -> Result<Int, IoErr> { web::look(d) }\n";
    let v1 = "pub fn look(d: Dir) -> Result<Int, IoErr> { Ok(d.list(\"\")?.len()) }\n";
    let v2 = "pub fn look(d: Dir) -> Result<Int, IoErr> { Ok(d.list(\"\")?.len() + 1) }\n";
    let s = Sides::new(app, app);
    let root = s.dir.path();
    let sha = |t: &str| lang::digest::to_hex(&lang::digest::sha256(t.as_bytes()));
    for (side, lib2) in [("old", v1), ("new", v2)] {
        for d in ["lib", "lib2", "sub"] {
            std::fs::create_dir(root.join(side).join(d)).unwrap();
        }
        write(root, &format!("{side}/lib/web.kio"), v1);
        write(
            root,
            &format!("{side}/kio.deps"),
            &format!("web lib/web.kio sha256:{}\n", sha(v1)),
        );
        write(root, &format!("{side}/lib2/web.kio"), lib2);
        write(root, &format!("{side}/sub/worker.kio"), app);
        let pin = format!("web ../lib2/web.kio sha256:{}\n", sha(lib2));
        write(root, &format!("{side}/sub/kio.deps"), &pin);
    }
    let (code, out, err) = s.review(&["--entry", "sub/worker.kio"]);
    assert_eq!(code, 3, "{out}{err}");
    let line = format!(
        "  not checked  sub/worker.kio: dependency web changed (sha256:{}.. -> sha256:{}..): its code is not shown; its authority is above",
        &sha(v1)[..12],
        &sha(v2)[..12]
    );
    assert!(Sides::section(&out, "Not checked").contains(&line), "{out}");
}

/// An entry's function is reached by a root test only if all it may run is the root's too: an
/// identical `deliver` calling a different `wipe` is not.
#[test]
fn an_entry_function_whose_callee_differs_from_the_roots() {
    let app =
        "fn wipe(d: Dir) -> Result<(), IoErr> { Ok(()) }\n\nfn deliver(d: Dir) -> Result<(), IoErr> { wipe(d) }\n\n\
        fn main(d: Dir) -> Result<(), IoErr> { deliver(d) }\n\n#[test]\nfn delivers(d: Dir) { let _ = deliver(d); }\n";
    let s = Sides::new(app, app);
    let root = s.dir.path();
    let wipe = "fn wipe(d: Dir) -> Result<(), IoErr> { d.remove(\"x\") }\n\nfn main(d: Dir) -> Result<(), IoErr> { deliver(d) }\n";
    write(
        root,
        "old/worker.kio",
        &format!("{wipe}\nfn deliver(d: Dir) -> Result<(), IoErr> {{ Ok(()) }}\n"),
    );
    write(
        root,
        "new/worker.kio",
        &format!("{wipe}\nfn deliver(d: Dir) -> Result<(), IoErr> {{ wipe(d) }}\n"),
    );
    let (code, out, err) = s.review(&["--entry", "worker.kio"]);
    assert_eq!(code, 3, "{out}{err}");
    let unreached: Vec<String> = (Sides::section(&out, "Tests").into_iter())
        .filter(|l| l.contains("reached by no test"))
        .collect();
    assert_eq!(
        unreached,
        ["  not checked  changed, reached by no test: worker.kio: deliver"],
        "{out}"
    );
}

/// The binds line is ASCII: a look-alike letter or comma is escaped, not shown; `--listen`,
/// `--host` and `--ws-origin` are listed, as they widen who may reach the program.
#[test]
fn the_binds_line_cannot_be_mistaken() {
    let app = "fn main(msg: WsMessage, conn: WsSend, d: Dir, e: Dir) -> Result<(), WsErr> { let _ = d.list(\".\"); let _ = e.list(\".\"); conn.text(msg.data().to_str()) }\n";
    let s = Sides::new(app, app);
    let grants = "--listen 127.0.0.1:8080\n--host app.example\n--ws-origin https://app.example\n\
        --dir d=./data:r\u{43e}\n--dir e=a\u{ff0c}\u{a0}f=b\\c\n--timeout 20\n";
    let g = write(s.dir.path(), "app.grants", grants);
    let t = write(s.dir.path(), "test.grants", "--timeout 20\n");
    let (code, out, err) = s.review(&["--grants", &g, "--test-grants", &t]);
    assert_eq!(code, 0, "{out}{err}");
    let binds = format!(
        "  enforced     --grants {g} binds {}'s main: --listen 127.0.0.1:8080, --host app.example, \
         --ws-origin https://app.example, d=./data:r\\u{{43e}}, e=a\\u{{ff0c}}\\u{{a0}}f=b\\\\c",
        s.new
    );
    assert!(Sides::section(&out, "Authority").contains(&binds), "{out}");
}

/// T4c: a program that posts to `hook`, and a test that retries against a fake.
const HOOKED: &str = r#"fn post(hook: Http) -> Result<Int, HttpErr> { Ok(hook.send("POST", "/v1/events", [], "x".to_bytes(), 0)?.status) }

fn main(hook: Http) -> Result<Int, HttpErr> { post(hook) }

#[test]
fn retries(hook: Http) -> Result<(), HttpErr> { assert_eq(post(hook)?, 503); assert_eq(post(hook)?, 200); Ok(()) }
"#;

const HOOK: &str = "POST /v1/events\n503\n---\nPOST /v1/events\n200\n";

fn sha(b: &[u8]) -> String {
    lang::digest::to_hex(&lang::digest::sha256(b))
}

/// Each fake of the test grants file is hashed in Identity, named on the grants line and has a
/// Not checked line. Its file name comes from the change's author, so it is shown as the binds
/// line shows a grant: ASCII only, quoted when it holds a separator.
#[test]
fn fakes_on_the_page() {
    let s = Sides::new(HOOKED, HOOKED);
    let up = "GET /up\n200\n";
    let hook = write(s.dir.path(), "new/hook.http", HOOK);
    let esc = write(s.dir.path(), "new/api\u{1b}[31m.http", up);
    // A Cyrillic `о`: reads as `hook.http`.
    let look = write(s.dir.path(), "new/h\u{43e}ok.http", up);
    // Would read as a second grant on the grants line.
    let sep = write(s.dir.path(), "new/hook.http), dest answered from dest.http (POST)", up);
    let (esc_s, look_s, sep_s) = (
        esc.replace('\u{1b}', "\\u{1b}"),
        look.replace('\u{43e}', "\\u{43e}"),
        format!("\"{sep}\""),
    );
    let t = write(
        s.dir.path(),
        "t.grants",
        &format!(
            "--http hook=fake:{hook}\n--http-methods hook=POST\n--http api=fake:{esc}\n--http look=fake:{look}\n--http dest=fake:{sep}\n"
        ),
    );
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.is_ascii(), "{out}");
    let tests = Sides::section(&out, "Tests");
    assert_eq!(
        tests[..3],
        [
            format!(
                "  tested       grants: {t} (hook answered from {hook}, api answered from {esc_s}, look answered from {look_s}, dest answered from {sep_s})"
            ),
            "  tested       old: 1 passed, 0 failed".to_string(),
            "  tested       new: 1 passed, 0 failed".to_string(),
        ],
        "{out}"
    );
    let not_checked = Sides::section(&out, "Not checked");
    let identity = Sides::section(&out, "Identity");
    let (h, u) = (sha(HOOK.as_bytes()), sha(up.as_bytes()));
    for (n, f, sum) in [
        ("hook", &hook, &h),
        ("api", &esc_s, &u),
        ("look", &look_s, &u),
        ("dest", &sep_s, &u),
    ] {
        let line = format!(
            "  not checked  {n}: Http: tests ran against {f}, not a server; the real endpoint's answers are untested"
        );
        assert!(not_checked.contains(&line), "{line}\n{out}");
        let line = format!("  test fake {f} sha256:{sum}");
        assert!(identity.contains(&line), "{line}\n{out}");
    }

    let (code, out, err) = s.review(&["--test-grants", &t, "--json"]);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let fakes = serde_json::json!([
        {"grant": "hook", "path": hook}, {"grant": "api", "path": esc},
        {"grant": "look", "path": look}, {"grant": "dest", "path": sep}
    ]);
    assert_eq!(v["tests"]["fakes"], fakes);
    assert_eq!(v["not_checked"]["fakes"], fakes);
    assert_eq!(
        v["identity"]["test_fakes"],
        serde_json::json!([
            {"path": hook, "sha256": h}, {"path": esc, "sha256": u},
            {"path": look, "sha256": u}, {"path": sep, "sha256": u}
        ])
    );
}

/// A test grants file's `--http` name must be an identifier; a fake FILE must be inside NEW's
/// directory, so the change shows it; a FILE `kio test` refuses is not hashed.
#[test]
fn fakes_refused() {
    let s = Sides::new(HOOKED, HOOKED);
    let up = write(s.dir.path(), "new/up.http", "GET /up\n200\n");
    let t = write(
        s.dir.path(),
        "t.grants",
        &format!("--http x: Http send GET: nothing is sent=fake:{up}\n"),
    );
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!((code, out.as_str()), (2, ""), "{err}");
    assert!(
        err.contains(&format!(
            "--test-grants {t}: --http x: Http send GET: nothing is sent=fake:{up}: `x: Http send GET: nothing is sent` is not a grant name"
        )),
        "{err}"
    );
    assert!(err.contains("hint: name the test's Http parameter"), "{err}");
    let (code, _, err) = kio(&s.dir.path().join("tmp"), &["test", &s.new, "--grants", &t]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("is not a grant name"), "{err}");

    let outside = write(s.dir.path(), "hook.http", HOOK);
    let t = write(s.dir.path(), "t.grants", &format!("--http hook=fake:{outside}\n"));
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!((code, out.as_str()), (2, ""), "{err}");
    assert!(
        err.contains(&format!(
            "--test-grants {t}: --http hook=fake:{outside}: outside {}'s directory, so the change does not show it\n  hint: keep the fake file beside the program it tests",
            s.new
        )),
        "{err}"
    );

    // `PUT` is not granted: `kio test` refuses the file, so no test ran on it and it has no hash.
    let put = write(s.dir.path(), "new/put.http", "PUT /x\n200\n");
    let t = write(s.dir.path(), "t.grants", &format!("--http hook=fake:{put}\n"));
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("--http hook grants only"), "{out}");
    assert!(!out.contains("test fake"), "{out}");
}

/// A file `name` below `dir`, `depth` levels down at most.
fn find(dir: &Path, name: &str, depth: u32) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let p = e.path();
        p.file_name().is_some_and(|n| n == name) || (depth > 0 && p.is_dir() && find(&p, name, depth - 1))
    })
}

/// FILE is read once: rewritten while OLD's tests run, NEW's still pass on the bytes read, and
/// Identity reports the change as an `error`, the name escaped.
#[test]
fn a_fake_changed_during_the_review() {
    let waits = "\n#[test]\nfn waits(d: Dir, watch: Dir) -> Result<(), IoErr> {\n    d.write(\"started\", \"\".to_bytes())?;\n    let mut i = 0;\n    while i < 100000 && !watch.exists(\"go\")? { i += 1; }\n    Ok(())\n}\n";
    let app = format!("{HOOKED}{waits}");
    let s = Sides::new(&app, &app);
    let hook = write(s.dir.path(), "new/hook\u{1b}.http", HOOK);
    let shown = hook.replace('\u{1b}', "\\u{1b}");
    let watch = s.dir.path().join("watch");
    std::fs::create_dir(&watch).unwrap();
    let t = write(
        s.dir.path(),
        "t.grants",
        &format!(
            "--http hook=fake:{hook}\n--http-methods hook=POST\n--dir watch={}:ro\n--calls 200000\n",
            watch.display()
        ),
    );
    let tmp = s.dir.path().join("tmp");
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["review", &s.new, "--base", &s.old, "--test-grants", &t])
        .env("TMPDIR", &tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while !find(&tmp, "started", 2) {
        assert!(
            child.try_wait().unwrap().is_none(),
            "the review ended before a test started"
        );
        assert!(start.elapsed().as_secs() < 60, "no test started");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::fs::write(&hook, "not a fake\n").unwrap();
    std::fs::write(watch.join("go"), "").unwrap();
    let r = child.wait_with_output().unwrap();
    let (out, err) = (String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr));
    assert_eq!(r.status.code(), Some(1), "{out}{err}");
    assert!(!out.contains('\u{1b}'), "{out}");
    let tests = Sides::section(&out, "Tests");
    assert!(
        tests.contains(&"  tested       new: 2 passed, 0 failed".to_string()),
        "{out}"
    );
    let identity = Sides::section(&out, "Identity");
    assert!(
        identity.contains(&format!("  test fake {shown} sha256:{}", sha(HOOK.as_bytes()))),
        "{out}"
    );
    assert!(
        identity.contains(&format!("  error        {shown} changed while the review ran")),
        "{out}"
    );
}

// R1b1: generated mutants of NEW's changed functions that `main` and a test reach.

const BARE: &str = "fn main() -> Int { 0 }\n";

/// `small` is pinned by a test, `big` only touched by one; `main` changed and no test reaches it.
const PINNED: &str = r#"fn small(n: Int) -> Bool { n < 10 }

fn big(n: Int) -> Bool { n >= 100 }

fn main() -> Int { if small(3) && big(200) { 1 } else { 0 } }

#[test]
fn pins_small() { assert(small(9), "9"); assert(!small(10), "10"); }

#[test]
fn touches_big() { assert(big(500) == true, "500"); }
"#;

/// `v`'s mutant rows without their fuel, which moves with the compiler.
fn mutant_rows(v: &serde_json::Value) -> Vec<serde_json::Value> {
    let rows = v["tests"]["mutants"]["rows"].as_array().unwrap().clone();
    rows.into_iter()
        .map(|mut r| {
            r.as_object_mut().unwrap().remove("fuel");
            r
        })
        .collect()
}

/// The caret line under `token` in page line `line`: at `token`'s first place in `context`, at
/// the last place `context` occurs in `line` (the snippet ends the line).
fn caret(line: &str, context: &str, token: &str) -> String {
    let at = line.rfind(context).unwrap() + context.find(token).unwrap();
    format!("{}{}", " ".repeat(at), "^".repeat(token.chars().count()))
}

/// A comparison the tests pin is killed, the killer named; one they do not survives with its
/// snippet, and the exit code stays 0. No mutant is made in `main` (no test reaches it) or in a
/// test.
#[test]
fn mutants_killed_and_survived() {
    let s = Sides::new(BARE, PINNED);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    let at = |l: &str| l.replace("{new}", &s.new);
    let flipped =
        at("  not checked  survived: generated: {new}:3:28: big: `>=` -> `>` in `...big(n: Int) -> Bool { n >= 100 }`");
    let raised =
        at("  not checked  survived: generated: {new}:3:31: big: `100` -> `101` in `...(n: Int) -> Bool { n >= 100 }`");
    assert_eq!(
        tests[5..],
        [
            "  not checked  changed, reached by no test: main",
            "  tested       mutants: 2 of 4 killed (generated at seed 1: 4 drawn of 4 sites, 0 did not check)",
            "  tested       killed by: pins_small 2",
            &flipped,
            &caret(&flipped, "n >= 100 }", ">="),
            &raised,
            &caret(&raised, ">= 100 }", "100"),
        ],
        "{out}"
    );
    let (code, out, _) = s.review(&["--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let mut m = v["tests"]["mutants"].clone();
    m.as_object_mut().unwrap().remove("rows");
    assert_eq!(
        m,
        serde_json::json!({"seed": 1, "requested": 20, "sites": 4, "drawn": 4, "made": 4, "did_not_check": 0, "error": null,
            "skipped": [], "constants": [], "unmutated": [], "mutations": null})
    );
    let row = |func: &str, (line, col): (u32, u32), from: &str, to: &str, around: &str, killed: Option<&str>| {
        let edit = format!("`{from}` -> `{to}` in `");
        serde_json::json!({
            "source": "generated", "written": false, "function": func, "file": s.new, "line": line, "col": col,
            "from": from, "to": to, "description": format!("{edit}{around}`"), "caret": edit.len() + around.rfind(from).unwrap(),
            "killed_by": killed, "stopped_by": null, "discarded_by": null, "how": killed.map(|_| "failed"),
            "tests_run": 1,
        })
    };
    assert_eq!(
        mutant_rows(&v),
        [
            row(
                "small",
                (1, 30),
                "<",
                "<=",
                "...all(n: Int) -> Bool { n < 10 }",
                Some("pins_small")
            ),
            row(
                "small",
                (1, 32),
                "10",
                "11",
                "...l(n: Int) -> Bool { n < 10 }",
                Some("pins_small")
            ),
            row("big", (3, 28), ">=", ">", "...big(n: Int) -> Bool { n >= 100 }", None),
            row("big", (3, 31), "100", "101", "...(n: Int) -> Bool { n >= 100 }", None),
        ]
    );
}

/// The mutant that drops `i += 1;` loops.
const COUNTED: &str = r#"fn count(n: Int) -> Int { let mut i = 0; let mut s = 0; while i < n { s += i; i += 1; } s }

fn main() -> Int { count(3) }

#[test]
fn counts() { assert_eq(count(100), 4950); }
"#;

/// A mutant that loops is killed out of fuel, at ten times what the test used on NEW.
#[test]
fn a_mutant_that_loops_runs_out_of_fuel() {
    let s = Sides::new(BARE, COUNTED);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    let at = |l: &str| l.replace("{new}", &s.new);
    let survived = at("  not checked  survived: generated: {new}:1:39: count: `0` -> `1` in `...t) -> Int { let mut i = 0; let mut s = 0; while i...`");
    assert_eq!(
        tests[5..],
        [
            "  tested       mutants: 5 of 6 killed (generated at seed 1: 6 drawn of 6 sites, 0 did not check)",
            "  tested       killed by: counts 5 (1 out of fuel)",
            &survived,
            &caret(&survived, "i = 0; let mut s", "0"),
        ],
        "{out}"
    );
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let rows = v["tests"]["mutants"]["rows"].as_array().unwrap();
    let looped = rows.iter().find(|r| r["from"] == "i += 1;").unwrap();
    assert_eq!(
        looped["description"],
        "`i += 1;` dropped in `...; while i < n { s += i; i += 1; } s }`"
    );
    assert_eq!([&looped["killed_by"], &looped["how"]], ["counts", "out of fuel"]);
    let fuel = v["resources"]["added"][0]["fuel"].as_u64().unwrap();
    assert_eq!(looped["fuel"].as_u64(), Some(10 * fuel), "{looped}");
}

/// One seed gives one page; another draws other mutants, and Identity names it.
#[test]
fn mutants_are_drawn_by_the_seed() {
    let s = Sides::new(BARE, COUNTED);
    let (_, one, _) = s.review(&["--mutants", "2"]);
    let (_, again, _) = s.review(&["--mutants", "2"]);
    assert_eq!(one, again);
    let (_, two, _) = s.review(&["--mutants", "2", "--seed", "2"]);
    let at = |l: &str| l.replace("{new}", &s.new);
    let mutants = |out: &str| -> Vec<String> {
        let tests = Sides::section(out, "Tests");
        tests.into_iter().skip_while(|l| !l.contains("mutants:")).collect()
    };
    let survived = at("  not checked  survived: generated: {new}:1:39: count: `0` -> `1` in `...t) -> Int { let mut i = 0; let mut s = 0; while i...`");
    assert_eq!(
        mutants(&one),
        [
            "  tested       mutants: 1 of 2 killed (generated at seed 1: 2 drawn of 6 sites, 0 did not check)",
            "  tested       killed by: counts 1",
            &survived,
            &caret(&survived, "i = 0; let mut s", "0"),
        ],
        "{one}"
    );
    assert_ne!(mutants(&one)[1..], mutants(&two)[1..], "{two}");
    assert!(mutants(&two)[0].contains("at seed 2: 2 drawn of 6 sites"), "{two}");
    assert!(
        Sides::section(&two, "Identity").contains(&"  test seed 2".to_string()),
        "{two}"
    );
}

/// `--mutants 0` runs none and prints nothing; a bad `--mutants` or `--seed` is a usage error.
#[test]
fn mutants_off_and_usage() {
    let s = Sides::new(BARE, PINNED);
    let (code, out, _) = s.review(&["--mutants", "0"]);
    assert_eq!(code, 0);
    assert!(!out.contains("mutants:") && !out.contains("survived:"), "{out}");
    let (_, out, _) = s.review(&["--mutants", "0", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["tests"]["mutants"], serde_json::Value::Null);
    let (code, _, err) = s.review(&["--mutants", "501"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("--mutants 501: expected 0 to 500\n  hint: `--mutants 0` runs none"),
        "{err}"
    );
    let (code, _, err) = s.review(&["--seed", "-1"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("--seed -1: expected a non-negative integer\n  hint: `--seed 1`, the default"),
        "{err}"
    );
}

/// A function that reaches `main`'s `Http`. `Ok` made an `Err` does not check.
const NOTIFIES: &str = r#"fn notify(h: Http, n: Int) -> Int {
    if n > 0 {
        match h.send("POST", "/n", [], "x".to_bytes(), 0) { Ok(r) => r.status, Err(_e) => 7 }
    } else { 0 }
}

fn main(h: Http) -> Int { notify(h, 1) }

#[test]
fn zero_sends_nothing(h: Http) { assert_eq(notify(h, 0), 0); }
"#;

/// Mutants run under the test grants' fake, never `--grants G`: G's host hears nothing.
#[test]
fn mutants_never_run_under_mains_grants() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let s = Sides::new(BARE, NOTIFIES);
    let hook = write(s.dir.path(), "new/hook.http", "POST /n\n200\n");
    let g = write(
        s.dir.path(),
        "new/app.grants",
        &format!("--http h=http://127.0.0.1:{port}\n--http-methods h=POST\n"),
    );
    let t = write(
        s.dir.path(),
        "t.grants",
        &format!("--http h=fake:{hook}\n--http-methods h=POST\n"),
    );
    let (code, out, err) = s.review(&["--grants", &g, "--test-grants", &t, "--mutants", "500"]);
    assert_eq!(code, 3, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    let mutants: Vec<&String> = (tests.iter())
        .filter(|l| l.contains("mutants:") || l.contains("killed by:"))
        .collect();
    assert_eq!(
        mutants,
        [
            "  tested       mutants: 2 of 8 killed (generated at seed 1: 9 drawn of 9 sites, 1 did not check)",
            "  tested       killed by: zero_sends_nothing 2",
        ],
        "{out}"
    );
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "a mutant reached main's host"
    );
}

/// The Tests lines from the first mutant line on.
fn mutant_lines(out: &str) -> Vec<String> {
    let tests = Sides::section(out, "Tests");
    let first = |l: &String| {
        ["mutants", "no mutant", "constants"]
            .iter()
            .any(|p| l[15..].starts_with(p))
    };
    tests.into_iter().skip_while(|l| !first(l)).collect()
}

/// A long function cannot take every draw: each function with sites gets one before any gets a
/// second, and one that gets none is named.
#[test]
fn padding_does_not_dilute_the_draw() {
    let pad = "x += 1; ".repeat(40);
    let new = format!(
        "fn pad(n: Int) -> Int {{ let mut x = n; {pad}x }}\n\nfn sensitive(n: Int) -> Bool {{ n > 5 }}\n\nfn main() -> Int {{ if sensitive(pad(0)) {{ 1 }} else {{ 0 }} }}\n\n#[test]\nfn pads() {{ assert_eq(pad(0), 40); }}\n\n#[test]\nfn touches() {{ assert(sensitive(100), \"big\"); }}\n"
    );
    let s = Sides::new(BARE, &new);
    let (code, out, err) = s.review(&["--mutants", "4"]);
    assert_eq!(code, 0, "{out}{err}");
    let at = |l: &str| l.replace("{new}", &s.new);
    let flipped = at(
        "  not checked  survived: generated: {new}:3:34: sensitive: `>` -> `>=` in `...ive(n: Int) -> Bool { n > 5 }`",
    );
    let raised =
        at("  not checked  survived: generated: {new}:3:36: sensitive: `5` -> `6` in `...e(n: Int) -> Bool { n > 5 }`");
    assert_eq!(
        mutant_lines(&out),
        [
            "  tested       mutants: 2 of 4 killed (generated at seed 1: 4 drawn of 82 sites, 0 did not check)",
            "  tested       killed by: pads 2",
            &flipped,
            &caret(&flipped, "n > 5 }", ">"),
            &raised,
            &caret(&raised, "> 5 }", "5"),
        ],
        "{out}"
    );
    let (_, out, _) = s.review(&["--mutants", "1"]);
    let lines = mutant_lines(&out);
    assert_eq!(lines.len(), 3, "{out}");
    assert!(
        lines[2] == "  not checked  no mutant drawn: pad (80 sites)"
            || lines[2] == "  not checked  no mutant drawn: sensitive (2 sites)",
        "{out}"
    );
}

/// A test holding a real host in T runs no mutant: the host could answer each run differently.
#[test]
fn tests_holding_a_real_host_run_no_mutant() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let s = Sides::new(BARE, NOTIFIES);
    let t = write(
        s.dir.path(),
        "t.grants",
        &format!("--http h=http://127.0.0.1:{port}\n--http-methods h=POST\n"),
    );
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 3, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    assert_eq!(
        tests[4..],
        [
            "  not checked  changed, reached by no test: main",
            "  not checked  mutants skipped zero_sends_nothing: it holds a real host (h), which can tell runs apart; use --http h=fake:FILE",
        ],
        "{out}"
    );
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "a mutant reached the test's host"
    );
    let (_, out, _) = s.review(&["--test-grants", &t, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["tests"]["mutants"]["skipped"],
        serde_json::json!([{"test": "zero_sends_nothing", "grant": "h", "holds": "a real host"}])
    );
}

/// T's `--fuel` at exactly what the test used: a mutant that costs more stops at T's limit, which
/// is not a kill.
#[test]
fn a_stop_at_ts_own_fuel_is_not_a_kill() {
    // The expected kills depend on the normal heap's fuel: the test heap's poisoned frees cost
    // more, so mutants stop at T's exact --fuel before their assertion fails.
    if std::env::var_os("KIO_TEST_HEAP").is_some() {
        return;
    }
    let s = Sides::new(BARE, COUNTED);
    let (_, out, _) = s.review(&["--json", "--mutants", "0"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let fuel = v["resources"]["added"][0]["fuel"].as_u64().unwrap();
    let t = write(s.dir.path(), "t.grants", &format!("--fuel {fuel}\n"));
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 0, "{out}{err}");
    let at = |l: &str| l.replace("{fuel}", &fuel.to_string()).replace("{new}", &s.new);
    let survived = at("  not checked  survived: generated: {new}:1:39: count: `0` -> `1` in `...t) -> Int { let mut i = 0; let mut s = 0; while i...`");
    let stopped = at("  not checked  stopped by T's --fuel {fuel} in counts: generated: {new}:1:79: count: `i += 1;` dropped in `...; while i < n { s += i; i += 1; } s }`");
    assert_eq!(
        mutant_lines(&out),
        [
            "  tested       mutants: 4 of 6 killed, 1 stopped by T's limits (generated at seed 1: 6 drawn of 6 sites, 0 did not check)",
            "  tested       killed by: counts 4",
            &survived,
            &caret(&survived, "i = 0; let mut s", "0"),
            &stopped,
            &caret(&stopped, "i += 1; } s }", "i += 1;"),
        ],
        "{out}"
    );
}

/// `LIMIT > n` is a comparison (the parser says so, not the case of a name), and the constant's
/// own literal is named as not mutated.
#[test]
fn a_comparison_after_a_constant_is_flipped() {
    let new = "const LIMIT: Int = 10;\n\nfn over(n: Int) -> Bool { LIMIT > n }\n\nfn main() -> Int { if over(3) { 1 } else { 0 } }\n\n#[test]\nfn three() { assert(over(3), \"3\"); }\n";
    let s = Sides::new(BARE, new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let at = |l: &str| l.replace("{new}", &s.new);
    let survived =
        at("  not checked  survived: generated: {new}:3:33: over: `>` -> `>=` in `...n: Int) -> Bool { LIMIT > n }`");
    assert_eq!(
        mutant_lines(&out),
        [
            "  tested       mutants: 0 of 1 killed (generated at seed 1: 1 drawn of 1 sites, 0 did not check)",
            &survived,
            &caret(&survived, "LIMIT > n }", ">"),
            "  not checked  constants are not mutated: LIMIT used by over",
        ],
        "{out}"
    );
}

/// R1c: the caret is under the edited token as the snippet shows it: tabs one space, a
/// non-ASCII character escaped, counted in characters.
#[test]
fn the_caret_marks_the_edited_token() {
    let new = "fn tag(n: Int) -> Str {\tlet s = \"\u{e9}\";\t\tif n > 5 { s } else { \"\" } }\n\nfn main() -> Str { tag(1) }\n\n#[test]\nfn big() { assert_eq(tag(100), \"\u{e9}\"); }\n";
    let s = Sides::new(BARE, new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let at = |l: &str| l.replace("{new}", &s.new);
    let flipped = at("  not checked  survived: generated: {new}:1:44: tag: `>` -> `>=` in `...tr { let s = \"\\u{e9}\"; if n > 5 { s } else { \"\" } }`");
    let raised = at("  not checked  survived: generated: {new}:1:46: tag: `5` -> `6` in `...{ let s = \"\\u{e9}\"; if n > 5 { s } else { \"\" } }`");
    assert_eq!(
        mutant_lines(&out),
        [
            "  tested       mutants: 1 of 3 killed (generated at seed 1: 3 drawn of 3 sites, 0 did not check)",
            "  tested       killed by: big 1",
            &flipped,
            &caret(&flipped, "n > 5 {", ">"),
            &raised,
            &caret(&raised, "> 5 {", "5"),
        ],
        "{out}"
    );
}

/// R1c: a changed function is named once in Not checked, with every grant it is marked under;
/// those marked under the same grants share a line. An entry the base lacks says so.
#[test]
fn a_changed_function_is_listed_once() {
    let old = r#"fn save(out: Dir, hook: Http) -> Result<(), Str> { out.write("a", "x".to_bytes()).map_err(|_| "out")?; let _ = hook.send("POST", "/", [], "x".to_bytes(), 0); Ok(()) }

fn note(out: Dir) -> Result<(), Str> { out.write("n", "n".to_bytes()).map_err(|_| "out") }

fn main(out: Dir, hook: Http) -> Result<(), Str> { save(out, hook)?; note(out) }
"#;
    let new = old.replace("\"x\"", "\"y\"").replace("\"n\".to", "\"m\".to");
    let s = Sides::new(old, &new);
    write(s.dir.path(), "new/worker.kio", &new);
    let (code, out, err) = s.review(&["--entry", "worker.kio"]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Not checked"),
        [
            "  not checked  out: Dir write: what is written or removed",
            "  not checked  hook: Http send POST: the paths, headers and bodies sent",
            "  not checked  note: out (changed)",
            "  not checked  save: out, hook (changed)",
            "  not checked  worker.kio: out: Dir write: what is written or removed",
            "  not checked  worker.kio: hook: Http send POST: the paths, headers and bodies sent",
            "  not checked  worker.kio: note: out (new entry)",
            "  not checked  worker.kio: save: out, hook (new entry)",
            SAME_AUTHOR,
        ],
        "{out}"
    );
}

/// R1c: a Resources row of a test that reaches no changed function says so: what moved it is
/// not the code it runs (here the type table `h: Http` brings).
#[test]
fn resources_on_unchanged_code() {
    let old = "fn f() -> Int { 1 }\n\nfn main() -> Int { f() }\n\n#[test]\nfn same() { assert_eq(1, 1); }\n\n#[test]\nfn calls() { assert_eq(f(), 1); }\n";
    let new = old.replace("{ 1 }", "{ 0 + 1 }").replace(
        "main() -> Int { f()",
        "main(h: Http) -> Int { let _ = h.get(\"/\"); f()",
    );
    let s = Sides::new(old, &new);
    let (code, out, err) = s.review(&["--mutants", "0"]);
    assert_eq!(code, 3, "{out}{err}");
    let resources = Sides::section(&out, "Resources");
    assert_eq!(resources.len(), 3, "{out}");
    assert!(resources[0].starts_with("  measured     calls: fuel "), "{out}");
    assert!(resources[0].ends_with(" KB (+0.00%)"), "{out}");
    assert!(resources[1].starts_with("  measured     same: fuel "), "{out}");
    assert!(resources[1].ends_with(" KB (+0.00%); unchanged code"), "{out}");
}

/// A mutant under which a property discards every case is not killed: it survives, discarded.
#[test]
fn a_property_that_discards_every_case_kills_nothing() {
    let new = "fn keep(n: Int) -> Bool { true }\n\nfn main() -> Int { if keep(1) { 1 } else { 0 } }\n\n#[test]\nfn kept(n: Int) { assume(keep(n)); assert(n == n, \"same\"); }\n";
    let s = Sides::new(BARE, new);
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    let at = |l: &str| l.replace("{new}", &s.new);
    let survived = at("  not checked  survived: generated: {new}:1:27: keep: `true` -> `false` in `...keep(n: Int) -> Bool { true }` (kept discarded every case)");
    assert_eq!(
        mutant_lines(&out),
        [
            "  tested       mutants: 0 of 1 killed (generated at seed 1: 1 drawn of 1 sites, 0 did not check)",
            &survived,
            &caret(&survived, "{ true }", "true"),
        ],
        "{out}"
    );
}

/// An entry's function with the root's tokens but another value of a constant it reads does not
/// run as the root's: no test of the root's reaches it, and it gets no mutant.
#[test]
fn an_entry_constant_that_differs_from_the_roots() {
    let over = "fn over(n: Int) -> Bool { n > LIMIT }\n";
    let root = format!("const LIMIT: Int = 10;\n\n{over}\nfn main() -> Int {{ 0 }}\n\n#[test]\nfn eleven() {{ assert(over(11), \"11\"); }}\n");
    let s = Sides::new(BARE, &root);
    let worker =
        format!("const LIMIT: Int = 1000;\n\n{over}\nfn main() -> Int {{ if over(5) {{ 1 }} else {{ 0 }} }}\n");
    write(s.dir.path(), "new/worker.kio", &worker);
    let (code, out, err) = s.review(&["--entry", "worker.kio"]);
    assert_eq!(code, 0, "{out}{err}");
    let tests = Sides::section(&out, "Tests");
    assert_eq!(
        tests[4..],
        ["  not checked  changed, reached by no test: worker.kio: main, over"],
        "{out}"
    );
    let (_, out, _) = s.review(&["--entry", "worker.kio", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["tests"]["mutants"]["sites"], 0);
}

// R1b2: a mutations file (`--mutations FILE`): written mutants, each a function, a description
// and one `-`/`+` pair.

/// kio-web's flow at F1.2, the worker as NEW (`examples/flow`, `web.kio` and `api.kio` two levels up, as its
/// `kio.deps` pins them): its four mutation checks, `flow.mutants`, are all killed.
#[test]
fn flows_four_written_mutants_are_killed() {
    let flow = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flow/examples/flow");
    let tmp = tempfile::tempdir().unwrap();
    let old = write(tmp.path(), "old.kio", BARE);
    let r = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["review", "worker.kio", "--base", &old, "--test-grants", "test.grants"])
        .args(["--mutations", "flow.mutants", "--mutants", "0"])
        .current_dir(&flow)
        .env("TMPDIR", tmp.path())
        .env("FLOW_KEY", "test-key")
        .output()
        .unwrap();
    let (out, err) = (String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr));
    // OLD holds none of NEW's grants: authority added, exit 3; no mutant survived.
    assert_eq!(r.status.code(), Some(3), "{out}{err}");
    assert_eq!(
        mutant_lines(&out),
        [
            "  tested       mutants: 4 of 4 killed (flow.mutants (inside NEW's tree) 4 of 4)",
            "  tested       killed by: tests::a_claim_needs_the_job_still_due_and_unchanged 1, tests::every_attempt_carries_the_event_id_as_the_key 1, tests::expired_lease_is_reclaimed_and_the_old_worker_changes_nothing 1, tests::failure_retries_with_doubling_backoff_then_dies 1",
        ],
        "{out}{err}"
    );
    let file = std::fs::read(flow.join("flow.mutants")).unwrap();
    let identity = Sides::section(&out, "Identity");
    let line = format!("  mutations flow.mutants sha256:{} (inside NEW's tree)", sha(&file));
    assert!(identity.contains(&line), "{out}");
}

/// `big` is touched by a test that does not pin its boundary.
const WRITTEN_SURVIVES: &str = "big: the boundary moves\n- n >= 100\n+ n > 100\n";

/// A written mutant no test kills is exit 3, its edit and the tests it ran shown. Generated
/// mutants run too, apart in the headline.
#[test]
fn a_written_survivor_is_exit_3() {
    let s = Sides::new(BARE, PINNED);
    let m = write(s.dir.path(), "m.mutants", WRITTEN_SURVIVES);
    let (code, out, err) = s.review(&["--mutants", "0", "--mutations", &m]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        mutant_lines(&out),
        [
            format!("  tested       mutants: 0 of 1 killed ({m} 0 of 1)"),
            format!("  not checked  survived: {m}:1: {}:3:26: big: `n >= 100` -> `n > 100`, ran 1 test: \"the boundary moves\"", s.new),
        ],
        "{out}"
    );
    let (code, out, _) = s.review(&["--mutations", &m]);
    assert_eq!(code, 3, "{out}");
    assert_eq!(
        mutant_lines(&out)[0],
        format!("  tested       mutants: 2 of 5 killed (generated 2 of 4 at seed 1: 4 drawn of 4 sites, 0 did not check; {m} 0 of 1)")
    );
}

/// Each entry that cannot run is an `error` with its hint, exit 1; the others still run. A file
/// that does not parse, or is not a regular file by its own name, is refused before any run.
#[test]
fn a_written_entry_that_cannot_run_is_an_error() {
    let s = Sides::new(BARE, PINNED);
    let entries = [
        "small: twice\n- n\n+ m\n",
        "small: gone\n- n > 10\n+ n < 10\n",
        "smal: unknown\n- n < 10\n+ n <= 10\n",
        "small: does not check\n- n < 10\n+ n < \"ten\"\n",
        "worker.kio: deliver: not an entry\n- 200\n+ 201\n",
        "small: killed\n- n < 10\n+ n <= 10\n",
    ];
    let m = write(s.dir.path(), "m.mutants", &entries.join("---\n"));
    let (code, out, err) = s.review(&["--mutants", "0", "--mutations", &m]);
    assert_eq!(code, 1, "{out}{err}");
    let hint = |h: &str| format!("                 hint: {h}");
    assert_eq!(
        mutant_lines(&out),
        [
            format!("  tested       mutants: 1 of 1 killed ({m} 1 of 1)"),
            "  tested       killed by: pins_small 1".to_string(),
            format!("  error        mutants: {m}:1: the `-` text occurs 4 times in small"),
            hint("lengthen it until it occurs once; an entry names a function and a snippet, never a line"),
            format!("  error        mutants: {m}:5: the `-` text is not in small"),
            hint("the code it pinned has changed: copy the text from small in NEW again, or drop the entry"),
            format!("  error        mutants: {m}:9: NEW has no function smal"),
            hint("name it as the page does, `flow::finish`; an --entry's own function as `worker.kio: deliver`"),
            format!("  error        mutants: {m}:13: the mutant does not check: `<` cannot be applied to Int and Str"),
            hint("change the `+` text so the program still checks; no test can run a program the checker refuses"),
            format!("  error        mutants: {m}:17: worker.kio is not an --entry of this review"),
            hint("add `--entry worker.kio`, or name a function of the root without a program"),
        ],
        "{out}"
    );
    // Refused before any run: a usage error, the format in the hint.
    let refused = |name: &str, text: &str| {
        let f = write(s.dir.path(), name, text);
        let (code, _, err) = s.review(&["--mutations", &f]);
        assert_eq!(code, 2, "{err}");
        (f, err)
    };
    let (f, err) = refused("bad.mutants", "small: no edit\n+ n\n");
    let want = format!(
        "{f}:2: expected `- text`, the text the mutant replaces\n  hint: an entry is `function: what it breaks`"
    );
    assert!(err.contains(&want), "{err}");
    let (f, err) = refused("same.mutants", "small: nothing\n- n < 10\n+ n < 10\n");
    let want = format!("{f}:3: the `+` text is the `-` text, so the mutant changes nothing\n  hint: write the wrong behaviour a test should catch");
    assert!(err.contains(&want), "{err}");
    let (f, err) = refused("big.mutants", &"#\n".repeat(40 << 10));
    assert!(
        err.contains(&format!("--mutations {f}: over 64 KiB\n  hint: an entry is")),
        "{err}"
    );
    let linked = s.dir.path().join("linked.mutants");
    std::fs::hard_link(&m, &linked).unwrap();
    let (code, _, err) = s.review(&["--mutations", linked.to_str().unwrap()]);
    assert_eq!(code, 2);
    let want = "a symlink, a hard link or not a regular file\n  hint: give the file itself; kio reads a mutations file only by its own name";
    assert!(err.contains(want), "{err}");
}

/// FILE is read once: rewritten while OLD's tests run, its mutants still come from the bytes
/// read, and Identity reports the change as an `error`, exit 1.
#[test]
fn a_mutations_file_changed_during_the_review() {
    let waits = "\n#[test]\nfn waits(d: Dir, watch: Dir) -> Result<(), IoErr> {\n    d.write(\"started\", \"\".to_bytes())?;\n    let mut i = 0;\n    while i < 100000 && !watch.exists(\"go\")? { i += 1; }\n    Ok(())\n}\n";
    let app = format!("{PINNED}{waits}");
    let s = Sides::new(&app, &app);
    let entry = "small: killed\n- n < 10\n+ n <= 10\n";
    let m = write(s.dir.path(), "new/m.mutants", entry);
    let watch = s.dir.path().join("watch");
    std::fs::create_dir(&watch).unwrap();
    let t = write(
        s.dir.path(),
        "t.grants",
        &format!("--dir watch={}:ro\n--calls 200000\n", watch.display()),
    );
    let tmp = s.dir.path().join("tmp");
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["review", &s.new, "--base", &s.old, "--test-grants", &t])
        .args(["--mutants", "0", "--mutations", &m])
        .env("TMPDIR", &tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while !find(&tmp, "started", 2) {
        assert!(
            child.try_wait().unwrap().is_none(),
            "the review ended before a test started"
        );
        assert!(start.elapsed().as_secs() < 60, "no test started");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::fs::write(&m, "small: other\n- n < 10\n+ n > 10\n").unwrap();
    std::fs::write(watch.join("go"), "").unwrap();
    let r = child.wait_with_output().unwrap();
    let (out, err) = (String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr));
    assert_eq!(r.status.code(), Some(1), "{out}{err}");
    assert_eq!(
        mutant_lines(&out),
        [
            format!("  tested       mutants: 1 of 1 killed ({m} (inside NEW's tree) 1 of 1)"),
            "  tested       killed by: pins_small 1".to_string(),
        ],
        "{out}"
    );
    let identity = Sides::section(&out, "Identity");
    let line = format!("  mutations {m} sha256:{} (inside NEW's tree)", sha(entry.as_bytes()));
    assert!(identity.contains(&line), "{out}");
    let changed = format!("  error        {m} changed while the review ran");
    assert!(identity.contains(&changed), "{out}");
}

/// A mutations file named `generated` is still the reviewer's: its survivor is exit 3, and the
/// rows say `written`, which no path can forge.
#[test]
fn a_mutations_file_named_generated_is_still_written() {
    let s = Sides::new(BARE, PINNED);
    write(s.dir.path(), "generated", WRITTEN_SURVIVES);
    let run = |json: bool| {
        let mut args = vec![
            "review",
            &s.new,
            "--base",
            &s.old,
            "--mutants",
            "0",
            "--mutations",
            "generated",
        ];
        if json {
            args.push("--json");
        }
        let r = Command::new(env!("CARGO_BIN_EXE_kio"))
            .args(args)
            .current_dir(s.dir.path())
            .env("TMPDIR", s.dir.path().join("tmp"))
            .output()
            .unwrap();
        (r.status.code(), String::from_utf8_lossy(&r.stdout).into_owned())
    };
    let (code, out) = run(false);
    assert_eq!(code, Some(3), "{out}");
    let at = |l: &str| l.replace("{new}", &s.new);
    assert_eq!(
        mutant_lines(&out),
        [
            "  tested       mutants: 0 of 1 killed (generated 0 of 1)",
            &at("  not checked  survived: generated:1: {new}:3:26: big: `n >= 100` -> `n > 100`, ran 1 test: \"the boundary moves\""),
        ],
        "{out}"
    );
    let (_, out) = run(true);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let row = &v["tests"]["mutants"]["rows"][0];
    assert_eq!(
        [&row["source"], &row["written"]],
        [&serde_json::json!("generated"), &serde_json::json!(true)]
    );
}

/// Each `probe` call makes `n` host calls; `<=` for `<` makes one more.
const PROBES: &str = r#"fn probe(d: Dir, n: Int) -> Int { let mut i = 0; while i < n { let _x = d.exists("a"); i += 1; } i }

fn main(d: Dir) -> Int { probe(d, 3) }

#[test]
fn probes(d: Dir) { assert(probe(d, 3) >= 0, "ran"); }
"#;

/// T's `--calls` at what the test used: a mutant that needs one more call stops at T's ceiling.
/// A generated one is a `not checked` stop; a written one is an `error`, exit 1, since the
/// reviewer's question went unanswered.
#[test]
fn a_stop_at_ts_own_calls_is_not_a_kill() {
    // OLD probes another name: `probe` changed, no authority added.
    let s = Sides::new(&PROBES.replace("\"a\"", "\"b\""), PROBES);
    let t = write(s.dir.path(), "t.grants", "--calls 3\n");
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!(code, 0, "{out}{err}");
    let lines = mutant_lines(&out);
    assert!(lines[0].contains("stopped by T's limits"), "{out}");
    assert!(
        lines.iter().any(|l| l.starts_with(&format!(
            "  not checked  stopped by T's --calls 3 in probes: generated: {}:1:",
            s.new
        )) && l.contains(": probe: `<` -> `<=`")),
        "{out}"
    );
    assert!(!out.contains("killed by: probes"), "{out}");
    let m = write(
        s.dir.path(),
        "m.mutants",
        "probe: one call too many\n- i < n\n+ i <= n\n",
    );
    let (code, out, err) = s.review(&["--test-grants", &t, "--mutants", "0", "--mutations", &m]);
    assert_eq!(code, 1, "{out}{err}");
    assert_eq!(
        mutant_lines(&out),
        [
            format!("  tested       mutants: 0 of 1 killed, 1 stopped by T's limits ({m} 0 of 1)"),
            format!("  error        mutants: {m}:1: the mutant was stopped by T's --calls 3 in probes, so no test could pass or fail on it"),
            "                 hint: raise T's --calls 3 (the limit named) so the mutant can finish".to_string(),
        ],
        "{out}"
    );
}

/// FILE reached through a symlinked directory inside NEW's tree is inside it, though its
/// canonical path is not.
#[test]
fn a_mutations_file_behind_a_symlink_in_new_is_inside() {
    let s = Sides::new(BARE, PINNED);
    std::fs::create_dir(s.dir.path().join("outside")).unwrap();
    write(s.dir.path(), "outside/m.mutants", WRITTEN_SURVIVES);
    std::os::unix::fs::symlink(s.dir.path().join("outside"), s.dir.path().join("new/checks")).unwrap();
    let m = s.dir.path().join("new/checks/m.mutants");
    let m = m.to_str().unwrap();
    let (code, out, _) = s.review(&["--mutants", "0", "--mutations", m]);
    assert_eq!(code, 3, "{out}");
    assert_eq!(
        mutant_lines(&out)[0],
        format!("  tested       mutants: 0 of 1 killed ({m} (inside NEW's tree) 0 of 1)")
    );
    assert!(
        out.contains(&format!("  mutations {m} sha256:")) && out.contains("(inside NEW's tree)\n"),
        "{out}"
    );
}

/// A helper only tests call is refused: a kill there pads the count and measures nothing `main`
/// does. An entry's header matches `--entry ./worker.kio`.
#[test]
fn a_written_entry_main_does_not_reach_is_refused() {
    let app = format!(
        "{PINNED}\nfn helper(n: Int) -> Int {{ n + 1 }}\n\n#[test]\nfn helps() {{ assert_eq(helper(1), 2); }}\n"
    );
    let s = Sides::new(BARE, &app);
    write(
        s.dir.path(),
        "new/worker.kio",
        "fn deliver(n: Int) -> Bool { n < 300 }\n\nfn main() -> Int { if deliver(1) { 1 } else { 0 } }\n",
    );
    let m = write(
        s.dir.path(),
        "m.mutants",
        "helper: off by one\n- n + 1\n+ n + 2\n---\nworker.kio: deliver: any status\n- n < 300\n+ true\n",
    );
    let (code, out, err) = s.review(&["--entry", "./worker.kio", "--mutants", "0", "--mutations", &m]);
    assert_eq!(code, 1, "{out}{err}");
    assert_eq!(
        mutant_lines(&out),
        [
            format!("  tested       mutants: 0 of 1 killed ({m} 0 of 1)"),
            format!(
                "  not checked  survived: {m}:5: {}/./worker.kio:1:30: worker.kio: deliver: `n < 300` -> `true`, ran 0 tests: \"any status\"",
                s.dir.path().join("new").display()
            ),
            format!("  error        mutants: {m}:1: helper is not reached from NEW's main"),
            "                 hint: name a function `main` reaches; a test or a helper only tests call is not one"
                .to_string(),
        ],
        "{out}"
    );
}

#[test]
fn test_grants_over_new_sources_are_refused() {
    let s = Sides::new(WRITES, WRITES);
    let root = s.dir.path();
    let new = root.join("new");
    std::os::unix::fs::symlink(&new, root.join("link")).unwrap();
    std::fs::create_dir(new.join("testdata")).unwrap();
    let refused = |dir: &Path, file: &str| {
        let t = write(root, "t.grants", &format!("--dir src={}:ro\n", dir.display()));
        let (code, out, err) = s.review(&["--test-grants", &t]);
        assert_eq!((code, out.as_str()), (2, ""), "{}: {err}", dir.display());
        assert_eq!(
            err,
            format!(
                "kio: --test-grants {t}: `--dir src={}:ro` holds NEW's source `{file}`; a test could read the code under review instead of running it\n  hint: grant a directory that holds no program file, e.g. `--dir data=testdata:ro` beside the program\n",
                dir.display()
            )
        );
    };
    // NEW's directory, an ancestor of it, and a symlink to it.
    refused(&new, &s.new);
    refused(root, &s.new);
    refused(&root.join("link"), &s.new);
    // A directory beside or inside NEW's that holds no source is granted.
    for dir in [root.join("old"), new.join("testdata")] {
        let t = write(root, "t.grants", &format!("--dir d={}:ro\n", dir.display()));
        let (code, out, err) = s.review(&["--test-grants", &t]);
        assert_ne!(code, 2, "{}: {out}{err}", dir.display());
    }
    // NEW's file is a symlink into another directory: that directory holds the source too.
    let real = root.join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::rename(&s.new, real.join("app.kio")).unwrap();
    std::os::unix::fs::symlink(real.join("app.kio"), &s.new).unwrap();
    refused(&real, &s.new);
    // A second hard link to NEW's source could sit in any granted directory.
    std::fs::hard_link(real.join("app.kio"), new.join("testdata/copy.kio")).unwrap();
    let old = root.join("old");
    let t = write(root, "t.grants", &format!("--dir d={}:ro\n", old.display()));
    let (code, out, err) = s.review(&["--test-grants", &t]);
    assert_eq!((code, out.as_str()), (2, ""), "{err}");
    assert_eq!(
        err,
        format!(
            "kio: --test-grants {t}: NEW's source `{}` has other hard links; one could be inside `--dir d={}:ro`\n  hint: give the source file one link (copy it), or leave the `--dir` out\n",
            s.new,
            old.display()
        )
    );
}

// T7: a side's example tests, a property's cases and a mutant's examples run on several threads
// when no test can see another's effects; the page is the one a single thread gives.

const CLAMP_OLD: &str = r#"fn clamp(n: Int) -> Int { if n < 0 { 0 } else if n > 100 { 100 } else { n } }

fn count(n: Int) -> Int { let mut i = 0; let mut s = 0; while i < n { s += i; i += 1; } s }

fn main() -> Int { clamp(count(3)) }

#[test]
fn high() { assert_eq(clamp(500), 99); }

#[test]
fn low() { assert_eq(clamp(-5), 0); }

#[test]
fn in_range(n: Int) { assert(clamp(n) >= 0 && clamp(n) <= 100, "range"); }
"#;

/// CLAMP_OLD capped at 99, with more examples, a property that discards, and one that fails.
const CLAMP_NEW: &str = r#"fn clamp(n: Int) -> Int { if n < 0 { 0 } else if n > 99 { 99 } else { n } }

fn count(n: Int) -> Int { let mut i = 0; let mut s = 0; while i < n { s += i; i += 1; } s }

fn main() -> Int { clamp(count(3)) }

#[test]
fn zero_stays() { assert_eq(clamp(0), 0); }

#[test]
fn high() { assert_eq(clamp(500), 99); }

#[test]
fn low() { assert_eq(clamp(-5), 0); }

#[test]
fn mid() { assert_eq(clamp(50), 50); }

#[test]
fn counts() { assert_eq(count(100), 4950); }

#[test]
fn in_range(n: Int) { assert(clamp(n) >= 0 && clamp(n) <= 99, "range"); }

#[test]
fn odd_ones(n: Int) { assume(n % 2 == 1); assert(clamp(n) <= 99, "odd"); }

#[test]
fn never_capped(n: Int) { assert(clamp(n) != 99, "capped"); }
"#;

/// `kio review` of `s` with `args`, on one thread when `serial` (`KIO_TEST_SERIAL`).
fn review_threads(s: &Sides, args: &[&str], serial: bool) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kio"));
    cmd.args(["review", &s.new, "--base", &s.old]).args(args);
    cmd.env("TMPDIR", s.dir.path().join("tmp"));
    if serial {
        cmd.env("KIO_TEST_SERIAL", "1");
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn threads_give_the_page_one_thread_gives() {
    let s = Sides::new(CLAMP_OLD, CLAMP_NEW);
    let (code, out) = review_threads(&s, &[], false);
    assert_eq!(code, 1, "{out}");
    let tests = Sides::section(&out, "Tests");
    for line in [
        "  not checked  old: 2 passed, 1 failed; the base fails its own tests",
        "  tested       new: 7 passed, 1 failed",
        "  error        FAIL never_capped  (4 cases compiled; seed 1)",
        "  not checked  failed in old: high",
    ] {
        assert!(tests.contains(&line.to_string()), "{line}\n{out}");
    }
    assert!(out.contains("odd_ones: 100 cases compiled; seed 1; "), "{out}");
    assert!(out.contains("  minimal input: n = 99"), "{out}");
    assert!(out.contains("killed by: "), "{out}");
    assert_eq!(review_threads(&s, &[], true), (code, out));
}

/// Each test fills a `Db` with 120 MB; SQLite's heap (256 MB) is the process's, so at
/// `--db-mb 200` only one run may hold a `Db` at a time, and every test passes as alone.
#[test]
fn runs_with_a_db_fit_sqlites_heap() {
    let fill = "fn fill(db: Db) -> Result<Int, DbErr> { db.exec(\"CREATE TABLE t (b BLOB)\", [])?; let mut i = 0; while i < 120 { db.exec(\"INSERT INTO t VALUES (randomblob(1000000))\", [])?; i += 1; } Ok(i) }\n\nfn main() -> Int { 1 }\n";
    let tests: String = (1..=4)
        .map(|i| format!("\n#[test]\nfn fill{i}(db: Db) {{ assert_eq(fill(db), Ok(120)); }}\n"))
        .collect();
    let src = format!("{fill}{tests}");
    let s = Sides::new(&src, &src);
    let t = write(s.dir.path(), "t.grants", "--db-mb 200\n");
    let (code, out) = review_threads(&s, &["--test-grants", &t], false);
    assert_eq!(code, 0, "{out}");
    let tests = Sides::section(&out, "Tests");
    assert!(
        tests.contains(&"  tested       new: 4 passed, 0 failed".to_string()),
        "{out}"
    );
    assert!(
        tests.contains(&"  tested       old: 4 passed, 0 failed".to_string()),
        "{out}"
    );
    assert_eq!(review_threads(&s, &["--test-grants", &t], true), (code, out));
}

/// R1d: a feed aggregator in miniature. `feed.kio` is a module the entries share; `fetch.kio`
/// polls a feed over `Http`; `digest.kio` writes a day's digest from a `Db`, the day given per run.
const FEED: &str = r#"pub fn page_size(n: Int) -> Int { if n > 50 { 50 } else { n } }

pub fn ok(status: Int) -> Bool { status >= 200 && status < 300 }

pub fn fixture_status() -> Int { 200 }

#[test]
fn ok_statuses() { assert(ok(fixture_status()), "2xx"); assert(!ok(404), "404"); }
"#;

const FETCH: &str = r#"mod feed;

fn fetch(src: Http) -> Result<Bool, HttpErr> { Ok(feed::ok(src.send("GET", "/feed.xml", [], "".to_bytes(), 0)?.status)) }

fn main(src: Http) -> Result<Bool, HttpErr> { fetch(src) }

#[test]
fn fetches(src: Http) -> Result<(), HttpErr> { assert(fetch(src)?, "200"); Ok(()) }
"#;

/// FETCH sending an ETag, and a test that reads the request back (`--http-record`).
const FETCH_ETAG: &str = r#"mod feed;

fn fetch(src: Http) -> Result<Bool, HttpErr> {
  let r = src.send("GET", "/feed.xml", ["If-None-Match: \"v1\""], "".to_bytes(), 0)?;
  Ok(r.status == 304 || feed::ok(r.status))
}

fn main(src: Http) -> Result<Bool, HttpErr> { fetch(src) }

#[test]
fn fetches(src: Http) -> Result<(), HttpErr> { assert(fetch(src)?, "200"); Ok(()) }

#[test]
fn sends_the_etag(src: Http, sent: Dir) -> Result<(), HttpErr> {
  let _ = fetch(src)?;
  let req = sent.read("src-1").map_err(|_| HttpErr::Timeout)?.to_str();
  assert(req.contains("if-none-match: \"v1\""), "etag sent");
  Ok(())
}
"#;

const DIGEST: &str = r##"mod feed;

fn header(day: Str, n: Int) -> Str { "# " + day + " (" + n.to_str() + " items)\n" }

fn digest(db: Db, out: Dir, day: Str) -> Result<Int, Str> {
  let rows = db.query("SELECT title FROM items ORDER BY id LIMIT ?", [SqlValue::Int(feed::page_size(500))]).map_err(|_| "db")?;
  out.write(day + ".md", header(day, rows.len()).to_bytes()).map_err(|_| "write")?;
  Ok(rows.len())
}

fn main(db: Db, out: Dir, day: Value) -> Result<Int, Str> { digest(db, out, day.get()) }

#[test]
fn dated(day: Value) { assert_eq(header(day.get(), 0), "# 2026-11-20 (0 items)\n"); }

#[test]
fn caps_the_digest() { assert_eq(feed::page_size(500), 50); }
"##;

/// A new entry: a page of items from a `Db`, its size from FEED.
const API: &str = r#"mod feed;

fn listing(db: Db, limit: Int) -> Result<Int, DbErr> {
  let rows = db.query("SELECT id FROM items ORDER BY id LIMIT ?", [SqlValue::Int(feed::page_size(limit))])?;
  Ok(rows.len())
}

fn main(db: Db, limit: Value) -> Result<Int, DbErr> { listing(db, limit.get().parse_int().unwrap_or(20)) }

#[test]
fn lists_a_page(db: Db) -> Result<(), DbErr> {
  db.exec("CREATE TABLE items (id INTEGER)", [])?;
  db.exec("INSERT INTO items VALUES (1), (2), (3)", [])?;
  assert_eq(listing(db, 2)?, 2);
  Ok(())
}
"#;

/// Each `(path, text)` written under a directory of its own, with `tmp/` for kio's temporary files.
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for d in ["old", "new", "tmp"] {
        std::fs::create_dir(dir.path().join(d)).unwrap();
    }
    for (name, text) in files {
        write(dir.path(), name, text);
    }
    dir
}

/// `(exit code, stdout, stderr)` of `kio review args..` run in `dir` (paths relative to it), on
/// one thread when `serial` (`KIO_TEST_SERIAL`).
fn review_in(dir: &Path, args: &[&str], serial: bool) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kio"));
    cmd.arg("review")
        .args(args)
        .current_dir(dir)
        .env("TMPDIR", dir.join("tmp"));
    if serial {
        cmd.env("KIO_TEST_SERIAL", "1");
    }
    let out = cmd.output().unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

/// R1d1: an entry OLD has no file for is reviewed against OLD's directory: its modules by
/// qualified name, their tests run as the base's. FEED is unchanged, so `feed::page_size` is
/// neither changed nor mutated, and `feed::ok_statuses` is not added.
#[test]
fn a_new_entry_is_compared_with_its_directorys_modules() {
    let t = tree(&[
        ("old/feed.kio", FEED),
        ("old/fetch.kio", FETCH),
        ("new/feed.kio", FEED),
        ("new/fetch.kio", FETCH),
        ("new/api.kio", API),
    ]);
    let args = ["new/api.kio", "--base", "old/api.kio"];
    let (code, out, err) = review_in(t.path(), &args, false);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        [
            "  not checked  new entry: old/api.kio has no file",
            "  enforced     + db: Db (query)",
            "  derived        query in listing",
            "  enforced     + limit: Value (get)",
            "  derived        get in main",
        ]
    );
    assert_eq!(
        Sides::section(&out, "Tests"),
        [
            DEFAULT_GRANTS,
            "  tested       old: 1 passed, 0 failed",
            "  tested       new: 2 passed, 0 failed",
            "  tested       added: lists_a_page",
            "  not checked  changed, reached by no test: main",
            "  tested       mutants: 1 of 1 killed (generated at seed 1: 2 drawn of 2 sites, 1 did not check)",
            "  tested       killed by: lists_a_page 1",
        ],
        "{out}"
    );
    assert_eq!(
        Sides::section(&out, "Not checked")[..2],
        [
            "  not checked  db: Db query: the SQL text and what it changes",
            "  not checked  listing: db (new entry)",
        ]
    );
    let identity = Sides::section(&out, "Identity");
    for line in [
        "  old old/api.kio absent".to_string(),
        format!("  old old/feed.kio sha256:{}", sha(FEED.as_bytes())),
        format!("  new new/feed.kio sha256:{}", sha(FEED.as_bytes())),
    ] {
        assert!(identity.contains(&line), "{line}\n{out}");
    }
    assert!(!out.contains("page_size"), "{out}");
    let (_, out, _) = review_in(t.path(), &[&args[..], &["--mutants", "0", "--json"]].concat(), false);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["authority"]["new_entry"], true);
    assert_eq!(
        v["identity"]["old"][0],
        serde_json::json!({"path": "old/api.kio", "sha256": null})
    );
    // A directory OLD lacks stays an error.
    let (code, out, err) = review_in(t.path(), &["new/api.kio", "--base", "gone/api.kio"], false);
    assert_eq!((code, out.as_str()), (2, ""), "{err}");
    assert!(err.contains("gone/api.kio: cannot read"), "{err}");
}

/// The change across entries: FEED's page size raised (only DIGEST's `main` and tests reach it),
/// its test fixture changed, FETCH sending an ETag with a test of it that needs `--http-record`,
/// the fake feed and FETCH's grants changed, DIGEST's grants new.
fn across() -> tempfile::TempDir {
    let feed_new = FEED
        .replace("n > 50 { 50 }", "n > 100 { 100 }")
        .replace("{ 200 }", "{ 204 }");
    let digest_new = DIGEST.replace("page_size(500), 50)", "page_size(500), 100)");
    tree(&[
        ("old/feed.kio", FEED),
        ("old/fetch.kio", FETCH),
        ("old/digest.kio", DIGEST),
        ("old/feed.http", "GET /feed.xml\n200\n"),
        ("old/fetch.grants", "--http src=https://feeds.example.org\n"),
        ("new/feed.kio", &feed_new),
        ("new/fetch.kio", FETCH_ETAG),
        ("new/digest.kio", &digest_new),
        ("new/feed.http", "GET /feed.xml\n304\n---\nGET /feed.xml\n200\n"),
        (
            "new/fetch.grants",
            "--http src=https://feeds.example.org\n--timeout 30\n",
        ),
        (
            "new/digest.grants",
            "--db db=feeds.db:ro\n--dir out=digests\n--timeout 600\n",
        ),
        (
            "new/test.grants",
            "--http src=fake:new/feed.http\n--http-record src=sent\n",
        ),
        ("new/digest-test.grants", "--value day=2026-11-20\n"),
    ])
}

const ACROSS: [&str; 15] = [
    "new/fetch.kio",
    "--base",
    "old/fetch.kio",
    "--grants",
    "new/fetch.grants",
    "--test-grants",
    "new/test.grants",
    "--entry",
    "digest.kio",
    "--entry-grants",
    "new/digest.grants",
    "--value",
    "day=2026-11-20",
    "--entry-test-grants",
    "new/digest-test.grants",
];

/// A change controls T: a `--http-record` only NEW's tests take must not keep OLD's tests from
/// running (F1). OLD's run without it, so the removed test, the changed one and its fuel show,
/// and the page stays exit 3.
const HIDE_OLD: &str = r#"fn fetch(src: Http) -> Result<Bool, HttpErr> { Ok(src.send("GET", "/x", [], "".to_bytes(), 0)?.status == 200) }

fn work(n: Int) -> Int { if n > 10 { 10 } else { n } }

fn main(src: Http) -> Result<Bool, HttpErr> { fetch(src) }

#[test]
fn fetches(src: Http) -> Result<(), HttpErr> { assert(fetch(src)?, "200"); Ok(()) }

#[test]
fn caps() { assert_eq(work(50), 10); }

#[test]
fn small() { assert_eq(work(3), 3); }
"#;

const HIDE_NEW: &str = r#"fn fetch(src: Http) -> Result<Bool, HttpErr> { Ok(src.send("GET", "/x", [], "".to_bytes(), 0)?.status == 200) }

fn work(n: Int) -> Int { let mut i = 0; let mut acc = 0; while i < 2000 { acc = acc + i; i = i + 1; } if n > 100 { 100 } else { n } }

fn main(src: Http) -> Result<Bool, HttpErr> { fetch(src) }

#[test]
fn fetches(src: Http) -> Result<(), HttpErr> { assert(fetch(src)?, "200"); Ok(()) }

#[test]
fn sends(src: Http, sent: Dir) -> Result<(), HttpErr> { let _ = fetch(src)?; assert(sent.read("src-1").is_ok(), "recorded"); Ok(()) }

#[test]
fn small() { assert_eq(work(3), 3); }
"#;

#[test]
fn a_record_only_new_tests_take_hides_no_old_test() {
    let t = tree(&[
        ("old/app.kio", HIDE_OLD),
        ("new/app.kio", HIDE_NEW),
        ("new/f.http", "GET /x\n200\n"),
        ("new/t.grants", "--http src=fake:new/f.http\n--http-record src=sent\n"),
    ]);
    let args = [
        "new/app.kio",
        "--base",
        "old/app.kio",
        "--test-grants",
        "new/t.grants",
        "--mutants",
        "0",
    ];
    let (code, out, err) = review_in(t.path(), &args, false);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Tests"),
        [
            "  tested       grants: new/t.grants (src answered from new/f.http)",
            "  tested       old: 3 passed, 0 failed",
            "  tested       new: 3 passed, 0 failed",
            "  tested       old: tests run without --http-record src=sent (no old test takes its Dir)",
            "  tested       added: sends",
            "  not checked  removed: caps",
            "  not checked  changed: small (reaches changed work)",
        ],
        "{out}"
    );
    // The fuel row of the changed test (numbers differ under the test heap).
    let row = &Sides::section(&out, "Resources")[0];
    assert!(
        row.starts_with("  measured     small: fuel ") && row.contains(" -> "),
        "{out}"
    );
}

/// F3: a module's test that runs in another suite on the other side (here FETCH loads FEED only
/// in NEW, so its test moves from DIGEST's suite to the root's) is the same test: neither
/// removed nor added.
#[test]
fn a_shared_test_that_moves_suites_is_the_same_test() {
    let feed = "pub fn ok(s: Int) -> Bool { s == 200 }\n#[test]\nfn ok_t() { assert(ok(200), \"ok\"); }\n";
    let digest = "mod feed;\nfn main() -> Int { if feed::ok(200) { 1 } else { 0 } }\n#[test]\nfn d_t() { assert(feed::ok(200), \"x\"); }\n";
    let t = tree(&[
        ("old/feed.kio", feed),
        ("old/digest.kio", digest),
        ("old/fetch.kio", "fn main() -> Int { 1 }\n#[test]\nfn root_t() { assert_eq(1, 1); }\n"),
        ("new/feed.kio", feed),
        ("new/digest.kio", digest),
        (
            "new/fetch.kio",
            "mod feed;\nfn main() -> Int { if feed::ok(200) { 1 } else { 0 } }\n#[test]\nfn root_t() { assert_eq(1, 1); }\n",
        ),
    ]);
    let args = [
        "new/fetch.kio",
        "--base",
        "old/fetch.kio",
        "--entry",
        "digest.kio",
        "--mutants",
        "0",
    ];
    let (code, out, err) = review_in(t.path(), &args, false);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Tests"),
        [
            DEFAULT_GRANTS,
            "  tested       old: 3 passed, 0 failed",
            "  tested       new: 3 passed, 0 failed",
            "  not checked  changed, reached by no test: main",
        ],
        "{out}"
    );
}

/// F2: a changed function of a module two entries share is mutated against every entry's tests
/// that reach it, each in its own program: which entry is the root does not change the kills.
#[test]
fn shared_mutants_do_not_depend_on_which_entry_is_the_root() {
    let feed = |n: &str| format!("pub fn page_size(n: Int) -> Int {{ if n > {n} {{ {n} }} else {{ n }} }}\n");
    let strong = |n: &str| {
        format!("mod feed;\nfn main() -> Int {{ feed::page_size(500) }}\n#[test]\nfn strong() {{ assert_eq(feed::page_size(500), {n}); assert_eq(feed::page_size(101), {n}); assert_eq(feed::page_size(5), 5); }}\n")
    };
    let weak = "mod feed;\nfn main() -> Int { feed::page_size(5) }\n#[test]\nfn weak() { assert(feed::page_size(5) > 0, \"positive\"); }\n";
    let t = tree(&[
        ("old/feed.kio", &feed("50")),
        ("old/digest.kio", &strong("50")),
        ("old/fetch.kio", weak),
        ("new/feed.kio", &feed("100")),
        ("new/digest.kio", &strong("100")),
        ("new/fetch.kio", weak),
    ]);
    let kills = |root: &str, entry: &str| {
        let (new, old) = (format!("new/{root}"), format!("old/{root}"));
        let (_, out, err) = review_in(t.path(), &[&new, "--base", &old, "--entry", entry], false);
        let lines: Vec<String> = Sides::section(&out, "Tests")
            .into_iter()
            .filter(|l| l.contains("mutants:") || l.contains("survived:"))
            .collect();
        assert!(!lines.is_empty(), "{out}{err}");
        let by = Sides::section(&out, "Tests")
            .into_iter()
            .find(|l| l.contains("killed by:"))
            .unwrap_or_default();
        (lines, by.replace("digest.kio: ", ""))
    };
    let (a, by_a) = kills("fetch.kio", "digest.kio");
    let (b, by_b) = kills("digest.kio", "fetch.kio");
    assert_eq!(a, b);
    assert_eq!(by_a, by_b);
    assert_eq!(by_a, "  tested       killed by: strong 2");
}

/// R1d3, 4, 6, 7: one review of a change to two entries and their shared module.
#[test]
fn a_change_across_entries() {
    let t = across();
    let (code, out, err) = review_in(t.path(), &ACROSS, false);
    assert_eq!(code, 0, "{out}{err}");
    // A grant per run on the binds line; a new grants file's limit; a changed one's.
    assert_eq!(
        Sides::section(&out, "Authority"),
        [
            "  enforced     no change in authority",
            "  enforced     fetch.grants: + --timeout 30",
            "  enforced     --grants new/fetch.grants binds new/fetch.kio's main: src=https://feeds.example.org",
            "  enforced     digest.kio: no change in authority",
            "  enforced     digest.kio: digest.grants: new file",
            "  enforced     digest.kio: digest.grants: + --timeout 600",
            "  enforced     digest.kio: --entry-grants new/digest.grants binds new/digest.kio's main: db=feeds.db:ro, out=digests, day=2026-11-20 (command line)",
        ],
        "{out}"
    );
    // DIGEST's tests run under their own grants, merged; FEED's test runs once. OLD's FETCH
    // tests run without the record, and its `fetches` fails on the changed fake.
    let fetch = r#"  not checked  survived: generated: new/fetch.kio:4:82: fetch: `0` -> `1` in `..."v1\\""], "".to_bytes(), 0)?;`"#;
    let gt = "  not checked  survived: generated: new/feed.kio:1:40: feed::page_size: `>` -> `>=` in `...e(n: Int) -> Int { if n > 100 { 100 } else { n }...`";
    let cap = "  not checked  survived: generated: new/feed.kio:1:42: feed::page_size: `100` -> `101` in `...n: Int) -> Int { if n > 100 { 100 } else { n } }`";
    assert_eq!(
        Sides::section(&out, "Tests"),
        [
            "  tested       grants: new/test.grants (src answered from new/feed.http)",
            "  tested       digest.kio: grants: new/digest-test.grants",
            "  not checked  old: 3 passed, 1 failed; the base fails its own tests",
            "  tested       new: 5 passed, 0 failed",
            "  tested       old: tests run without --http-record src=sent (no old test takes its Dir)",
            "  tested       added: sends_the_etag",
            "  not checked  changed: feed::ok_statuses (reaches changed feed::fixture_status)",
            "  not checked  changed: fetches (reaches changed fetch)",
            "  not checked  changed: digest.kio: caps_the_digest (reaches changed feed::page_size)",
            "  not checked  failed in old: fetches",
            "  tested       mutants: 7 of 10 killed (generated at seed 1: 11 drawn of 11 sites, 1 did not check)",
            "  tested       killed by: fetches 6, digest.kio: caps_the_digest 1",
            fetch,
            &caret(fetch, "0)?;`", "0"),
            gt,
            &caret(gt, "n > 100 {", ">"),
            cap,
            &caret(cap, "100 { 100 } else { n } }`", "100"),
            "  not checked  1 changed function not mutated: reached by no `main` (feed::fixture_status)",
        ],
        "{out}"
    );
    // `:ro` reads; the fake's change is counted.
    assert_eq!(
        Sides::section(&out, "Not checked"),
        [
            "  not checked  src: Http send GET: the paths, headers and bodies sent",
            "  not checked  fetch: src (changed)",
            "  not checked  digest.kio: db: Db query: the SQL text and what it reads",
            "  not checked  digest.kio: out: Dir write: what is written or removed",
            "  not checked  digest.kio: feed::page_size: db, out (changed)",
            "  not checked  src: Http: tests ran against new/feed.http, not a server; the real endpoint's answers are untested",
            "  not checked  fake new/feed.http changed (+3 -0 lines)",
            SAME_AUTHOR,
        ],
        "{out}"
    );
    let identity = Sides::section(&out, "Identity");
    for line in [
        format!("  old test fake old/feed.http sha256:{}", sha(b"GET /feed.xml\n200\n")),
        format!(
            "  digest.kio: entry test grants new/digest-test.grants sha256:{}",
            sha(b"--value day=2026-11-20\n")
        ),
    ] {
        assert!(identity.contains(&line), "{line}\n{out}");
    }
    assert_eq!(review_in(t.path(), &ACROSS, true), (code, out, err));
}

/// R1d3: without `--entry-test-grants` an entry's tests run under `--test-grants`: here DIGEST's
/// `dated` lacks its `day` (T's record binds per test, T8, so the rest run).
#[test]
fn an_entrys_tests_default_to_the_test_grants() {
    let t = across();
    let args = [&ACROSS[..13], &["--mutants", "0"]].concat();
    let (code, out, _) = review_in(t.path(), &args, false);
    assert_eq!(code, 1, "{out}");
    let tests = Sides::section(&out, "Tests");
    let at = tests.iter().position(|l| l == "  error        FAIL digest.kio: dated");
    assert!(at.is_some(), "{out}");
    assert_eq!(
        tests[at.unwrap() + 1],
        "                 test `dated` needs `day: Value`",
        "{out}"
    );
}

/// An entry's tests' grants are held to T's rules: no writable `--dir`.
#[test]
fn entry_test_grants_refuse_a_writable_dir() {
    let t = across();
    write(t.path(), "new/w.grants", "--dir scratch=scratch\n");
    let args = [&ACROSS[..14], &["new/w.grants", "--mutants", "0"]].concat();
    let (code, out, err) = review_in(t.path(), &args, false);
    assert_eq!((code, out.as_str()), (2, ""), "{err}");
    assert!(
        err.contains(
            "--entry-test-grants new/w.grants: `--dir scratch=scratch` would carry OLD's test writes into NEW's"
        ),
        "{err}"
    );
}

/// F5: a written mutant of an entry whose tests do not run is said, not dropped.
#[test]
fn a_written_mutant_of_an_entry_whose_tests_do_not_run_is_said() {
    let t = across();
    write(t.path(), "new/l.grants", "--listen 127.0.0.1:1\n");
    write(
        t.path(),
        "d.mutants",
        "digest.kio: digest: no page cap\n- feed::page_size(500)\n+ 500\n",
    );
    let args = [
        &ACROSS[..14],
        &["new/l.grants", "--mutants", "0", "--mutations", "d.mutants"],
    ]
    .concat();
    let (code, out, err) = review_in(t.path(), &args, false);
    assert_eq!(code, 1, "{out}{err}");
    let line = "  not checked  mutants: d.mutants:1: digest.kio's tests do not run, so its mutant did not run";
    assert!(Sides::section(&out, "Tests").contains(&line.to_string()), "{out}");
}

/// F6: a new entry's base in NEW's own directory compares NEW with its own modules: said.
#[test]
fn a_new_entry_base_in_news_directory_is_said() {
    let t = tree(&[("new/feed.kio", FEED), ("new/api.kio", API)]);
    let (_, out, err) = review_in(
        t.path(),
        &["new/api.kio", "--base", "new/./gone.kio", "--mutants", "0"],
        false,
    );
    assert_eq!(
        Sides::section(&out, "Authority")[..2],
        [
            "  not checked  new entry: new/./gone.kio has no file",
            "  not checked  new entry: new/./gone.kio shares NEW's directory; its modules are NEW's own",
        ],
        "{out}{err}"
    );
}

/// R1d4: a grant flag goes right after the grants file it adds to.
#[test]
fn grant_flags_follow_a_grants_file() {
    let t = across();
    for args in [
        &["new/fetch.kio", "--base", "old/fetch.kio", "--value", "day=1"][..],
        &[
            "new/fetch.kio",
            "--base",
            "old/fetch.kio",
            "--entry",
            "digest.kio",
            "--value",
            "day=1",
        ],
        &[
            "new/fetch.kio",
            "--base",
            "old/fetch.kio",
            "--grants",
            "new/fetch.grants",
            "--seed",
            "2",
            "--value",
            "day=1",
        ],
    ] {
        let (code, out, err) = review_in(t.path(), args, false);
        assert_eq!((code, out.as_str()), (2, ""), "{args:?}");
        assert!(
            err.contains("`--value day=1` is a grant: give it right after the grants file it adds to"),
            "{err}"
        );
        assert!(err.contains("hint: `--grants app.grants --value ..`"), "{err}");
    }
    // As `kio run` takes them: a limit the file sets once cannot be given again.
    let args = [&ACROSS[..5], &["--timeout", "5", "--mutants", "0"]].concat();
    let (code, out, _) = review_in(t.path(), &args, false);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("  error        --grants new/fetch.grants does not bind to new/fetch.kio's main:"),
        "{out}"
    );
}
