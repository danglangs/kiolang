use kio_host::listen::ListenLimits;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

struct Kio {
    child: Child,
    port: u16,
}

impl Drop for Kio {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start `kio run app.kio --listen 127.0.0.1:0` and read the port from its stderr.
fn start(public: &std::path::Path) -> Kio {
    start_app("../examples/serve/app.kio", public)
}

fn start_app(app: &str, public: &std::path::Path) -> Kio {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["run", app, "--dir"])
        .arg(format!("public={}", public.display()))
        .args(["--listen", "127.0.0.1:0"])
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
    // Keep draining stderr so the child never blocks on a full pipe.
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    Kio { child, port }
}

/// One request on a fresh connection; returns the status and body.
fn send(port: u16, method: &str, target: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!(
        "{method} {target} HTTP/1.1\r\nhost: x\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).unwrap();
    // The server may answer 413 and close before reading everything we send.
    let _ = s.write_all(body);
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("no response head") + 4;
    let status = std::str::from_utf8(&raw[..split]).unwrap().split(' ').nth(1).unwrap();
    (status.parse().unwrap(), raw[split..].to_vec())
}

#[test]
fn serves_a_route() {
    let public = tempfile::tempdir().unwrap();
    let kio = start(public.path());
    let (status, body) = send(kio.port, "GET", "/hello/kio", b"");
    assert_eq!((status, body.as_slice()), (200, &br#"{"hello": "kio"}"#[..]));
}

#[test]
fn hundred_concurrent_requests_succeed() {
    let public = tempfile::tempdir().unwrap();
    let kio = start(public.path());
    let port = kio.port;
    let threads: Vec<_> = (0..100)
        .map(|i| std::thread::spawn(move || send(port, "GET", &format!("/hello/n{i}"), b"")))
        .collect();
    // Under load a request may wait past QUEUE_WAIT and be told 503, which is correct; any 200
    // must carry its own body, and nearly all should succeed.
    let mut ok = 0;
    for (i, t) in threads.into_iter().enumerate() {
        match t.join().unwrap() {
            (200, body) => {
                assert_eq!(body, format!(r#"{{"hello": "n{i}"}}"#).into_bytes());
                ok += 1;
            }
            (status, _) => assert_eq!(status, 503),
        }
    }
    assert!(ok >= 90, "only {ok} of 100 succeeded");
}

#[test]
fn body_over_one_megabyte_is_413() {
    let public = tempfile::tempdir().unwrap();
    let kio = start(public.path());
    assert_eq!(send(kio.port, "POST", "/hello/kio", &vec![b'x'; (1 << 20) + 1]).0, 413);
    assert_eq!(send(kio.port, "GET", "/hello/kio", b"").0, 200);
}

/// examples/serve/app.kio passes the target with its leading slash to `Dir.read`, which refuses
/// it (a 500 for every file), so this uses a small app that strips the slash.
#[test]
fn serves_files_and_404() {
    let public = tempfile::tempdir().unwrap();
    std::fs::write(public.path().join("index.html"), "<h1>hi</h1>").unwrap();
    let kio = start(public.path());
    assert_eq!(
        send(kio.port, "GET", "/index.html", b""),
        (200, b"<h1>hi</h1>".to_vec())
    );
    assert_eq!(send(kio.port, "GET", "/missing", b"").0, 404);
    // A path escaping the granted dir is refused by the host and answered as missing.
    assert_eq!(send(kio.port, "GET", "/../../etc/passwd", b"").0, 404);
}

/// The host owns framing: a guest-set content-length cannot desync the connection.
#[test]
fn guest_framing_headers_are_dropped() {
    let public = tempfile::tempdir().unwrap();
    let app = public
        .path()
        .join("..")
        .join(format!("framing-{}.kio", std::process::id()));
    std::fs::write(
        &app,
        r#"fn main(req: Incoming, res: Outgoing, public: Dir) -> Result<(), WebErr> {
            res.send(200, ["content-length", "x-ok"], ["0", "1"], "hello".to_bytes())
        }"#,
    )
    .unwrap();
    let kio = start_app(app.to_str().unwrap(), public.path());
    let got = send(kio.port, "GET", "/", b"");
    std::fs::remove_file(&app).unwrap();
    assert_eq!(got, (200, b"hello".to_vec()));
}

/// W8: serve a one-file app in this process with small limits; returns the port.
mod limits {
    use super::*;
    use kio_host::listen::{serve_with, ListenLimits};
    use kio_host::{open_dir, Limits, Server};
    use std::time::Duration;

    /// `/pipe` reads a path the `test-hooks` build blocks on (until the 2 s deadline); any other path answers 200.
    const APP: &str = r#"fn main(req: Incoming, res: Outgoing, public: Dir) -> Result<(), WebErr> {
        if req.target() == "/pipe" { let r = public.read("pipe"); }
        res.send(200, [], [], "ok".to_bytes())
    }"#;

    pub fn start(public: &std::path::Path, limits: ListenLimits) -> u16 {
        let server = Server::new(
            vec![("app.kio".into(), APP.into())],
            Limits {
                timeout_secs: 2,
                ..Limits::web()
            },
        )
        .unwrap()
        .unwrap();
        let dirs = vec![("public".to_string(), open_dir(public).unwrap())];
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = serve_with(server, dirs, "127.0.0.1:0".parse().unwrap(), limits, |a| {
                tx.send(a.port()).unwrap()
            });
        });
        rx.recv().unwrap()
    }

    pub fn connect(port: u16) -> TcpStream {
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s
    }

    /// Everything the server sends before it closes the connection.
    pub fn read_all(mut s: TcpStream) -> String {
        let mut raw = Vec::new();
        // A read timeout is an error here: the server must have closed the connection.
        s.read_to_end(&mut raw)
            .or_else(|e| {
                if raw.is_empty() && e.kind() == std::io::ErrorKind::ConnectionReset {
                    Ok(0)
                } else {
                    Err(e)
                }
            })
            .unwrap();
        String::from_utf8_lossy(&raw).into_owned()
    }

    pub fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
}

#[test]
fn stalled_headers_get_no_response_and_close() {
    let public = tempfile::tempdir().unwrap();
    let port = limits::start(
        public.path(),
        ListenLimits {
            header_timeout: limits::ms(300),
            ..Default::default()
        },
    );
    let mut s = limits::connect(port);
    s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n").unwrap();
    assert_eq!(limits::read_all(s), "");
}

#[test]
fn slow_body_is_408() {
    let public = tempfile::tempdir().unwrap();
    let port = limits::start(
        public.path(),
        ListenLimits {
            body_deadline: limits::ms(300),
            ..Default::default()
        },
    );
    let mut s = limits::connect(port);
    s.write_all(b"POST / HTTP/1.1\r\nhost: x\r\ncontent-length: 10\r\n\r\nabc")
        .unwrap();
    assert!(limits::read_all(s).starts_with("HTTP/1.1 408"));
}

#[test]
fn idle_keep_alive_connection_is_closed() {
    let public = tempfile::tempdir().unwrap();
    let port = limits::start(
        public.path(),
        ListenLimits {
            idle: limits::ms(300),
            ..Default::default()
        },
    );
    let mut s = limits::connect(port);
    s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
    // One answer, then the server closes the idle connection (read_all returns at EOF).
    let got = limits::read_all(s);
    assert!(got.starts_with("HTTP/1.1 200") && got.ends_with("ok"));
}

#[test]
fn connections_past_the_cap_are_dropped() {
    let public = tempfile::tempdir().unwrap();
    let port = limits::start(
        public.path(),
        ListenLimits {
            connections: 1,
            ..Default::default()
        },
    );
    let _held = limits::connect(port);
    // The held connection takes the only slot; the next is closed without an answer.
    let mut s = limits::connect(port);
    let _ = s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n");
    assert_eq!(limits::read_all(s), "");
}

#[cfg(unix)]
#[test]
fn request_past_the_queue_cap_is_503_at_once() {
    let public = tempfile::tempdir().unwrap();
    let port = limits::start(
        public.path(),
        ListenLimits {
            workers: 1,
            queued: 1,
            ..Default::default()
        },
    );
    // One request holds the worker (blocked on `pipe`), one waits in the queue.
    let hold = std::thread::spawn(move || send(port, "GET", "/pipe", b""));
    std::thread::sleep(limits::ms(500));
    let wait = std::thread::spawn(move || send(port, "GET", "/pipe", b""));
    std::thread::sleep(limits::ms(300));
    let start = std::time::Instant::now();
    assert_eq!(send(port, "GET", "/", b"").0, 503);
    assert!(start.elapsed() < limits::ms(500));
    // The queued request gives up after the 1 s queue wait.
    assert_eq!(wait.join().unwrap().0, 503);
    assert_eq!(hold.join().unwrap().0, 500);
}

#[test]
fn connection_is_closed_at_the_lifetime_cap() {
    let public = tempfile::tempdir().unwrap();
    let port = limits::start(
        public.path(),
        ListenLimits {
            lifetime: limits::ms(500),
            ..Default::default()
        },
    );
    // Never read the answer: the server must still close the connection at the cap.
    let mut s = limits::connect(port);
    s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
    let start = std::time::Instant::now();
    std::thread::sleep(limits::ms(800));
    assert!(limits::read_all(s).starts_with("HTTP/1.1 200"));
    assert!(start.elapsed() < limits::ms(2000));
}

/// W11b: `--audit` and `--http` conflict with `--listen`: exit 2 and a hint, before any file exists.
#[test]
fn audit_and_http_are_refused_with_listen() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("audit.log");
    for (flag, value, hint) in [
        (
            "--audit",
            log.to_str().unwrap(),
            "per-request audit logging is not supported yet; drop --audit",
        ),
        (
            "--http",
            "x=http://127.0.0.1:1",
            "Http grants under --listen are not supported yet",
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_kio"))
            .args([
                "run",
                "../examples/serve/app.kio",
                flag,
                value,
                "--listen",
                "127.0.0.1:0",
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(&format!("hint: {hint}")),
            "{flag}"
        );
    }
    assert!(!log.exists());
}

/// H10: `Incoming` and `Outgoing` bind by type, so any parameter names serve.
#[test]
fn http_grants_bind_by_type() {
    let public = tempfile::tempdir().unwrap();
    let app = public
        .path()
        .join("..")
        .join(format!("bytype-{}.kio", std::process::id()));
    std::fs::write(
        &app,
        r#"fn main(request: Incoming, response: Outgoing, public: Dir) -> Result<(), WebErr> {
            response.send(200, [], [], request.target().to_bytes())
        }"#,
    )
    .unwrap();
    let kio = start_app(app.to_str().unwrap(), public.path());
    let got = send(kio.port, "GET", "/x", b"");
    std::fs::remove_file(&app).unwrap();
    assert_eq!(got, (200, b"/x".to_vec()));
}

/// H10: zero or two of either HTTP grant is refused at startup with a hint.
#[test]
fn wrong_http_grant_counts_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    for (i, (params, body)) in [
        (
            "a: Incoming, b: Incoming, res: Outgoing",
            "res.send(200, [], [], \"\".to_bytes())",
        ),
        ("req: Incoming", "Ok(())"),
        (
            "res: Outgoing, a: Incoming, b: Outgoing",
            "res.send(200, [], [], \"\".to_bytes())",
        ),
    ]
    .iter()
    .enumerate()
    {
        let app = dir.path().join(format!("bad{i}.kio"));
        std::fs::write(&app, format!("fn main({params}) -> Result<(), WebErr> {{ {body} }}")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_kio"))
            .args(["run", app.to_str().unwrap(), "--listen", "127.0.0.1:0"])
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{params}: {err}");
        assert!(err.contains("takes one Incoming and one Outgoing"), "{params}: {err}");
        assert!(
            err.contains("hint: write `fn main(request: Incoming, response: Outgoing, ..)`"),
            "{err}"
        );
    }
}
