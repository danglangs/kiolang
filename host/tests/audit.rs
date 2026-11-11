use kio_host::{Interp, Kio, Limits};
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    /// The log's lines, with the `ms` field removed so runs compare equal.
    fn lines(&self) -> Vec<String> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|l| match l.find(r#","ms":"#) {
                Some(i) => format!("{}}}", &l[..i]),
                None => l.to_string(),
            })
            .collect()
    }
}

/// Run the primer with `input` holding `invoices.csv` and `out` holding `out_files`.
fn primer(limits: Limits, out_files: &[&str], hide: Option<(&str, &str)>) -> (Log, bool) {
    primer_on(false, limits, out_files, hide)
}

/// `primer`, compiled if `compiled`.
fn primer_on(compiled: bool, limits: Limits, out_files: &[&str], hide: Option<(&str, &str)>) -> (Log, bool) {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("invoices.csv"), "a,1\nb,2\n").unwrap();
    for f in out_files {
        std::fs::write(b.path().join(f), "old").unwrap();
    }
    let dirs = vec![
        ("input".to_string(), kio_host::open_dir(a.path()).unwrap()),
        ("out".to_string(), kio_host::open_dir(b.path()).unwrap()),
    ];
    let log = Log::default();
    let paths = vec![("input".into(), "/in".into()), ("out".into(), "/out".into())];
    if compiled {
        let mut c = Kio::new(limits).unwrap();
        c.audit(log.clone(), paths);
        if let Some((cap, m)) = hide {
            c.hide_from_report(cap, m);
        }
        let stopped = c.run_with(&src, dirs, vec![]).is_err();
        return (log, stopped);
    }
    let mut kio = Interp::new(limits).unwrap();
    kio.audit(log.clone(), paths);
    if let Some((c, m)) = hide {
        kio.hide_from_report(c, m);
    }
    let stopped = kio.run(&src, dirs).is_err();
    (log, stopped)
}

