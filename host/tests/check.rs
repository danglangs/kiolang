use host::{Kio, Limits};
use lang::{check, CapTable};

#[test]
fn summarize_reports_two_lines() {
    let src = std::fs::read_to_string("../examples/summarize.kio").unwrap();
    let mut kio = Kio::new(Limits::default()).unwrap();
    let report = kio.check(&src).unwrap().unwrap();
    let want = lang::report(&check(&src, &CapTable::v0()).unwrap());
    assert_eq!(report.lines.len(), 2);
    assert_eq!(report.lines.join("\n"), want);
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn unknown_name_matches_lang_diag() {
    let src = "fn main() -> Int {\n    nope\n}\n";
    let mut kio = Kio::new(Limits::default()).unwrap();
    let got = kio.check(src).unwrap().unwrap_err();
    let want = check(src, &CapTable::v0()).unwrap_err();
    assert_eq!(got.len(), want.len());
    let (g, w) = (&got[0], &want[0]);
    assert_eq!((g.start as usize, g.end as usize), (w.span.start, w.span.end));
    assert_eq!(g.message, w.msg);
    assert_eq!(g.hint, w.hint);
    assert!(g.hint.is_some());
    assert_eq!(kio.import_calls(), 0);
}

/// No clock, randomness or environment reaches the guest, so fresh instances agree exactly.
#[test]
fn check_is_deterministic_across_instances() {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let run = || Kio::new(Limits::default()).unwrap().check(&src).unwrap().unwrap().lines;
    assert_eq!(run(), run());
}

/// A temp directory with `files`, opened as a grant.
fn dir(files: &[(&str, &[u8])]) -> (tempfile::TempDir, cap_std::fs::Dir) {
    let t = tempfile::tempdir().unwrap();
    for (p, b) in files {
        std::fs::write(t.path().join(p), b).unwrap();
    }
    let d = host::open_dir(t.path()).unwrap();
    (t, d)
}

fn invoices() -> String {
    std::fs::read_to_string("../examples/invoices.kio").unwrap()
}

#[test]
fn invoices_writes_total() {
    let (_i, input) = dir(&[("invoices.csv", b"a,40\nb,2\nbad\n")]);
    let (out_t, out) = dir(&[]);
    let mut kio = Kio::new(Limits::default()).unwrap();
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    let out = kio.run(&invoices(), grants).unwrap().unwrap();
    assert_eq!(out.report.lines.len(), 4);
    assert_eq!(out.value, r#"Enum("Result", 0, [Unit])"#);
    assert!(!out.failed);
    assert_eq!(std::fs::read(out_t.path().join("total.txt")).unwrap(), b"42");
    assert_eq!(kio.import_calls(), 2);
}

#[test]
fn summarize_writes_summary() {
    let src = std::fs::read_to_string("../examples/summarize.kio").unwrap();
    let (_i, input) = dir(&[("invoices.csv", b"12345")]);
    let (out_t, out) = dir(&[]);
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    Kio::new(Limits::default()).unwrap().run(&src, grants).unwrap().unwrap();
    assert_eq!(std::fs::read(out_t.path().join("summary.csv")).unwrap(), b"5");
}

/// Run `main(d: Dir)` as `body` on `d` and return the printed value.
fn on(d: cap_std::fs::Dir, body: &str) -> String {
    let src = format!(
        "fn main(d: Dir) -> Result<{}, IoErr> {{ {body} }}",
        if body.contains("create") { "()" } else { "Bytes" }
    );
    Kio::new(Limits::default())
        .unwrap()
        .run(&src, vec![("d".into(), d)])
        .unwrap()
        .unwrap()
        .value
}

const NOT_FOUND: &str = r#"Enum("Result", 1, [Enum("IoErr", 0, [])])"#;
const DENIED: &str = r#"Enum("Result", 1, [Enum("IoErr", 1, [])])"#;
const INVALID: &str = r#"Enum("Result", 1, [Enum("IoErr", 3, [])])"#;

#[test]
fn read_errors() {
    let (t, d) = dir(&[("a", b"x")]);
    std::fs::write(t.path().join("secret"), "s").unwrap();
    std::fs::create_dir_all(t.path().join("sub/in")).unwrap();
    let read = |p: &str| {
        let d = host::open_dir(&t.path().join("sub")).unwrap();
        on(d, &format!("d.read(\"{p}\")"))
    };
    assert_eq!(on(d, "d.read(\"missing\")"), NOT_FOUND);
    assert_eq!(read("../secret"), INVALID);
    assert_eq!(read("/etc/passwd"), INVALID);
    assert_eq!(read("in/../../secret"), INVALID);
    assert_eq!(read(""), INVALID);
    assert_eq!(read("a\u{0}b"), INVALID);
    assert_eq!(read("no/such/file"), NOT_FOUND);
}

#[test]
fn symlinks_out_are_refused() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join("secret"), "s").unwrap();
    std::fs::create_dir(outer.path().join("outside")).unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    use std::os::unix::fs::symlink;
    symlink(outer.path().join("secret"), root.join("file")).unwrap();
    symlink(outer.path().join("outside"), root.join("dir")).unwrap();
    symlink("../secret", root.join("rel")).unwrap();
    symlink("/etc/passwd", root.join("abs")).unwrap();
    let d = || host::open_dir(&root).unwrap();
    for p in ["file", "rel", "abs", "dir/x"] {
        assert_eq!(on(d(), &format!("d.read(\"{p}\")")), INVALID, "{p}");
    }
    assert_eq!(on(d(), "d.create(\"dir/new\", \"x\".to_bytes())"), INVALID);
    assert_eq!(on(d(), "d.create(\"file\", \"x\".to_bytes())"), DENIED);
    assert_eq!(std::fs::read(outer.path().join("secret")).unwrap(), b"s");
    assert_eq!(std::fs::read_dir(outer.path().join("outside")).unwrap().count(), 0);
}

