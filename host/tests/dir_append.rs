//! H34: `Dir.append` creates or appends with one `O_APPEND` write per call, on both paths; `:ro`
//! refuses it in the host and at startup; `--io` bounds it; concurrent appends under `--listen`
//! never interleave within a call.
use kio_host::{Interp, Kio, Limits, RunResult, Sources};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
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

/// `src` on `root` granted to `d` (`:ro` if `ro`), compiled or interpreted, under `limits`: the
/// value (or `error: ..`) and the audit log.
fn run(src: &str, root: &Path, ro: bool, compiled: bool, limits: Limits) -> (String, String) {
    let log = Buf::default();
    let dirs = vec![("d".to_string(), kio_host::open_dir(root).unwrap())];
    let paths = vec![("d".to_string(), "/in".to_string())];
    let sources = Sources {
        read_only_dirs: if ro { vec!["d".into()] } else { vec![] },
        ..Sources::default()
    };
    // A limit stops the run as the outer error.
    let r: Result<RunResult, _> = if compiled {
        let mut k = Kio::new(limits).unwrap();
        k.audit(log.clone(), paths);
        k.grant_sources(sources);
        k.run(src, dirs)
    } else {
        let mut k = Interp::new(limits).unwrap();
        k.audit(log.clone(), paths);
        k.grant_sources(sources);
        k.run(src, dirs)
    };
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let value = match r {
        Ok(Ok(o)) => o.value,
        Ok(Err(d)) => format!("error: {}", d[0].message),
        Err(e) => format!("error: {e:#}"),
    };
    (value, text)
}

#[test]
fn create_then_append_on_both_paths() {
    for compiled in [false, true] {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("old"), "first\n").unwrap();
        std::fs::create_dir(t.path().join("sub")).unwrap();
        std::os::unix::fs::symlink("old", t.path().join("link")).unwrap();
        let src = r#"fn main(d: Dir) -> Result<Str, IoErr> {
          d.append("new", "a\n".to_bytes())?;
          d.append("new", "".to_bytes())?;
          d.append("new", "b\n".to_bytes())?;
          d.append("old", "second\n".to_bytes())?;
          let errs = [d.append("sub/", "x".to_bytes()), d.append("sub", "x".to_bytes()), d.append("link", "x".to_bytes()),
            d.append("../out", "x".to_bytes()), d.append("", "x".to_bytes()), d.append("none/x", "x".to_bytes())];
          let shown = errs.map(|e| match e {
            Ok(_) => "ok", Err(IoErr::Denied) => "Denied", Err(IoErr::InvalidPath) => "InvalidPath",
            Err(IoErr::NotFound) => "NotFound", Err(_) => "other",
          });
          Ok(d.read("new")?.to_str() + d.read("old")?.to_str() + shown.join(","))
        }"#;
        let (value, log) = run(src, t.path(), false, compiled, Limits::default());
        assert_eq!(
            value, r#"Ok("a\nb\nfirst\nsecond\nDenied,Denied,Denied,InvalidPath,InvalidPath,NotFound")"#,
            "compiled: {compiled}"
        );
        assert_eq!(std::fs::read_to_string(t.path().join("new")).unwrap(), "a\nb\n");
        assert_eq!(
            std::fs::read_to_string(t.path().join("old")).unwrap(),
            "first\nsecond\n"
        );
        assert!(!t.path().join("out").exists() && !t.path().parent().unwrap().join("out").exists());
        assert!(
            log.contains(r#""method":"append","arg":"new","bytes":2,"result":"ok""#),
            "{log}"
        );
        assert!(
            log.contains(r#""method":"append","arg":"link","bytes":1,"result":"denied""#),
            "{log}"
        );
    }
}

