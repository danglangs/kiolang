//! D1: `Dir.list`, `write`, `remove` and `exists` on real temp directories.
use kio_host::{Kio, Limits};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

/// Run `main(d: Dir) -> Result<ty, IoErr>` as `body` on `root` and return the printed value.
fn on(root: &Path, ty: &str, body: &str) -> String {
    let src = format!("fn main(d: Dir) -> Result<{ty}, IoErr> {{ {body} }}");
    let d = kio_host::open_dir(root).unwrap();
    let out = Kio::new(Limits::default()).unwrap().run(&src, vec![("d".into(), d)]);
    out.unwrap().unwrap().value
}

fn list(root: &Path, p: &str) -> String {
    on(root, "List<Str>", &format!("d.list(\"{p}\")"))
}
fn write(root: &Path, p: &str, data: &str) -> String {
    on(root, "()", &format!("d.write(\"{p}\", \"{data}\".to_bytes())"))
}
fn remove(root: &Path, p: &str) -> String {
    on(root, "()", &format!("d.remove(\"{p}\")"))
}
fn exists(root: &Path, p: &str) -> String {
    on(root, "Bool", &format!("d.exists(\"{p}\")"))
}

const OK: &str = "Ok(())";
const NOT_FOUND: &str = "IoErr::NotFound";
const DENIED: &str = "IoErr::Denied";
const INVALID: &str = "IoErr::InvalidPath";

fn mkfifo(p: &Path) {
    assert!(std::process::Command::new("mkfifo").arg(p).status().unwrap().success());
}

/// The names in `root`, sorted.
fn names(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[test]
fn list_sorts_marks_dirs_and_leaves_out_the_rest() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join("b"), "").unwrap();
    fs::write(r.join("C"), "").unwrap();
    fs::create_dir(r.join("a")).unwrap();
    fs::write(r.join("a/inner"), "").unwrap();
    symlink("b", r.join("link")).unwrap();
    mkfifo(&r.join("fifo"));
    assert_eq!(list(r, ""), r#"Ok(["C", "a/", "b"])"#);
    assert_eq!(list(r, "."), r#"Ok(["C", "a/", "b"])"#);
    assert_eq!(list(r, "a"), r#"Ok(["inner"])"#);
    assert_eq!(list(r, "missing"), NOT_FOUND);
    assert_eq!(list(r, "b"), DENIED);
    assert_eq!(list(r, "fifo"), DENIED);
}

#[test]
fn list_over_the_cap_is_too_large() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..10_000 {
        fs::write(t.path().join(i.to_string()), "").unwrap();
    }
    let len = "match d.list(\"\") { Ok(n) => Ok(n.len()), Err(e) => Err(e) }";
    assert_eq!(on(t.path(), "Int", len), "Ok(10000)");
    fs::write(t.path().join("one-more"), "").unwrap();
    assert_eq!(on(t.path(), "Int", len), "IoErr::TooLarge");
}

#[test]
fn write_creates_and_replaces_leaving_no_temp() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::create_dir(r.join("sub")).unwrap();
    assert_eq!(write(r, "f", "one"), OK);
    assert_eq!(write(r, "f", "two"), OK);
    assert_eq!(fs::read(r.join("f")).unwrap(), b"two");
    assert_eq!(write(r, "sub/g", "x"), OK);
    assert_eq!(fs::read(r.join("sub/g")).unwrap(), b"x");
    assert_eq!(names(r), ["f", "sub"]);
    assert_eq!(names(&r.join("sub")), ["g"]);
}

#[test]
fn write_errors_leave_the_filesystem_unchanged() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::create_dir(r.join("sub")).unwrap();
    fs::write(r.join("target"), "keep").unwrap();
    symlink("target", r.join("link")).unwrap();
    mkfifo(&r.join("fifo"));
    assert_eq!(write(r, "sub", "x"), DENIED);
    assert_eq!(write(r, "link", "x"), DENIED);
    assert_eq!(write(r, "fifo", "x"), DENIED);
    assert_eq!(write(r, "nodir/x", "x"), NOT_FOUND);
    assert_eq!(fs::read(r.join("target")).unwrap(), b"keep");
    assert_eq!(names(r), ["fifo", "link", "sub", "target"]);
    assert_eq!(names(&r.join("sub")), Vec::<String>::new());
}