#[test]
fn create_errors_leave_filesystem_unchanged() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("old"), "keep").unwrap();
    std::fs::create_dir(root.join("in")).unwrap();
    let d = || host::open_dir(&root).unwrap();
    let ok = r#"Enum("Result", 0, [Unit])"#;
    assert_eq!(on(d(), "d.create(\"old\", \"new\".to_bytes())"), DENIED);
    assert_eq!(std::fs::read(root.join("old")).unwrap(), b"keep");
    for p in ["../x", "/tmp/kio-x", "", "in/../../x"] {
        assert_eq!(on(d(), &format!("d.create(\"{p}\", \"x\".to_bytes())")), INVALID, "{p}");
    }
    assert_eq!(on(d(), "d.create(\"nodir/x\", \"x\".to_bytes())"), NOT_FOUND);
    assert!(!outer.path().join("x").exists() && !root.join("nodir").exists());
    assert_eq!(on(d(), "d.create(\"new\", \"x\".to_bytes())"), ok);
    assert_eq!(std::fs::read(root.join("new")).unwrap(), b"x");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
}

#[test]
fn open_dir_refuses_missing_and_files() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("f"), "x").unwrap();
    assert!(host::open_dir(&t.path().join("nope")).is_err());
    assert!(host::open_dir(&t.path().join("f")).is_err());
}

fn rejected(names: &[&str], message: &str, hint: &str) {
    let ts: Vec<_> = names.iter().map(|_| dir(&[])).collect();
    let grants = names.iter().zip(ts).map(|(n, (_t, d))| (n.to_string(), d)).collect();
    let mut kio = Kio::new(Limits::default()).unwrap();
    let diags = kio.run(&invoices(), grants).unwrap().unwrap_err();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, message);
    assert!(diags[0].hint.as_deref().unwrap().contains(hint));
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn duplicate_grant() {
    rejected(
        &["input", "out", "out"],
        "grant `out` is given more than once",
        "one name",
    );
}

#[test]
fn extra_grant() {
    rejected(
        &["input", "out", "extra"],
        "grant `extra` is not a parameter of `main`",
        "input, out",
    );
}

