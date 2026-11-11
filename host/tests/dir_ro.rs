//! H27: `--dir name=path:ro` refuses `write`, `create` and `remove` in the host (`denied`), on
//! both paths; `kio run`, `--listen` and `kio check --grants` refuse a program that can write it.
use kio_host::{Interp, Kio, Limits, RunResult, Sources};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

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

/// `src` on `root` granted to `d`, `:ro`, compiled or interpreted; the value and the audit log.
fn run(src: &str, root: &Path, compiled: bool) -> (String, String) {
    let log = Buf::default();
    let dirs = vec![("d".to_string(), kio_host::open_dir(root).unwrap())];
    let paths = vec![("d".to_string(), "/in".to_string())];
    let sources = Sources {
        read_only_dirs: vec!["d".into()],
        ..Sources::default()
    };
    let r: RunResult = if compiled {
        let mut k = Kio::new(Limits::default()).unwrap();
        k.audit(log.clone(), paths);
        k.grant_sources(sources);
        k.run(src, dirs).unwrap()
    } else {
        let mut k = Interp::new(Limits::default()).unwrap();
        k.audit(log.clone(), paths);
        k.grant_sources(sources);
        k.run(src, dirs).unwrap()
    };
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    (r.unwrap().value, text)
}

#[test]
fn writes_are_denied_and_reads_work_on_both_paths() {
    for compiled in [false, true] {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a"), "old").unwrap();
        for (method, call) in [
            ("write", r#"d.write("a", "new".to_bytes())"#),
            ("write", r#"d.write("b", "new".to_bytes())"#),
            ("create", r#"d.create("b", "new".to_bytes())"#),
            ("remove", r#"d.remove("a")"#),
        ] {
            let src = format!("fn main(d: Dir) -> Result<(), IoErr> {{ {call} }}");
            let (value, log) = run(&src, t.path(), compiled);
            assert_eq!(value, "IoErr::Denied", "{call} (compiled: {compiled})");
            let line = format!(r#""method":"{method}","arg":"#);
            assert!(log.contains(&line) && log.contains(r#""result":"denied""#), "{log}");
            assert!(log.contains(r#""name":"d","kind":"Dir","target":"/in:ro""#), "{log}");
        }
        assert_eq!(std::fs::read_to_string(t.path().join("a")).unwrap(), "old");
        assert!(!t.path().join("b").exists());
        let src = r#"fn main(d: Dir) -> Result<Str, IoErr> {
          let names = d.list("")?;
          let e = d.exists("a")?;
          Ok(d.read("a")?.to_str() + " " + names.len().to_str() + " " + e.to_str())
        }"#;
        assert_eq!(run(src, t.path(), compiled).0, r#"Ok("old 1 true")"#);
    }
}

/// `(exit code, stdout, stderr)` of `kio args..`.
fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn a_program_that_can_write_is_refused_at_start() {
    let t = tempfile::tempdir().unwrap();
    let input = t.path().join("in");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("a"), "hi").unwrap();
    let ro = format!("input={}:ro", input.to_str().unwrap());
    for m in ["write", "create", "remove"] {
        let call = match m {
            "remove" => r#"input.remove("a")"#.to_string(),
            _ => format!(r#"input.{m}("b", "x".to_bytes())"#),
        };
        let prog = write(
            t.path(),
            "p.kio",
            &format!("fn main(input: Dir) -> Result<(), IoErr> {{ {call} }}"),
        );
        let refusal = format!(
            "kio: `input` is granted `:ro` but the program can write it (`input {m}`)\n  hint: drop `:ro`, or remove the write\n"
        );
        assert_eq!(kio(&["run", &prog, "--dir", &ro]), (2, String::new(), refusal.clone()));
        let g = write(t.path(), "p.grants", &format!("--dir {ro}\n"));
        assert_eq!(
            kio(&["check", &prog, "--grants", &g]),
            (2, String::new(), refusal.clone())
        );
        let app = write(
            t.path(),
            "app.kio",
            &format!(
                "fn main(req: Incoming, res: Outgoing, input: Dir) -> Result<(), WebErr> {{ let r = {call}; res.send(200, [], [], \"ok\".to_bytes()) }}"
            ),
        );
        let (code, _, err) = kio(&["run", &app, "--listen", "127.0.0.1:0", "--dir", &ro]);
        assert_eq!((code, err), (2, refusal));
    }
    assert!(!input.join("b").exists() && input.join("a").exists());
    // A program that only reads runs, and the audit header shows `:ro`.
    let prog = write(
        t.path(),
        "r.kio",
        r#"fn main(input: Dir) -> Str { match input.read("a") { Ok(b) => b.to_str(), Err(_) => "no" } }"#,
    );
    let audit = t.path().join("audit.log");
    let (code, out, err) = kio(&["run", &prog, "--dir", &ro, "--audit", audit.to_str().unwrap()]);
    assert_eq!((code, out.as_str()), (0, "hi\n"), "{err}");
    let log = std::fs::read_to_string(&audit).unwrap();
    let target = format!(
        r#""name":"input","kind":"Dir","target":"{}:ro""#,
        input.to_str().unwrap()
    );
    assert!(log.contains(&target), "{log}");
    let g = write(t.path(), "r.grants", &format!("--dir {ro}\n"));
    assert_eq!(kio(&["check", &prog, "--grants", &g]).0, 0);
}
