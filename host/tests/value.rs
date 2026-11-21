//! H35: `Value` is a run-time value the operator gives `main` (`--value name=TEXT`), read with
//! `get`. Interpreted and compiled, through `kio run`, `kio check --grants` and `kio test`.
use kio_host::{Interp, Kio, Limits, RunResult, Sources, ValueText};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

/// An audit log in memory.
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn value(text: &str) -> Sources {
    Sources {
        values: vec![("v".into(), ValueText::new(text.into()).unwrap())],
        ..Sources::default()
    }
}

fn run(src: &str, sources: Sources, compiled: bool, audit: Option<Buf>) -> RunResult {
    if compiled {
        let mut k = Kio::new(Limits::default()).unwrap();
        if let Some(a) = audit {
            k.audit(a, vec![]);
        }
        k.grant_sources(sources);
        k.run(src, vec![]).unwrap()
    } else {
        let mut k = Interp::new(Limits::default()).unwrap();
        if let Some(a) = audit {
            k.audit(a, vec![]);
        }
        k.grant_sources(sources);
        k.run(src, vec![]).unwrap()
    }
}

fn shown(r: RunResult) -> String {
    match r {
        Ok(o) => o.value,
        Err(d) => format!("error: {}", d[0].message),
    }
}

/// `src`'s value from both paths, which must agree.
fn both(src: &str, sources: impl Fn() -> Sources) -> String {
    let c = shown(run(src, sources(), true, None));
    let i = shown(run(src, sources(), false, None));
    assert_eq!(c, i, "compiled and interpreted disagree on:\n{src}");
    c
}

const PROGRAM: &str = r#"fn main(v: Value) -> Str { "[" + v.get() + "] " + v.get().len().to_str() }"#;

#[test]
fn a_value_and_an_empty_one_on_both_paths() {
    assert_eq!(both(PROGRAM, || value("24")), "[24] 2");
    assert_eq!(both(PROGRAM, || value("")), "[] 0");
    assert_eq!(both(PROGRAM, || value("héllo wörld")), "[héllo wörld] 13");
    let big = "x".repeat(4096);
    assert_eq!(both(PROGRAM, || value(&big)), format!("[{big}] 4096"));
}

#[test]
fn the_rules_of_a_value() {
    assert!(ValueText::new(String::new()).is_ok());
    assert!(ValueText::new("x".repeat(4096)).is_ok());
    let e = ValueText::new("x".repeat(4097)).unwrap_err();
    assert!(
        e.starts_with("the value is over 4096 bytes\n  hint: `--value` takes at most 4 KB"),
        "{e}"
    );
    for bad in ["a\tb", "a\nb", "a\u{7f}b", "a\u{85}b", "\u{1b}[31m"] {
        let e = ValueText::new(bad.into()).unwrap_err();
        assert!(e.starts_with("the value holds a control character (U+"), "{bad:?}: {e}");
        assert!(e.contains("hint: `--value` takes one line of plain text"), "{e}");
    }
}

/// The header shows the value as `kio review`'s grants line does (ASCII, `\` doubled, the rest
/// escaped); each call's line has the method and the bytes read.
#[test]
fn the_audit_header_shows_the_value() {
    for compiled in [true, false] {
        let audit = Buf::default();
        let v = shown(run(
            PROGRAM,
            value("tz=Europe/Zürich\\x"),
            compiled,
            Some(audit.clone()),
        ));
        assert_eq!(v, "[tz=Europe/Zürich\\x] 19");
        let text = String::from_utf8(audit.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(
            lines[0].contains(r#""name":"v","kind":"Value","target":"tz=Europe/Z\\u{fc}rich\\\\x""#),
            "{text}"
        );
        assert!(
            lines[1].contains(r#""cap":"v","kind":"Value","method":"get","arg":"get","bytes":19,"result":"ok""#),
            "{text}"
        );
    }
}

fn kio(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap()
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

const HOURS: &str = r#"fn main(hours: Value) -> Int { hours.get().parse_int().unwrap_or(-1) * 60 }"#;

#[test]
fn the_flag_grants_a_value_and_check_grants_binds_it() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "hours.kio", HOURS);
    let audit = t.path().join("audit.log");
    let out = kio(&["run", &prog, "--value", "hours=24", "--audit", audit.to_str().unwrap()]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1440");
    let log = std::fs::read_to_string(&audit).unwrap();
    assert!(log.contains(r#""name":"hours","kind":"Value","target":"24""#), "{log}");
    assert!(
        log.contains(r#""kind":"Value","method":"get","arg":"get","bytes":2,"result":"ok""#),
        "{log}"
    );
    // An empty value, and one holding `=` (split at the first).
    let out = kio(&["run", &prog, "--value", "hours="]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "-60");
    let out = kio(&["run", &prog, "--value", "hours=a=b"]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "-60");
    // The report names the one method; `kio check --grants` binds the flag.
    let out = kio(&["check", &prog]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "main : hours get");
    let g = write(t.path(), "h.grants", "--value hours=24\n");
    let out = kio(&["check", &prog, "--grants", &g]);
    assert_eq!(
        (out.status.code(), String::from_utf8(out.stdout).unwrap().trim()),
        (Some(0), "main : hours get")
    );
    // Ungranted: the hint names the flag.
    let out = kio(&["run", &prog]);
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("no grant for `hours: Value`"), "{err}");
    assert!(err.contains("grant it: `--value hours=TEXT`"), "{err}");
}

#[test]
fn bad_values_are_refused_at_start() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "hours.kio", HOURS);
    let huge = format!("hours={}", "9".repeat(4097));
    let cases: [(&str, &str); 4] = [
        (
            &huge,
            "kio: --value hours: the value is over 4096 bytes\n  hint: `--value` takes at most 4 KB; put a longer value in a file under a `--dir`\n",
        ),
        (
            "hours=2\t4",
            "kio: --value hours: the value holds a control character (U+0009)\n  hint: `--value` takes one line of plain text; put more in a file under a `--dir`\n",
        ),
        ("hours", "kio: --value hours: expected name=TEXT\n"),
        // A usage error echoes the spec escaped: no control character reaches the terminal.
        ("hours\u{1b}[31m", "kio: --value hours\\u{1b}[31m: expected name=TEXT\n"),
    ];
    for (spec, want) in cases {
        let out = kio(&["run", &prog, "--value", spec]);
        let err = String::from_utf8(out.stderr).unwrap();
        assert_eq!((out.status.code(), err.as_str()), (Some(2), want), "{spec:?}");
        assert!(out.stdout.is_empty());
    }
    // The same from a grants file, and for another flag's usage error.
    let g = write(t.path(), "esc.grants", "--value hours\u{1b}[31m\n");
    let out = kio(&["run", &prog, "--grants", &g]);
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(err, "kio: --value hours\\u{1b}[31m: expected name=TEXT\n");
    let out = kio(&["run", &prog, "--db", "\u{1b}]0;x\u{7}"]);
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        err,
        "kio: --db \\u{1b}]0;x\\u{07}: expected name=path, name=path:ro or name=:memory:\n"
    );
}

#[test]
fn a_test_needs_the_grant_from_a_grants_file() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(
        t.path(),
        "app.kio",
        r#"#[test]
fn reads(hours: Value) { assert_eq(hours.get(), "24"); }
"#,
    );
    let out = kio(&["test", &prog]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("test `reads` needs `hours: Value`")
            && text.contains("hint: name it in a --grants file: `--value hours=..`"),
        "{text}"
    );
    let g = write(t.path(), "t.grants", "--value hours=24\n");
    let out = kio(&["test", &prog, "--grants", &g]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");
}