/// A write that fails after the temp file is created removes it and keeps the old file.
#[test]
fn failed_write_keeps_the_old_file_and_no_temp() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("f"), "old").unwrap();
    let src = "fn main(d: Dir) -> Result<(), IoErr> { d.write(\"f\", \"new\".to_bytes()) }";
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.fail_writes();
    let d = kio_host::open_dir(t.path()).unwrap();
    assert_eq!(kio.run(src, vec![("d".into(), d)]).unwrap().unwrap().value, DENIED);
    assert_eq!(fs::read(t.path().join("f")).unwrap(), b"old");
    assert_eq!(names(t.path()), ["f"]);
}

#[test]
fn remove_regular_files_only() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join("f"), "").unwrap();
    fs::write(r.join("target"), "").unwrap();
    fs::create_dir(r.join("sub")).unwrap();
    symlink("target", r.join("link")).unwrap();
    mkfifo(&r.join("fifo"));
    assert_eq!(remove(r, "f"), OK);
    assert_eq!(remove(r, "f"), NOT_FOUND);
    assert_eq!(remove(r, "sub"), DENIED);
    assert_eq!(remove(r, "link"), DENIED);
    assert_eq!(remove(r, "fifo"), DENIED);
    assert_eq!(names(r), ["fifo", "link", "sub", "target"]);
}

#[test]
fn exists_files_and_dirs() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join("f"), "").unwrap();
    fs::create_dir(r.join("sub")).unwrap();
    assert_eq!(exists(r, "f"), "Ok(true)");
    assert_eq!(exists(r, "sub"), "Ok(true)");
    assert_eq!(exists(r, "missing"), "Ok(false)");
    assert_eq!(exists(r, "f/x"), "Ok(false)");
    // A symlink inside the root is followed, as by `read`; a FIFO is neither file nor directory.
    symlink("f", r.join("link")).unwrap();
    mkfifo(&r.join("fifo"));
    assert_eq!(exists(r, "link"), "Ok(true)");
    assert_eq!(exists(r, "fifo"), "Ok(false)");
}

/// Every new method refuses the paths `read` refuses, and touches nothing outside the root.
#[test]
fn escapes_are_invalid_path() {
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join("secret"), "s").unwrap();
    fs::create_dir(outer.path().join("outside")).unwrap();
    fs::write(outer.path().join("outside/x"), "s").unwrap();
    let root = outer.path().join("root");
    fs::create_dir_all(root.join("in")).unwrap();
    symlink(outer.path().join("outside"), root.join("dir")).unwrap();
    symlink("../secret", root.join("rel")).unwrap();
    let long = "a/".repeat(2049);
    let bad = [
        "../secret",
        "/etc",
        "in/../../secret",
        "a\u{0}b",
        "dir/x",
        "rel",
        long.as_str(),
    ];
    for p in bad {
        assert_eq!(write(&root, p, "x"), INVALID, "write {p}");
        assert_eq!(remove(&root, p), INVALID, "remove {p}");
        assert_eq!(exists(&root, p), INVALID, "exists {p}");
    }
    for p in ["..", "../outside", "/", "dir", "in/../..", long.as_str()] {
        assert_eq!(list(&root, p), INVALID, "list {p}");
    }
    for p in ["", "\u{0}"] {
        assert_eq!(write(&root, p, "x"), INVALID);
        assert_eq!(remove(&root, p), INVALID);
        assert_eq!(exists(&root, p), INVALID);
    }
    assert_eq!(fs::read(outer.path().join("secret")).unwrap(), b"s");
    assert_eq!(names(&outer.path().join("outside")), ["x"]);
    assert_eq!(names(outer.path()), ["outside", "root", "secret"]);
}

#[test]
fn report_lists_the_new_methods() {
    let t = tempfile::tempdir().unwrap();
    let src = "fn main(d: Dir) -> Result<Bool, IoErr> { \
        d.write(\"f\", \"x\".to_bytes())?; let n = d.list(\"\")?; d.remove(\"f\")?; d.exists(\"f\") }";
    let d = kio_host::open_dir(t.path()).unwrap();
    let out = Kio::new(Limits::default()).unwrap().run(src, vec![("d".into(), d)]);
    let out = out.unwrap().unwrap();
    assert_eq!(out.value, "Ok(false)");
    assert_eq!(out.report.lines, ["main : d exists list remove write"]);
}