#[test]
fn a_read_only_dir_denies_append_on_both_paths() {
    for compiled in [false, true] {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a"), "old").unwrap();
        for path in ["a", "b"] {
            let src = format!(r#"fn main(d: Dir) -> Result<(), IoErr> {{ d.append("{path}", "new".to_bytes()) }}"#);
            let (value, log) = run(&src, t.path(), true, compiled, Limits::default());
            assert_eq!(value, "IoErr::Denied", "{path} (compiled: {compiled})");
            assert!(
                log.contains(&format!(
                    r#""method":"append","arg":"{path}","bytes":0,"result":"denied""#
                )),
                "{log}"
            );
        }
        assert_eq!(std::fs::read_to_string(t.path().join("a")).unwrap(), "old");
        assert!(!t.path().join("b").exists());
    }
}

/// The data is charged to `--io` before anything is written, as `write`'s is.
#[test]
fn the_io_limit_bounds_an_append() {
    for compiled in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let src = r#"fn main(d: Dir) -> Result<(), IoErr> { d.append("log", "0123456789".to_bytes()) }"#;
        let limits = Limits {
            io: 5,
            ..Limits::default()
        };
        let (value, log) = run(src, t.path(), false, compiled, limits);
        assert!(value.starts_with("error: too much I/O"), "{value}");
        assert!(!t.path().join("log").exists());
        assert!(
            log.contains(r#""method":"append","arg":"log","bytes":10,"result":"stopped:io""#),
            "{log}"
        );
        let limits = Limits {
            io: 10,
            ..Limits::default()
        };
        assert_eq!(run(src, t.path(), false, compiled, limits).0, "Ok(())");
        assert_eq!(std::fs::read_to_string(t.path().join("log")).unwrap(), "0123456789");
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
fn a_program_that_appends_is_refused_a_read_only_dir_at_start() {
    let t = tempfile::tempdir().unwrap();
    let input = t.path().join("in");
    std::fs::create_dir(&input).unwrap();
    let ro = format!("input={}:ro", input.to_str().unwrap());
    let prog = write(
        t.path(),
        "p.kio",
        r#"fn main(input: Dir) -> Result<(), IoErr> { input.append("log", "x".to_bytes()) }"#,
    );
    let refusal = "kio: `input` is granted `:ro` but the program can write it (`input append`)\n  hint: drop `:ro`, or remove the write\n";
    assert_eq!(kio(&["run", &prog, "--dir", &ro]), (2, String::new(), refusal.into()));
    let g = write(t.path(), "p.grants", &format!("--dir {ro}\n"));
    assert_eq!(
        kio(&["check", &prog, "--grants", &g]),
        (2, String::new(), refusal.into())
    );
    let app = write(
        t.path(),
        "app.kio",
        r#"fn main(req: Incoming, res: Outgoing, input: Dir) -> Result<(), WebErr> { let r = input.append("log", req.body()); res.send(200, [], [], "ok".to_bytes()) }"#,
    );
    let (code, _, err) = kio(&["run", &app, "--listen", "127.0.0.1:0", "--dir", &ro]);
    assert_eq!((code, err.as_str()), (2, refusal));
    assert!(!input.join("log").exists());
    // The report names the method; gaining `append` where `write` was is covered.
    assert_eq!(kio(&["check", &prog]).1.trim(), "main : input append");
    let base = write(
        t.path(),
        "base.kio",
        r#"fn main(input: Dir) -> Result<(), IoErr> { input.write("log", "x".to_bytes()) }"#,
    );
    let (code, out, _) = kio(&["check", &prog, "--base", &base]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("append is covered by write"), "{out}");
}

/// Each request appends its body to one file: every line arrives whole.
const APPENDER: &str = r#"fn main(req: Incoming, res: Outgoing, d: Dir) -> Result<(), WebErr> {
  match d.append("log", req.body()) {
    Ok(_) => res.send(200, [], [], "ok".to_bytes()),
    Err(_) => res.send(500, [], [], "append failed".to_bytes()),
  }
}"#;

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(app: &str, extra: &[&str]) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["run", app, "--listen", "127.0.0.1:0"])
        .args(extra)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("listening on http://") {
        line.clear();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "kio exited before listening");
    }
    let port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
    });
    Server { child, port }
}

fn post(port: u16, body: &[u8]) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "POST /log HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    s.write_all(body).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    raw
}

#[test]
fn concurrent_appends_under_listen_are_each_intact() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let app = write(t.path(), "app.kio", APPENDER);
    let audit = t.path().join("audit.log");
    let server = start(
        &app,
        &[
            "--dir",
            &format!("d={}", out.display()),
            "--audit",
            audit.to_str().unwrap(),
        ],
    );
    const N: usize = 20;
    const LEN: usize = 100_000;
    let port = server.port;
    let threads: Vec<_> = (0..N)
        .map(|i| {
            std::thread::spawn(move || {
                let mut body = vec![b'a' + i as u8; LEN];
                body.push(b'\n');
                post(port, &body)
            })
        })
        .collect();
    let responses: Vec<String> = threads.into_iter().map(|h| h.join().unwrap()).collect();
    drop(server);
    let log = std::fs::read_to_string(&audit).unwrap();
    for r in &responses {
        assert!(r.starts_with("HTTP/1.1 200"), "{r}\n{log}");
    }
    assert_eq!(
        log.matches(r#""method":"append","arg":"log","bytes":100001,"result":"ok""#)
            .count(),
        N,
        "{log}"
    );
    let log = std::fs::read(out.join("log")).unwrap();
    assert_eq!(log.len(), N * (LEN + 1));
    let mut seen = vec![false; N];
    for line in log.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        assert_eq!(line.len(), LEN);
        let c = line[0];
        assert!(line.iter().all(|&b| b == c), "a line mixes requests");
        seen[(c - b'a') as usize] = true;
    }
    assert!(seen.iter().all(|&s| s), "{seen:?}");
}