#[test]
fn missing_grant_is_no_grant_error() {
    let (_t, d) = dir(&[]);
    let mut kio = Kio::new(Limits::default()).unwrap();
    let diags = kio.run(&invoices(), vec![("input".into(), d)]).unwrap().unwrap_err();
    assert_eq!(diags[0].message, "no grant for `out: Dir`");
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn runtime_error_has_span() {
    let src = "fn main(d: Dir) -> Int {\n    let xs: List<Int> = [];\n    xs[1]\n}\n";
    let (_t, d) = dir(&[]);
    let diags = Kio::new(Limits::default())
        .unwrap()
        .run(src, vec![("d".into(), d)])
        .unwrap()
        .unwrap_err();
    assert_eq!(diags.len(), 1);
    assert!(diags[0].end > diags[0].start);
    assert!(diags[0].hint.is_none());
}

/// G4: on the default stack the deepest recursion stops with the depth error, never a trap,
/// in debug and release builds of the host.
#[test]
fn depth_budget_fits_the_default_stack() {
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { down(n - 1) } } fn main() -> Int { down(100000) }";
    let diags = Kio::new(Limits::default())
        .unwrap()
        .run(src, vec![])
        .unwrap()
        .unwrap_err();
    assert_eq!(diags[0].message, "evaluation is too deep");
}

#[test]
fn small_stack_traps_with_stack_overflow() {
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { down(n - 1) } } fn main() -> Int { down(1000) }";
    let err = Kio::new(Limits {
        stack: 64 << 10,
        ..Limits::default()
    })
    .unwrap()
    .run(src, vec![])
    .unwrap_err();
    assert_eq!(
        err.downcast_ref::<wasmtime::Trap>(),
        Some(&wasmtime::Trap::StackOverflow)
    );
}

/// `main` returning `Err` is a failed run: the host exits nonzero and still shows the value.
#[test]
fn main_err_marks_the_run_failed() {
    let (_i, input) = dir(&[("invoices.csv", b"a,1\n")]);
    let (_o, out) = dir(&[("total.txt", b"old")]);
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    let out = Kio::new(Limits::default())
        .unwrap()
        .run(&invoices(), grants)
        .unwrap()
        .unwrap();
    assert!(out.failed);
    assert_eq!(out.value, r#"Enum("Result", 1, [Enum("IoErr", 1, [])])"#);
}

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let limits = Limits {
        fuel: 10_000_000,
        ..Limits::default()
    };
    let src = "fn main() -> Int { while true {} 0 }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "out of fuel (limit 10000000; raise it with --fuel)"
    );
}

/// The deadline fires once a slow host call returns control to the guest.
#[test]
fn slow_host_call_times_out() {
    let limits = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let (_t, d) = dir(&[("a", b"x")]);
    let src = "fn main(d: Dir) -> Int { let r = d.read(\"a\"); while true {} 0 }";
    let mut kio = Kio::new(limits).unwrap();
    kio.slow_reads(std::time::Duration::from_millis(1500));
    let err = kio.run(src, vec![("d".into(), d)]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "timed out after 1 s (raise it with --timeout)"
    );
}

/// H3: a program that allocates without bound stops at the memory limit with a message naming it.
#[test]
fn unbounded_allocation_runs_out_of_memory() {
    let limits = Limits {
        memory: 16 << 20,
        ..Limits::default()
    };
    let src = "fn main() -> Int { let mut xs: List<Str> = []; while true { xs.push(\"0123456789abcdef\"); } 0 }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "out of memory (limit 16 MB; raise it with --memory)"
    );
    assert!(kio.peak_memory() <= 16 << 20);
}

/// H3: recursion past the Wasm stack stops with a message naming the stack limit.
#[test]
fn deep_recursion_overflows_a_small_stack() {
    let limits = Limits {
        stack: 64 << 10,
        ..Limits::default()
    };
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { down(n - 1) } } fn main() -> Int { down(100000) }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "stack overflow (limit 64 KB; raise it with --stack)"
    );
}
