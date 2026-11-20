//! L32b: `kio check NEW --base OLD` with capability groups and binding lines: the printed text,
//! the exit code and `--json`'s `authority.bindings[]`.
use std::path::Path;
use std::process::Command;

fn side(root: &Path, side: &str, src: &str) -> String {
    let dir = root.join(side);
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("app.kio"), src).unwrap();
    dir.join("app.kio").to_str().unwrap().to_string()
}

/// `kio check new --base old [flags]`: exit code, stdout, stderr.
fn diff(old: &str, new: &str, flags: &[&str]) -> (i32, String, String) {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = (side(t.path(), "old", old), side(t.path(), "new", new));
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["check", &new, "--base", &old])
        .args(flags)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (out.status.code().unwrap(), s(out.stdout), s(out.stderr))
}

const FLAT: &str = r#"fn handle(db: Db, admin: Secret, r: Str) -> Result<Int, DbErr> {
  if admin.matches(r) { db.exec(r, []) } else { Ok(0) }
}
fn main(db: Db, admin: Secret) -> Result<Int, DbErr> { handle(db, admin, "x") }
"#;

const GROUPED: &str = r#"struct Env { db: Db, admin: Secret }
fn handle(env: Env, r: Str) -> Result<Int, DbErr> {
  if env.admin.matches(r) { env.db.exec(r, []) } else { Ok(0) }
}
fn main(db: Db, admin: Secret) -> Result<Int, DbErr> { handle(Env { db: db, admin: admin }, "x") }
"#;

/// A pure grouping refactor is no change; a field added to the group is one grant line.
#[test]
fn grouping_is_no_change_and_a_new_field_is_one_line() {
    assert_eq!(
        diff(FLAT, GROUPED, &[]),
        (0, "no change in authority\n".into(), "".into())
    );
    let with_log = GROUPED
        .replace(
            "struct Env { db: Db, admin: Secret }",
            "struct Env { db: Db, admin: Secret, log: Log }",
        )
        .replace(
            "{ env.db.exec(r, []) }",
            "{ let _ = env.log.write(r); env.db.exec(r, []) }",
        )
        .replace(
            "fn main(db: Db, admin: Secret)",
            "fn main(db: Db, admin: Secret, log: Log)",
        )
        .replace("Env { db: db, admin: admin }", "Env { db: db, admin: admin, log: log }");
    let want = "+ log: Log (write)\nauthority added (exit 3); --detail lists each function\n";
    assert_eq!(diff(GROUPED, &with_log, &[]), (3, want.into(), "".into()));
    let want = "+ log: Log (write)\n\n+ handle: env.log write\n+ main: log write\nauthority added (exit 3)\n";
    assert_eq!(diff(GROUPED, &with_log, &["--detail"]), (3, want.into(), "".into()));
}

/// Rows 1b, 13 and 17: a leaf-only re-pairing of same-typed grants exits 3 under the header,
/// and `--json` carries `authority.bindings[]`.
#[test]
fn rebinding_exits_3_with_the_header_and_json_rows() {
    let fg = |calls: &str| {
        format!(
            "fn f(d: Db) -> Result<Int, DbErr> {{ d.exec(\"f\", []) }}
fn g(d: Db) -> Result<Int, DbErr> {{ d.exec(\"g\", []) }}
fn main(x: Db, y: Db) -> Result<Int, DbErr> {{ {calls} }}"
        )
    };
    let (old, new) = (fg("let _ = f(x)?; g(y)"), fg("let _ = f(y)?; g(x)"));
    let want = "derived  grants unchanged; capability binding changed: f, g\n~ f: d from y (was x)\n~ g: d from x (was y)\nauthority added (exit 3)\n";
    assert_eq!(diff(&old, &new, &[]), (3, want.into(), "".into()));
    let (code, out, _) = diff(&old, &new, &["--json"]);
    assert_eq!(code, 3);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(
        v["diff"]["bindings"],
        serde_json::json!([
            {"function": "f", "leaf": "d", "from": ["y"], "was": ["x"]},
            {"function": "g", "leaf": "d", "from": ["x"], "was": ["y"]}
        ])
    );
    assert_eq!(v["diff"]["added"], true);
    assert_eq!(v["diff"]["grants"], serde_json::json!([]));
    // A leaf new in this version has `was: null`; a leaf `main` does not reach has no row.
    let (_, out, _) = diff(&fg("f(x)"), &fg("let _ = f(x)?; g(y)"), &["--json"]);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(
        v["diff"]["bindings"],
        serde_json::json!([{"function": "g", "leaf": "d", "from": ["y"], "was": null}])
    );
}
