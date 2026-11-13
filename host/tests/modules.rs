use kio_host::{Kio, Limits};
use std::process::Command;

fn files(web: &str) -> Vec<(String, String)> {
    let app = "mod web;\nfn main() -> Int {\n    web::inc(1)\n}\n";
    vec![("app.kio".into(), app.into()), ("web.kio".into(), web.into())]
}

const GOOD: &str = "pub fn inc(n: Int) -> Int {\n    n + 1\n}\n";

#[test]
fn two_files_check_compile_and_run() {
    let mut kio = Kio::new(Limits::default()).unwrap();
    let report = kio.check_files(&files(GOOD)).unwrap().unwrap();
    assert_eq!(
        report.lines,
        [
            "main : none",
            "",
            "1 more uses no capability; --all lists every function"
        ]
    );
    let out = kio.run_files(&files(GOOD), vec![], vec![]).unwrap().unwrap();
    assert_eq!(out.value, "2");
}

#[test]
fn diag_in_the_module_carries_file_1() {
    let mut kio = Kio::new(Limits::default()).unwrap();
    let bad = "pub fn inc(n: Int) -> Int {\n    nope\n}\n";
    let diags = kio.check_files(&files(bad)).unwrap().unwrap_err();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].file, 1);
    let diags = kio.run_files(&files(bad), vec![], vec![]).unwrap().unwrap_err();
    assert_eq!(diags[0].file, 1);
}

fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (out.status.code().unwrap(), text(out.stdout), text(out.stderr))
}

fn project(web: Option<&str>) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("app.kio"), files(GOOD)[0].1.as_str()).unwrap();
    if let Some(w) = web {
        std::fs::write(d.path().join("web.kio"), w).unwrap();
    }
    d
}

#[test]
fn cli_runs_the_example() {
    let (code, out, _) = kio(&["run", "../examples/web/app.kio"]);
    assert_eq!((code, out.lines().last()), (0, Some("41")));
    assert_eq!(kio(&["check", "../examples/web/app.kio"]).0, 0);
}

#[test]
fn cli_prints_module_diags_with_the_module_name() {
    let d = project(Some("pub fn inc(n: Int) -> Int {\n    nope\n}\n"));
    let app = d.path().join("app.kio");
    let (code, _, err) = kio(&["check", app.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.starts_with("web.kio:2: error: "), "{err}");
}

#[test]
fn cli_missing_module_file_is_exit_2() {
    let d = project(None);
    let app = d.path().join("app.kio");
    let (code, _, err) = kio(&["run", app.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(err.starts_with("kio: mod web: cannot read "), "{err}");
}

const P8_APP: &str = r#"mod tests;
fn check(key: Hmac, m: Bytes) -> Bool { key.verify(m, "x".to_bytes()) }
fn mint(key: Hmac) -> Int { let _s = key.sign("m".to_bytes()); 0 }
fn main(key: Hmac) -> Bool { check(key, "m".to_bytes()) }
"#;
const P8_TESTS: &str = r#"fn sig_of(key: Hmac) -> Int { let _s = key.sign("m".to_bytes()); 0 }
#[test]
fn signs(key: Hmac) { assert_eq(sig_of(key), 0); }
"#;

/// P8: `kio check` shows what `main` reaches, test-only helpers apart; `--all` every function.
/// The host's checks still read `main`'s line, which neither form changes.
#[test]
fn check_shows_what_main_reaches() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("app.kio"), P8_APP).unwrap();
    std::fs::write(d.path().join("tests.kio"), P8_TESTS).unwrap();
    let app = d.path().join("app.kio");
    let app = app.to_str().unwrap();
    let want = "main          : key verify\ncheck         : key verify\n\nreached only from tests:\ntests::sig_of : key sign\n\n1 is unreached (1 uses a capability); --all lists every function\n";
    assert_eq!(kio(&["check", app]), (0, want.into(), "".into()));
    let all =
        "check         : key verify\nmain          : key verify\nmint          : key sign\ntests::sig_of : key sign\n";
    assert_eq!(kio(&["check", app, "--all"]), (0, all.into(), "".into()));
    // `:verify` is refused only for a `sign` on `main`'s line, as before P8: neither `mint` nor
    // the test helper is reached from `main`.
    let grants = d.path().join("app.grants");
    std::fs::write(&grants, "--hmac key=env:P8_KEY:verify\n").unwrap();
    let (code, out, err) = kio(&["check", app, "--grants", grants.to_str().unwrap()]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, want, ""));
    // Reached, the `sign` is on `main`'s line and refused.
    std::fs::write(
        d.path().join("app.kio"),
        P8_APP.replace("{ check(", "{ mint(key) == 0 && check("),
    )
    .unwrap();
    let (code, _, err) = kio(&["check", app, "--grants", grants.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("can sign with it"), "{err}");
}