/// C4: compiled code makes the same host calls through the same host functions, so its log is
/// the interpreted one's, apart from durations and the trailer's peak memory and fuel. This
/// covers a refused call, a quota stop and the report check.
#[test]
fn compiled_log_is_the_interpreted_one() {
    let calls = Limits {
        calls: 1,
        ..Limits::default()
    };
    let io = Limits {
        io: 4,
        ..Limits::default()
    };
    type Case<'a> = (Limits, &'a [&'a str], Option<(&'a str, &'a str)>);
    let cases: [Case; 5] = [
        (Limits::default(), &[], None),
        (Limits::default(), &["total.txt"], None),
        (calls, &[], None),
        (io, &[], None),
        (Limits::default(), &[], Some(("out", "create"))),
    ];
    let cut = |l: &String| match l.find(r#","peak_memory":"#) {
        Some(i) => l[..i].to_string(),
        None => l.clone(),
    };
    for (limits, out_files, hide) in cases {
        let (i, i_stopped) = primer_on(false, limits, out_files, hide);
        let (c, c_stopped) = primer_on(true, limits, out_files, hide);
        let (i, c): (Vec<_>, Vec<_>) = (i.lines().iter().map(cut).collect(), c.lines().iter().map(cut).collect());
        assert_eq!(c, i, "{out_files:?} {hide:?}");
        assert_eq!(c_stopped, i_stopped);
        assert!(c.len() >= 3, "{c:?}");
    }
}

#[test]
fn primer_log_has_header_two_calls_and_trailer() {
    let (log, stopped) = primer(Limits::default(), &[], None);
    let lines = log.lines();
    assert!(!stopped);
    assert_eq!(lines.len(), 4);
    let hash = lines[0].split(r#""program_hash":""#).nth(1).unwrap();
    assert_eq!(hash.find('"'), Some(64));
    assert!(lines[0].starts_with(r#"{"event":"start","program_hash":""#));
    assert!(lines[0].contains(
        r#""grants":[{"name":"input","kind":"Dir","target":"/in"},{"name":"out","kind":"Dir","target":"/out"}]"#
    ));
    assert_eq!(
        lines[1],
        r#"{"event":"call","seq":1,"cap":"input","kind":"Dir","method":"read","arg":"invoices.csv","bytes":8,"result":"ok"}"#
    );
    assert_eq!(
        lines[2],
        r#"{"event":"call","seq":2,"cap":"out","kind":"Dir","method":"create","arg":"total.txt","bytes":1,"result":"ok"}"#
    );
    assert!(lines[3].starts_with(r#"{"event":"end","outcome":"ok","calls":2,"io":9,"peak_memory":"#));
}

#[test]
fn refused_create_is_logged() {
    let (log, _) = primer(Limits::default(), &["total.txt"], None);
    let lines = log.lines();
    assert!(
        lines[2].contains(r#""method":"create""#) && lines[2].contains(r#""result":"exists""#),
        "{lines:?}"
    );
    assert!(lines[3].contains(r#""outcome":"failed""#), "main returned Err");
}

#[test]
fn quota_stop_is_logged() {
    let limits = Limits {
        calls: 1,
        ..Limits::default()
    };
    let (log, stopped) = primer(limits, &[], None);
    let lines = log.lines();
    assert!(stopped);
    assert!(lines[2].contains(r#""result":"stopped:calls""#), "{lines:?}");
    assert!(lines[3].contains(r#""outcome":"error""#) && lines[3].contains(r#""calls":2"#));
}

#[test]
fn same_program_same_log() {
    let (a, _) = primer(Limits::default(), &[], None);
    let (b, _) = primer(Limits::default(), &[], None);
    assert_eq!(a.lines(), b.lines());
}

#[test]
fn call_missing_from_the_report_stops_the_run() {
    let (log, stopped) = primer(Limits::default(), &[], Some(("out", "create")));
    let lines = log.lines();
    assert!(stopped);
    assert!(
        lines[2].contains(r#""result":"stopped:unexpected","unexpected":true"#),
        "{lines:?}"
    );
    assert!(lines[3].contains(r#""outcome":"error""#));
}

#[test]
fn report_check_holds_without_a_log() {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("invoices.csv"), "a,1\n").unwrap();
    let dirs = vec![
        ("input".to_string(), kio_host::open_dir(a.path()).unwrap()),
        ("out".to_string(), kio_host::open_dir(b.path()).unwrap()),
    ];
    let mut kio = Interp::new(Limits::default()).unwrap();
    kio.hide_from_report("input", "read");
    let e = kio.run(&src, dirs).unwrap_err();
    let msg = kio.explain(&e).unwrap();
    assert!(
        msg.contains("`input.read` not in the authority report (a checker bug)"),
        "{msg}"
    );
}

/// D1: each new `Dir` method is logged with its path and bytes, and checked against the report.
#[test]
fn dir_methods_are_logged() {
    let src = "fn main(d: Dir) -> Result<Bool, IoErr> { \
        d.write(\"f\", \"abc\".to_bytes())?; let n = d.list(\"\")?; d.remove(\"f\")?; d.exists(\"f\") }";
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("sub")).unwrap();
    let run = |hide: Option<&str>| {
        let log = Log::default();
        let mut kio = Interp::new(Limits::default()).unwrap();
        kio.audit(log.clone(), vec![("d".into(), "/d".into())]);
        if let Some(m) = hide {
            kio.hide_from_report("d", m);
        }
        let d = kio_host::open_dir(t.path()).unwrap();
        let stopped = kio.run(src, vec![("d".to_string(), d)]).is_err();
        (log.lines(), stopped)
    };
    let (lines, stopped) = run(None);
    assert!(!stopped);
    let call = |seq, method, arg, bytes| {
        format!(
            r#"{{"event":"call","seq":{seq},"cap":"d","kind":"Dir","method":"{method}","arg":"{arg}","bytes":{bytes},"result":"ok"}}"#
        )
    };
    // `list` charges its names: "f" and "sub/".
    assert_eq!(lines[1], call(1, "write", "f", 3));
    assert_eq!(lines[2], call(2, "list", "", 5));
    assert_eq!(lines[3], call(3, "remove", "f", 0));
    assert_eq!(lines[4], call(4, "exists", "f", 0));
    for m in ["list", "write", "remove", "exists"] {
        let (lines, stopped) = run(Some(m));
        assert!(stopped, "{m}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains(&format!(r#""method":"{m}""#)) && l.contains(r#""unexpected":true"#)),
            "{m}: {lines:?}"
        );
    }
}

/// WS1b: `WsMessage.target` and `header` are logged with the header's name, never its value,
/// the same compiled and interpreted, and checked against the report.
#[test]
fn ws_upgrade_calls_are_logged_by_name() {
    let src = r#"fn main(m: WsMessage, w: WsSend) -> Result<(), WsErr> {
        let t = m.target();
        let a = m.header("Authorization").unwrap_or("");
        let c = m.header("cookie").unwrap_or("");
        w.text(t + a + c)
    }"#;
    let event = || kio_host::ws::WsEvent {
        kind: "text",
        conn: 1,
        data: vec![],
        upgrade: Arc::new(kio_host::ws::Upgrade {
            target: "/chat?room=1".into(),
            headers: vec![("authorization".into(), "Bearer s3cret".into())],
        }),
    };
    let names = ("m".to_string(), "w".to_string());
    let run = |compiled: bool, hide: Option<&str>| {
        let log = Log::default();
        let stopped = if compiled {
            let mut c = Kio::new(Limits::default()).unwrap();
            c.audit(log.clone(), vec![]);
            if let Some(m) = hide {
                c.hide_from_report("m", m);
            }
            !matches!(c.run_ws(src, vec![], event(), false, &names), Ok((Ok(_), _)))
        } else {
            let mut k = Interp::new(Limits::default()).unwrap();
            k.audit(log.clone(), vec![]);
            if let Some(m) = hide {
                k.hide_from_report("m", m);
            }
            !matches!(k.run_ws(src, vec![], event(), false, &names), Ok((Ok(_), _)))
        };
        (log.lines(), stopped)
    };
    let (lines, stopped) = run(false, None);
    assert!(!stopped);
    let call = |seq, method, arg| {
        format!(
            r#"{{"event":"call","seq":{seq},"cap":"m","kind":"WsMessage","method":"{method}","arg":"{arg}","bytes":0,"result":"ok"}}"#
        )
    };
    assert_eq!(lines[1], call(1, "target", "target"));
    assert_eq!(lines[2], call(2, "header", "Authorization"));
    assert_eq!(lines[3], call(3, "header", "cookie"));
    assert!(!lines.iter().any(|l| l.contains("s3cret")), "{lines:?}");
    let cut = |l: &String| l.split(r#","peak_memory":"#).next().unwrap().to_string();
    let compiled: Vec<_> = run(true, None).0.iter().map(cut).collect();
    assert_eq!(compiled, lines.iter().map(cut).collect::<Vec<_>>());
    for m in ["target", "header"] {
        for compiled in [false, true] {
            let (lines, stopped) = run(compiled, Some(m));
            assert!(stopped, "{m}");
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains(&format!(r#""method":"{m}""#)) && l.contains(r#""unexpected":true"#)),
                "{m}: {lines:?}"
            );
        }
    }
}

/// `--audit` checked against the other grants, through the `kio` binary.
mod cli {
    use std::path::Path;
    use std::process::Command;

    /// `kio run` with `args`, the cache under `cache`: exit code and stderr.
    fn kio(cache: &Path, args: &[&str]) -> (i32, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_kio"))
            .arg("run")
            .args(args)
            .env("XDG_CACHE_HOME", cache)
            .output()
            .unwrap();
        (out.status.code().unwrap(), String::from_utf8(out.stderr).unwrap())
    }

    fn arg(name: &str, path: &Path) -> String {
        format!("{name}={}", path.display())
    }

    const STORE: &str = r#"fn main(store: Kv) -> Result<(), KvErr> { store.set("a", "1".to_bytes()) }"#;

    /// Writing the log into a store would corrupt it: the same path, a path that is not yet a
    /// file, a hard link and the store's compaction file are all refused, the store untouched.
    #[test]
    fn an_audit_log_that_is_a_kv_store_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let prog = t.path().join("store.kio");
        std::fs::write(&prog, STORE).unwrap();
        let prog = prog.to_str().unwrap();
        let db = t.path().join("c.kv");
        assert_eq!(kio(t.path(), &[prog, "--kv", &arg("store", &db)]).0, 0);
        let before = std::fs::read(&db).unwrap();
        let link = t.path().join("hard.log");
        std::fs::hard_link(&db, &link).unwrap();
        let tmp = t.path().join("c.kv.tmp");
        let fresh = t.path().join("new.kv");
        let refused = |log: &Path, store: &Path, spec: &str| {
            let (code, err) = kio(
                t.path(),
                &[
                    prog,
                    "--kv",
                    &format!("store={}{spec}", store.display()),
                    "--audit",
                    log.to_str().unwrap(),
                ],
            );
            assert_eq!(code, 2, "{}: {err}", log.display());
            assert_eq!(
                err,
                format!("kio: `--audit {}` is the same file as `--kv store`\n  hint: give the audit log its own file; writing it would corrupt the store\n", log.display())
            );
        };
        refused(&db, &db, "");
        refused(&t.path().join(".").join("c.kv"), &db, "");
        refused(&link, &db, "");
        refused(&tmp, &db, "");
        refused(&fresh, &fresh, "");
        assert_eq!(std::fs::read(&db).unwrap(), before);
        assert!(!tmp.exists() && !fresh.exists());
    }

    /// A program granted the directory that holds its log could read or replace it.
    #[test]
    fn an_audit_log_inside_a_granted_dir_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let prog = t.path().join("dir.kio");
        std::fs::write(&prog, "fn main(d: Dir) -> Int { 1 }").unwrap();
        let prog = prog.to_str().unwrap();
        let data = t.path().join("data");
        std::fs::create_dir_all(data.join("sub")).unwrap();
        std::fs::write(data.join("old.log"), "").unwrap();
        let outside = t.path().join("link.log");
        std::os::unix::fs::symlink(data.join("old.log"), &outside).unwrap();
        let dir = arg("d", &data);
        for log in [
            data.join("a.log"),
            data.join("sub/a.log"),
            data.join("sub/../a.log"),
            outside,
        ] {
            let (code, err) = kio(t.path(), &[prog, "--dir", &dir, "--audit", log.to_str().unwrap()]);
            assert_eq!(code, 2, "{err}");
            assert_eq!(
                err,
                format!("kio: `--audit {}` is inside `--dir d`\n  hint: keep the audit log outside granted directories, where the program cannot read or replace it\n", log.display())
            );
        }
        assert!(!data.join("a.log").exists() && !data.join("sub/a.log").exists());
        // A dangling symlink is never followed into the directory.
        let dangling = t.path().join("dangling.log");
        std::os::unix::fs::symlink(data.join("new.log"), &dangling).unwrap();
        let (code, err) = kio(t.path(), &[prog, "--dir", &dir, "--audit", dangling.to_str().unwrap()]);
        assert_eq!(code, 2, "{err}");
        assert!(
            err.starts_with(&format!("kio: --audit {}: ", dangling.display())),
            "{err}"
        );
        assert!(!data.join("new.log").exists());
        let (code, err) = kio(
            t.path(),
            &[prog, "--dir", &dir, "--audit", t.path().join("a.log").to_str().unwrap()],
        );
        assert_eq!(code, 0, "{err}");
    }

    /// The log is opened to append: an existing file keeps what it held, and each run adds its
    /// own header and trailer.
    #[test]
    fn an_existing_file_is_appended_to() {
        let t = tempfile::tempdir().unwrap();
        let prog = t.path().join("one.kio");
        std::fs::write(&prog, "fn main() -> Int { 1 }").unwrap();
        let log = t.path().join("a.log");
        std::fs::write(&log, "keep\n").unwrap();
        for _ in 0..2 {
            let (code, err) = kio(t.path(), &[prog.to_str().unwrap(), "--audit", log.to_str().unwrap()]);
            assert_eq!(code, 0, "{err}");
        }
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with("keep\n{"), "{text}");
        assert_eq!(text.matches(r#""event":"start""#).count(), 2);
        assert_eq!(text.matches(r#""event":"end""#).count(), 2);
    }
}
