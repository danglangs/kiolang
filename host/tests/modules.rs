use kio_host::{Kio, Limits};
use std::process::Command;

fn files(web: &str) -> Vec<(String, String)> {
    let app = "mod web;\nfn main() -> Int {\n    web::inc(1)\n}\n";
    vec![("app.kio".into(), app.into()), ("web.kio".into(), web.into())]
}

const GOOD: &str = "pub fn inc(n: Int) -> Int {\n    n + 1\n}\n";

#[test]
fn two_files_check_and_run_through_the_guest() {
    let mut kio = Kio::new(Limits::default()).unwrap();
    let report = kio.check_files(&files(GOOD)).unwrap().unwrap();
    assert_eq!(report.lines.len(), 2);
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
