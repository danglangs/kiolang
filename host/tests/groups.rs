//! L32: capability groups in `kio test` and `kio run`. A test taking a capability struct is
//! granted by field name; the host's checks see the flattened fields.
use std::path::Path;
use std::process::Command;

fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

struct Case {
    dir: tempfile::TempDir,
    app: String,
}

impl Case {
    fn new(src: &str) -> Case {
        let dir = tempfile::tempdir().unwrap();
        let app = write(dir.path(), "app.kio", src);
        Case { dir, app }
    }

    fn file(&self, name: &str, text: &str) -> String {
        write(self.dir.path(), name, text)
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).to_str().unwrap().to_string()
    }

    fn test(&self, args: &[&str]) -> (i32, String, String) {
        let mut all = vec!["test", self.app.as_str()];
        all.extend(args);
        kio(&all)
    }
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

const APP: &str = r#"struct Env { d: Dir, s: Secret }

fn save(env: Env, name: Str) -> Result<(), IoErr> {
  if env.s.matches("tok") { env.d.write(name, "x".to_bytes()) } else { Ok(()) }
}

fn main(d: Dir, s: Secret) -> Result<(), IoErr> { save(Env { d: d, s: s }, "a") }

#[test]
fn writes_when_the_token_matches(env: Env) -> Result<(), IoErr> {
  save(env, "a")?;
  assert(env.d.exists("a")?, "written");
  Ok(())
}
"#;

/// A test's leaves are granted by field name from the grants file, and by type without one; a
/// missing grant names the field.
#[test]
fn test_grants_bind_by_field_name() {
    let c = Case::new(APP);
    let (code, out, _) = c.test(&[]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("test `writes_when_the_token_matches` needs `s: Secret`"),
        "{out}"
    );
    let tok = c.file("tok.txt", "tok");
    let grants = c.file("test.grants", &format!("--secret s=file:{tok}\n"));
    let (code, out, err) = c.test(&["--grants", &grants]);
    assert_eq!(
        (code, out.as_str()),
        (0, "ok   writes_when_the_token_matches\n1 passed, 0 failed\n"),
        "{err}"
    );
    // A default `Dir` and `Clock` need no grants file at all.
    let c = Case::new(
        "struct Env { d: Dir, c: Clock }
fn stamp(env: Env) -> Result<(), IoErr> { env.d.write(\"t\", env.c.now_ms().to_str().to_bytes()) }
#[test]
fn stamps(env: Env) -> Result<(), IoErr> { stamp(env)?; assert_eq(env.d.read(\"t\")?.to_str(), \"1767225600000\"); Ok(()) }",
    );
    let (code, out, err) = c.test(&[]);
    assert_eq!((code, out.as_str()), (0, "ok   stamps\n1 passed, 0 failed\n"), "{err}");
}

/// The host's startup checks see the fields: a `:ro` dir the test writes, and the property rules.
#[test]
fn host_checks_see_the_fields() {
    let c = Case::new(APP);
    let tok = c.file("tok.txt", "tok");
    std::fs::create_dir(c.dir.path().join("data")).unwrap();
    let data = c.path("data");
    let grants = c.file("ro.grants", &format!("--secret s=file:{tok}\n--dir d={data}:ro\n"));
    let (code, out, _) = c.test(&["--grants", &grants]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("`d` is granted `:ro` but the program can write it (`d write`)"),
        "{out}"
    );
    // Row 14: a property may not take a fake `Http` or a writable store through a field.
    let c = Case::new(
        "struct Env { api: Http, db: Db }
fn ping(env: Env) -> Int { let _ = env.api.get(\"/\"); match env.db.query(\"select 1\", []) { Ok(_) => 1, Err(_) => 0 } }
#[test]
fn p(xs: List<Int>, env: Env) { let _ = ping(env); }",
    );
    let fake = c.file("api.http", "GET /\n200\n\nok\n");
    let grants = c.file("test.grants", &format!("--http api=fake:{fake}\n"));
    let (code, out, _) = c.test(&["--grants", &grants]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("a property may not take `api: Http` yet; a fake's answers are counted per test, not per case"),
        "{out}"
    );
    let db = c.path("x.db");
    let grants = c.file("db.grants", &format!("--http api=fake:{fake}\n--db db={db}\n"));
    let (_, out, _) = c.test(&["--grants", &grants]);
    assert!(
        out.contains("a property may not take `api: Http` yet")
            || out.contains("`db: Db` from the grants file would share its writes"),
        "{out}"
    );
    let c = Case::new(
        "struct Env { db: Db }
fn n(env: Env) -> Int { match env.db.query(\"select 1\", []) { Ok(_) => 1, Err(_) => 0 } }
#[test]
fn p(xs: List<Int>, env: Env) { let _ = n(env); }",
    );
    let db = c.path("x.db");
    let grants = c.file("db.grants", &format!("--db db={db}\n"));
    let (code, out, _) = c.test(&["--grants", &grants]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("a property runs many times; `db: Db` from the grants file would share its writes across cases"),
        "{out}"
    );
}

/// Rule 8 is refused at the test's signature, before any grant is opened: the grants file names a
/// secret file that does not exist, and nothing complains about it.
#[test]
fn rule8_before_any_grant_opens() {
    let c = Case::new(
        "struct Env { d: Dir, s: Secret }
#[test]
fn t(env: Env, d: Int) {}",
    );
    let grants = c.file("test.grants", "--secret s=file:/nonexistent/tok\n");
    let (code, out, err) = c.test(&["--grants", &grants]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("test `t` names `d` twice: `env.d` and `d`"), "{err}");
    assert!(err.contains("hint: rename the field or drop the parameter"), "{err}");
    assert!(!err.contains("nonexistent"), "{err}");
}

/// `kio run`: a program passing a group runs compiled with the same calls as the interpreter's
/// report says, and the audit log names the grants.
#[test]
fn run_charges_the_grants() {
    let c = Case::new(
        "struct Env { d: Dir, c: Clock }
fn stamp(env: Env) -> Result<Int, IoErr> { env.d.write(\"t\", env.c.now_ms().to_str().to_bytes())?; Ok(env.d.read(\"t\")?.len()) }
fn main(input: Dir, now: Clock) -> Result<Int, IoErr> { stamp(Env { d: input, c: now }) }",
    );
    std::fs::create_dir(c.dir.path().join("data")).unwrap();
    let data = c.path("data");
    let audit = c.path("audit.log");
    let (code, out, err) = kio(&[
        "run",
        &c.app,
        "--dir",
        &format!("input={data}"),
        "--clock",
        "now=fixed:1000",
        "--audit",
        &audit,
    ]);
    assert_eq!((code, out.as_str()), (0, "Ok(4)\n"), "{err}");
    let (_, report, _) = kio(&["check", &c.app]);
    assert_eq!(
        report,
        "main  : input read write, now now_ms\nstamp : env.d read write, env.c now_ms\n"
    );
    let log = std::fs::read_to_string(&audit).unwrap();
    assert!(log.contains(r#""cap":"input","kind":"Dir","method":"write""#), "{log}");
    assert!(log.contains(r#""cap":"now","kind":"Clock","method":"now_ms""#), "{log}");
    assert_eq!(std::fs::read_to_string(c.dir.path().join("data/t")).unwrap(), "1000");
}
