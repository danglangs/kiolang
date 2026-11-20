//! L32c: `kio review` with capability groups: the labels of binding lines, the per-leaf Not
//! checked line, an entry's prefixed lines (row 15) and mutants reached only through a group.
use std::path::Path;
use std::process::Command;

fn kio(tmp: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(args)
        .env("TMPDIR", tmp)
        .output()
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

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

const GROUPED: &str = r#"struct Env { db: Db, audit: Dir }
fn handle(env: Env, r: Str) -> Result<Int, DbErr> { let _ = env.audit.append("log", r.to_bytes()); env.db.exec(r, []) }
fn main(db: Db, audit: Dir) -> Result<Int, DbErr> { handle(Env { db: db, audit: audit }, "x") }

#[test]
fn handles(env: Env) -> Result<(), IoErr> {
  assert_eq(handle(env, "create table t (a)").unwrap_or(-1), 0);
  assert(env.audit.exists("log")?, "logged");
  Ok(())
}
"#;

/// A field added to the group: the grant line once, the `derived` line naming the function that
/// uses it; a mutant in a function reached only through a group runs.
#[test]
fn a_new_field_is_one_grant_line() {
    let with_admin = GROUPED
        .replace("struct Env { db: Db, audit: Dir }", "struct Env { db: Db, admin: Secret, audit: Dir }")
        .replace(
            "{ let _ = env.audit.append(\"log\", r.to_bytes()); env.db.exec(r, []) }",
            "{ if !env.admin.matches(r) { return Ok(-2); } let _ = env.audit.append(\"log\", r.to_bytes()); env.db.exec(r, []) }",
        )
        .replace("fn main(db: Db, audit: Dir)", "fn main(db: Db, admin: Secret, audit: Dir)")
        .replace("Env { db: db, audit: audit }", "Env { db: db, admin: admin, audit: audit }")
        .replace(
            "  assert(env.audit.exists(\"log\")?, \"logged\");",
            "  assert(env.audit.exists(\"log\")?, \"logged\");\n  assert_eq(handle(env, \"nope\").unwrap_or(-1), -2);",
        );
    let s = Sides::new(GROUPED, &with_admin);
    let tok = write(s.dir.path(), "tok.txt", "create table t (a)");
    let g = write(s.dir.path(), "t.grants", &format!("--secret admin=file:{tok}\n"));
    let (code, out, err) = s.review(&["--test-grants", &g]);
    assert_eq!(code, 3, "{out}{err}");
    let authority = Sides::section(&out, "Authority");
    assert_eq!(
        authority[..2],
        [
            "  enforced     + admin: Secret (matches)",
            "  derived        matches in handle"
        ]
    );
    assert!(
        authority.iter().filter(|l| l.contains("+ admin: Secret")).count() == 1,
        "{out}"
    );
    // `handle` changed and is reached only through the group: its mutants ran, and the test that
    // reaches it through `env` killed them.
    let tests = Sides::section(&out, "Tests");
    assert!(
        tests
            .iter()
            .any(|l| l.contains("mutants: 3 of 3 killed (generated at seed 1: 4 drawn of 4 sites")),
        "{out}"
    );
    assert!(tests.iter().any(|l| l.contains("killed by: handles")), "{out}");
}

/// Rows 1b and 1c: a rebinding exits 3 with its lines labelled `derived`; a reordering among
/// origins a leaf already has prints nothing but the Not checked line, marked by the changed
/// caller.
#[test]
fn rebinding_lines_are_derived_and_blind_spots_per_leaf() {
    let fg = |calls: &str| {
        format!(
            "fn f(d: Db) -> Result<Int, DbErr> {{ d.exec(\"f\", []) }}
fn g(d: Db) -> Result<Int, DbErr> {{ d.exec(\"g\", []) }}
fn main(x: Db, y: Db) -> Result<Int, DbErr> {{ {calls} }}
#[test]
fn t(x: Db) {{ assert_eq(f(x).unwrap_or(-1), -1); }}
"
        )
    };
    let s = Sides::new(&fg("let _ = f(x)?; g(y)"), &fg("let _ = f(y)?; g(x)"));
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 3, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        [
            "  derived      grants unchanged; capability binding changed: f, g",
            "  derived      ~ f: d from y (was x)",
            "  derived      ~ g: d from x (was y)",
        ]
    );
    let (_, out, _) = s.review(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["authority"]["bindings"],
        serde_json::json!([
            {"function": "f", "leaf": "d", "from": ["y"], "was": ["x"]},
            {"function": "g", "leaf": "d", "from": ["x"], "was": ["y"]}
        ])
    );
    // Row 1c: both calls before and after, reordered: no authority line; Not checked says so.
    let copy = |calls: &str| {
        format!(
            "fn copy(src: Dir, dst: Dir) -> Result<(), IoErr> {{ dst.write(\"x\", src.read(\"x\")?) }}
fn main(a: Dir, b: Dir) -> Result<(), IoErr> {{ {calls} }}
#[test]
fn t(a: Dir, b: Dir) {{ let _ = copy(a, b); }}
"
        )
    };
    let s = Sides::new(&copy("copy(a, b)?; copy(b, a)"), &copy("copy(b, a)?; copy(a, b)"));
    let (code, out, err) = s.review(&[]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        Sides::section(&out, "Authority"),
        ["  enforced     no change in authority"]
    );
    let not_checked = Sides::section(&out, "Not checked");
    assert!(
        not_checked.contains(
            &"  not checked  copy: src from a, b; which call uses which is behaviour (changed: main)".to_string()
        ),
        "{out}"
    );
    assert!(
        not_checked.contains(
            &"  not checked  copy: dst from a, b; which call uses which is behaviour (changed: main)".to_string()
        ),
        "{out}"
    );
    // The same program on both sides: the line stays, unmarked.
    let same = copy("copy(a, b)?; copy(b, a)");
    let s = Sides::new(&same, &same);
    let (_, out, _) = s.review(&[]);
    assert!(
        Sides::section(&out, "Not checked")
            .contains(&"  not checked  copy: src from a, b; which call uses which is behaviour".to_string()),
        "{out}"
    );
}

/// Row 15: a function two entries share, with different origins: one function line, and each
/// entry's binding line prefixed with its name when it changes.
#[test]
fn an_entry_prefixes_its_binding_lines() {
    let lib = "pub fn init(db: Db) -> Result<Int, DbErr> { db.exec(\"create table if not exists t (a)\", []) }\n";
    let app = "mod lib;\nfn main(db: Db) -> Result<Int, DbErr> { lib::init(db) }\n";
    let remind =
        |call: &str| format!("mod lib;\nfn main(db: Db, archive: Db) -> Result<Int, DbErr> {{ lib::init({call}) }}\n");
    let s = Sides::new(app, app);
    let root = s.dir.path();
    for side in ["old", "new"] {
        write(root, &format!("{side}/lib.kio"), lib);
    }
    write(root, "old/remind.kio", &remind("db"));
    write(root, "new/remind.kio", &remind("archive"));
    let (code, out, err) = s.review(&["--entry", "remind.kio"]);
    assert_eq!(code, 3, "{out}{err}");
    let authority = Sides::section(&out, "Authority");
    assert_eq!(
        authority[..5],
        [
            "  enforced     no change in authority",
            "  enforced     remind.kio: ~ db: Db (-exec)",
            "  enforced     remind.kio: ~ archive: Db (+exec)",
            "  derived      remind.kio:   exec in lib::init",
            "  derived      remind.kio: ~ lib::init: db from archive (was db)",
        ]
    );
}
