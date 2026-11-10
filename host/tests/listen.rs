use kio_host::listen::ListenLimits;

mod upstream;
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
    start_with(app, public, &[])
}

fn start_with(app: &str, public: &std::path::Path, extra: &[&str]) -> Kio {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["run", app, "--dir"])
        .arg(format!("public={}", public.display()))
        .args(["--listen", "127.0.0.1:0"])
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
    // Keep draining stderr so the child never blocks on a full pipe.
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    Kio { child, port }
}

/// One request on a fresh connection; returns the status and body.
fn send(port: u16, method: &str, target: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!(
        "{method} {target} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
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

/// Requests on one keep-alive connection, in turn; returns each status.
fn send_keep_alive(port: u16, paths: &[String]) -> Vec<u16> {
    send_keep_alive_timed(port, paths).into_iter().map(|(s, _)| s).collect()
}

/// `send_keep_alive` with how long each request took to be answered.
fn send_keep_alive_timed(port: u16, paths: &[String]) -> Vec<(u16, std::time::Duration)> {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut w = s;
    let mut statuses = Vec::new();
    for path in paths {
        let start = std::time::Instant::now();
        write!(w, "GET {path} HTTP/1.1\r\nhost: localhost\r\n\r\n").unwrap();
        let (mut line, mut len) = (String::new(), 0);
        r.read_line(&mut line).unwrap();
        statuses.push((line.split(' ').nth(1).unwrap().parse().unwrap(), start.elapsed()));
        loop {
            line.clear();
            r.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
        }
        r.read_exact(&mut vec![0; len]).unwrap();
    }
    statuses
}

/// The pool holds an instance for every worker: 64 keep-alive clients at once are never told 503
/// (a component instance takes three core instances; a pool sized by hand ran out at 22).
#[test]
fn sixty_four_keep_alive_clients_get_no_503() {
    let public = tempfile::tempdir().unwrap();
    let kio = start(public.path());
    let port = kio.port;
    let threads: Vec<_> = (0..64)
        .map(|i| {
            let paths: Vec<String> = (0..50).map(|j| format!("/hello/c{i}r{j}")).collect();
            std::thread::spawn(move || send_keep_alive(port, &paths))
        })
        .collect();
    for t in threads {
        assert!(t.join().unwrap().iter().all(|&s| s == 200));
    }
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

/// Compiled code runs on the server's worker threads' stacks: recursion to the largest `--stack`
/// is a 500 for that request, never a crash of the server.
#[test]
fn deep_recursion_at_the_largest_stack_is_a_500() {
    let public = tempfile::tempdir().unwrap();
    let app = public
        .path()
        .join("..")
        .join(format!("deep-{}.kio", std::process::id()));
    std::fs::write(
        &app,
        r#"fn down(n: Int) -> Int { if n == 0 { 0 } else { 1 + down(n - 1) } }
        fn main(req: Incoming, res: Outgoing, public: Dir) -> Result<(), WebErr> {
            let n = if req.target() == "/deep" { 100000000 } else { 1 };
            res.send(200, [], [], down(n).to_str().to_bytes())
        }"#,
    )
    .unwrap();
    let kio = start_with(
        app.to_str().unwrap(),
        public.path(),
        &["--stack", "8192", "--fuel", "1000000000"],
    );
    let deep = send(kio.port, "GET", "/deep", b"");
    let after = send(kio.port, "GET", "/", b"");
    std::fs::remove_file(&app).unwrap();
    assert_eq!(deep.0, 500);
    assert_eq!(after, (200, b"1".to_vec()));
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
    s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n").unwrap();
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
    s.write_all(b"POST / HTTP/1.1\r\nhost: localhost\r\ncontent-length: 10\r\n\r\nabc")
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
    s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n").unwrap();
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
    let _ = s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n");
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

/// Every worker can hold an instance at once: all of them blocked on `pipe` until the deadline
/// get 500, never 503 for want of an instance.
#[cfg(unix)]
#[test]
fn every_worker_gets_an_instance() {
    let public = tempfile::tempdir().unwrap();
    let workers = ListenLimits::default().workers;
    let port = limits::start(
        public.path(),
        ListenLimits {
            workers,
            ..Default::default()
        },
    );
    let threads: Vec<_> = (0..workers)
        .map(|_| std::thread::spawn(move || send(port, "GET", "/pipe", b"").0))
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), 500);
    }
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
    s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n").unwrap();
    let start = std::time::Instant::now();
    std::thread::sleep(limits::ms(800));
    assert!(limits::read_all(s).starts_with("HTTP/1.1 200"));
    assert!(start.elapsed() < limits::ms(2000));
}

/// H19: a handler that calls an upstream API: `/batch..` sends 8 GETs at once and answers with
/// their bodies in order; any other target is POSTed upstream with the request's body.
mod http {
    use super::*;
    use std::time::{Duration, Instant};

    const APP: &str = r#"fn main(req: Incoming, res: Outgoing, public: Dir, up: Http) -> Result<(), WebErr> {
        let t = req.target();
        if t.starts_with("/batch") {
            let mut rs: List<HttpRequest> = [];
            for i in 0..8 {
                rs.push(HttpRequest { method: "GET", path: t + "/" + i.to_str(), headers: [], body: "".to_bytes(), timeout_ms: 0 });
            }
            let mut out = "";
            for r in up.send_all(rs) {
                out = out + match r { Ok(r) => r.body.to_str(), Err(_) => "!" } + ";";
            }
            return res.send(200, [], [], out.to_bytes());
        }
        match up.send("POST", t, ["x-key: k"], req.body()) {
            Ok(r) => res.send(r.status, [], [], r.body),
            Err(_) => res.send(502, [], [], "upstream failed".to_bytes()),
        }
    }"#;

    /// Serve `APP` granted `up` with POST, and `extra` flags; returns the server and its temp dir.
    pub fn start(up: &crate::upstream::Upstream, extra: &[&str]) -> (Kio, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app.kio");
        std::fs::write(&app, APP).unwrap();
        let public = dir.path().join("public");
        std::fs::create_dir(&public).unwrap();
        let grant = format!("up={}", up.grant);
        let mut flags = vec!["--http", grant.as_str(), "--http-methods", "up=POST"];
        flags.extend_from_slice(extra);
        (start_with(app.to_str().unwrap(), &public, &flags), dir)
    }

    pub fn timed(port: u16, target: &str) -> (u16, Vec<u8>, Duration) {
        let start = Instant::now();
        let (status, body) = send(port, "GET", target, b"");
        (status, body, start.elapsed())
    }
}

/// H19: each request calls the granted API with its own body; the audit log has the call on the
/// request's line, with H16's fields, and the grant in the header.
#[test]
fn a_handler_calls_an_api_per_request() {
    let up = upstream::start();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("audit.log");
    let (kio, _app) = http::start(&up, &["--audit", log.to_str().unwrap()]);
    for i in 0..3 {
        let body = format!("hi {i}");
        let (status, got) = send(kio.port, "POST", &format!("/api/{i}"), body.as_bytes());
        assert_eq!(
            (status, String::from_utf8(got).unwrap()),
            (200, format!("POST /api/{i} hi {i}"))
        );
    }
    assert_eq!(up.seen.lock().unwrap().len(), 3);
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let up_grant = (lines[0]["grants"].as_array().unwrap().iter())
        .find(|g| g["name"] == "up")
        .unwrap();
    assert_eq!(up_grant["kind"], "Http");
    assert_eq!(up_grant["target"], format!("{} methods=GET,POST", up.grant));
    // #164: 1,000 calls per request under `--listen`; `--calls` overrides (below).
    assert_eq!(lines[0]["limits"]["calls"], 1000);
    for req in 1..=3u64 {
        let call = (lines.iter())
            .find(|l| l["req"] == req && l["kind"] == "Http")
            .unwrap_or_else(|| panic!("no Http call for req {req}: {text}"));
        let i = req - 1;
        assert_eq!(call["method"], "send");
        assert_eq!(call["http"], "POST");
        assert_eq!(call["arg"], format!("/api/{i}"));
        assert_eq!(call["headers"], serde_json::json!(["x-key"]));
        assert_eq!((call["sent"].as_u64(), call["status"].as_u64()), (Some(4), Some(200)));
        assert_eq!(
            call["received"].as_u64(),
            Some(format!("POST /api/{i} hi {i}").len() as u64)
        );
        assert_eq!(call["result"], "ok");
    }
}

/// #164: `--calls` overrides the 1,000 default under `--listen`; the audit header shows it.
#[test]
fn calls_flag_overrides_the_listen_default() {
    let up = upstream::start();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("audit.log");
    let (kio, _app) = http::start(&up, &["--audit", log.to_str().unwrap(), "--calls", "7"]);
    assert_eq!(send(kio.port, "POST", "/api", b"x").0, 200);
    let text = std::fs::read_to_string(&log).unwrap();
    let head: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(head["limits"]["calls"], 7);
}

/// #164 (H19 security review): 16 requests each fanning 64 requests out to an upstream that
/// never answers held the whole HTTP pool (1,024), and every other request's `get` was `timeout`
/// at once. With a full batch per worker in the pool, another request's `get` answers while they
/// hang. Served in-process at 32 workers (16 cores' worth) whatever this machine has.
#[test]
fn sixteen_fan_outs_do_not_starve_another_requests_get() {
    use kio_host::listen::{serve_with, ListenLimits};
    use kio_host::{open_dir, Limits, Origin, Server};
    const APP: &str = r#"fn main(req: Incoming, res: Outgoing, public: Dir, up: Http) -> Result<(), WebErr> {
        if req.target() == "/fan" {
            let mut rs: List<HttpRequest> = [];
            for i in 0..64 {
                rs.push(HttpRequest { method: "GET", path: "/hang", headers: [], body: "".to_bytes(), timeout_ms: 0 });
            }
            let out = up.send_all(rs).map(|r| match r { Ok(_) => "ok", Err(_) => "!" }).join(";");
            return res.send(200, [], [], out.to_bytes());
        }
        match up.get("/ok") {
            Ok(b) => res.send(200, [], [], b),
            Err(_) => res.send(502, [], [], "upstream failed".to_bytes()),
        }
    }"#;
    let up = upstream::start();
    let public = tempfile::tempdir().unwrap();
    let mut server = Server::new(
        vec![("app.kio".into(), APP.into())],
        Limits {
            timeout_secs: 2,
            ..Limits::web()
        },
    )
    .unwrap()
    .unwrap();
    server.grant_https(vec![("up".into(), Origin::parse(&up.grant).unwrap())]);
    let dirs = vec![("public".to_string(), open_dir(public.path()).unwrap())];
    let limits = ListenLimits {
        workers: 32,
        ..Default::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = serve_with(server, dirs, "127.0.0.1:0".parse().unwrap(), limits, |a| {
            tx.send(a.port()).unwrap()
        });
    });
    let port = rx.recv().unwrap();
    let fans: Vec<_> = (0..16)
        .map(|_| std::thread::spawn(move || send(port, "GET", "/fan", b"")))
        .collect();
    let start = std::time::Instant::now();
    while up.seen.lock().unwrap().len() < 16 * 64 {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "the fan-outs never all started"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let (status, body, took) = http::timed(port, "/one");
    assert_eq!((status, body), (200, b"GET /ok ".to_vec()));
    assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    for f in fans {
        let (status, _) = f.join().unwrap();
        assert!(matches!(status, 200 | 500), "{status}");
    }
}

/// H19: 50 requests at once, each with a `send_all` of 8, all answered in order.
#[test]
fn fifty_concurrent_requests_each_send_eight() {
    let up = upstream::start();
    let (kio, _app) = http::start(&up, &[]);
    let port = kio.port;
    let threads: Vec<_> = (0..50)
        .map(|i| std::thread::spawn(move || send(port, "GET", &format!("/batch{i}"), b"")))
        .collect();
    let mut ok = 0;
    for (i, t) in threads.into_iter().enumerate() {
        match t.join().unwrap() {
            (200, body) => {
                let want: String = (0..8).map(|j| format!("GET /batch{i}/{j} ;")).collect();
                assert_eq!(String::from_utf8(body).unwrap(), want);
                ok += 1;
            }
            // Under load a request may wait past the queue wait and be told 503.
            (status, _) => assert_eq!(status, 503),
        }
    }
    assert!(ok >= 45, "only {ok} of 50 succeeded");
    assert_eq!(up.seen.lock().unwrap().len(), ok * 8);
}

/// H19: a request whose upstream never answers holds one worker until its deadline; others are
/// answered meanwhile.
#[test]
fn a_hanging_upstream_does_not_starve_other_requests() {
    let up = upstream::start();
    let (kio, _app) = http::start(&up, &["--timeout", "2"]);
    let port = kio.port;
    let hung = std::thread::spawn(move || http::timed(port, "/hang"));
    std::thread::sleep(std::time::Duration::from_millis(200));
    for i in 0..20 {
        let (status, body, took) = http::timed(port, &format!("/ok{i}"));
        assert_eq!((status, body), (200, format!("POST /ok{i} ").into_bytes()));
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    }
    let (status, _, took) = hung.join().unwrap();
    assert!(matches!(status, 500 | 502), "{status}");
    assert!(took >= std::time::Duration::from_millis(1500), "{took:?}");
}

/// H12: `--audit` with `--listen` writes one log: the header once, then each request's calls and
/// trailer, every line a whole JSON object carrying its request's number.
#[test]
fn audit_log_of_concurrent_requests() {
    let public = tempfile::tempdir().unwrap();
    std::fs::write(public.path().join("a.txt"), "hello").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("audit.log");
    let kio = start_with(
        "../examples/serve/app.kio",
        public.path(),
        &["--audit", log.to_str().unwrap()],
    );
    let port = kio.port;
    let threads: Vec<_> = (0..3)
        .map(|_| std::thread::spawn(move || send(port, "GET", "/a.txt", b"")))
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), (200, b"hello".to_vec()));
    }
    // Each trailer is written before its response is sent.
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect();
    let head = &lines[0];
    assert_eq!(head["event"], "start");
    assert!(head.get("req").is_none());
    let grants: Vec<(&str, &str)> = (head["grants"].as_array().unwrap().iter())
        .map(|g| (g["name"].as_str().unwrap(), g["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(grants, [("public", "Dir"), ("req", "Incoming"), ("res", "Outgoing")]);
    assert_eq!(head["grants"][0]["target"], public.path().to_str().unwrap());
    assert_eq!(lines.iter().filter(|l| l["event"] == "start").count(), 1);
    let mut ends = Vec::new();
    for req in 1..=3u64 {
        let mine: Vec<_> = lines.iter().filter(|l| l["req"] == req).collect();
        let (end, calls) = mine.split_last().unwrap();
        assert_eq!(end["event"], "end");
        assert_eq!(end["outcome"], "ok");
        assert!(end["fuel_used"].as_u64().unwrap() > 0);
        assert_eq!(end["calls"].as_u64().unwrap() as usize, calls.len());
        assert!(end["io"].as_u64().unwrap() >= 5);
        for (i, c) in calls.iter().enumerate() {
            assert_eq!(
                (c["event"].as_str(), c["seq"].as_u64()),
                (Some("call"), Some(i as u64 + 1))
            );
        }
        assert!(calls.iter().any(|c| c["method"] == "read" && c["arg"] == "a.txt"));
        ends.push(*end);
    }
    assert_eq!(
        ends.len() + 1 + lines.iter().filter(|l| l["event"] == "call").count(),
        lines.len()
    );
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

/// The status and body of `GET /` with the given `Host` header lines.
fn get_with_host(port: u16, host_lines: &str) -> (u16, String) {
    get_at("127.0.0.1".parse().unwrap(), port, host_lines)
}

fn get_at(ip: std::net::IpAddr, port: u16, host_lines: &str) -> (u16, String) {
    let mut s = TcpStream::connect((ip, port)).unwrap();
    s.write_all(format!("GET /hello/kio HTTP/1.1\r\n{host_lines}connection: close\r\n\r\n").as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let status = raw.split(' ').nth(1).unwrap().parse().unwrap();
    (status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

/// H11: on a loopback bind only local names, and names given with `--host`, are served.
#[test]
fn host_header_is_checked_on_a_loopback_bind() {
    let public = tempfile::tempdir().unwrap();
    let kio = start(public.path());
    for host in [
        "localhost",
        "LOCALHOST",
        "localhost:8080",
        "127.0.0.1",
        "127.0.0.1:9",
        "[::1]",
        "[::1]:80",
    ] {
        assert_eq!(get_with_host(kio.port, &format!("host: {host}\r\n")).0, 200, "{host}");
    }
    for host_lines in [
        "host: evil.example\r\n",
        "host: evil.example:8080\r\n",
        "host: localhost.evil.example\r\n",
        "host: localhost\r\nhost: localhost\r\n",
        "",
    ] {
        let (status, body) = get_with_host(kio.port, host_lines);
        // hyper itself may answer 400 to a missing Host; otherwise the check answers 421.
        if host_lines.is_empty() {
            assert!(status == 400 || status == 421, "{status}");
        } else {
            assert_eq!(status, 421, "{host_lines}");
            assert!(body.contains("`--host <name>`"), "{body}");
        }
    }
    drop(kio);
    let kio = start_with("../examples/serve/app.kio", public.path(), &["--host", "App.Example"]);
    assert_eq!(get_with_host(kio.port, "host: app.example:81\r\n").0, 200);
    assert_eq!(get_with_host(kio.port, "host: APP.EXAMPLE\r\n").0, 200);
    assert_eq!(get_with_host(kio.port, "host: localhost\r\n").0, 200);
    assert_eq!(get_with_host(kio.port, "host: evil.example\r\n").0, 421);
}

/// H11: a non-loopback bind serves any `Host` unless `--host` is given.
#[test]
fn host_header_is_unchecked_on_a_non_loopback_bind() {
    use kio_host::{Limits, Server};
    let ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").and_then(|_| s.local_addr()))
        .map(|a| a.ip());
    // No routable address (offline sandbox): fall back to the wildcard, also non-loopback.
    let ip = ip
        .ok()
        .filter(|ip| !ip.is_loopback())
        .unwrap_or("0.0.0.0".parse().unwrap());
    // Connect to the bound address; the wildcard is reached through loopback.
    let ip = if ip.is_unspecified() {
        "127.0.0.1".parse().unwrap()
    } else {
        ip
    };
    let bind = if ip.is_loopback() {
        "0.0.0.0".parse().unwrap()
    } else {
        ip
    };
    let run = |hosts: Vec<String>| {
        let server = Server::new(
            vec![("app.kio".into(), r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> { res.send(200, [], [], "ok".to_bytes()) }"#.into())],
            Limits::web(),
        )
        .unwrap()
        .unwrap();
        let limits = ListenLimits {
            hosts,
            ..Default::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ =
                kio_host::listen::serve_with(server, vec![], (bind, 0).into(), limits, |a| tx.send(a.port()).unwrap());
        });
        rx.recv().unwrap()
    };
    let port = run(vec![]);
    assert_eq!(get_at(ip, port, "host: evil.example\r\n").0, 200);
    let port = run(vec!["app.example".into()]);
    assert_eq!(get_at(ip, port, "host: evil.example\r\n").0, 421);
    assert_eq!(get_at(ip, port, "host: app.example\r\n").0, 200);
}

/// H11: `--host` without `--listen` is a usage error with a hint.
#[test]
fn host_without_listen_is_refused() {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["run", "../examples/serve/app.kio", "--host", "app.example"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--host without --listen\n  hint: "), "{err}");
}

/// `kio run` on `src` with `args`; the exit code and stderr.
fn run_src(src: &str, args: &[&str]) -> (Option<i32>, String) {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.kio");
    std::fs::write(&app, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .arg("run")
        .arg(&app)
        .args(args)
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into())
}

/// H20: a program that fails to parse says so under `--listen`, not that `main` has the wrong shape.
#[test]
fn listen_reports_a_parse_error_first() {
    let src = "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {\n    res.send(200, [], [], \"x\".to_bytes()\n}\n";
    let (code, err) = run_src(src, &["--listen", "127.0.0.1:0"]);
    assert_eq!(code, Some(1));
    assert!(
        err.contains("app.kio:3: error: expected `)`\n  hint: a `(` is not closed"),
        "{err}"
    );
    assert!(!err.contains("--listen needs"), "{err}");
}

/// H18: `send` hands the response over at once and `main` runs on. `/slow` sends, then loops
/// until the 2 s deadline; `/count` sends, then adds 1 to `hits`; `/trap` sends, then traps;
/// `/none` never sends.
mod after_send {
    use super::*;
    use kio_host::listen::{serve_with, ListenLimits};
    use kio_host::{kv, open_dir, Limits, Server};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const APP: &str = r#"fn main(req: Incoming, res: Outgoing, public: Dir, store: Kv) -> Result<(), WebErr> {
        let t = req.target();
        if t == "/none" { return Ok(()); }
        if t == "/hits" {
            let n = match store.get("hits") { Ok(Some(b)) => b.to_str(), _ => "0" };
            return res.send(200, [], [], n.to_bytes());
        }
        res.send(200, [], [], "ok".to_bytes())?;
        if t == "/slow" { while true {} }
        if t == "/trap" { let xs: List<Int> = []; let x = xs[t.len()]; }
        if t == "/count" { let _n = store.add("hits", 1); }
        Ok(())
    }"#;

    /// Serve `APP` with `--audit`, a 2 s deadline and fuel enough to reach it; returns the server
    /// and the log's path.
    pub fn start(dir: &std::path::Path) -> (Kio, std::path::PathBuf) {
        let app = dir.join("app.kio");
        std::fs::write(&app, APP).unwrap();
        let log = dir.join("audit.log");
        // The log may not lie inside a granted Dir.
        let public = dir.join("public");
        std::fs::create_dir(&public).unwrap();
        let kio = start_with(
            app.to_str().unwrap(),
            &public,
            &[
                "--kv",
                "store=:memory:",
                "--timeout",
                "2",
                "--fuel",
                "1000000000000",
                "--audit",
                log.to_str().unwrap(),
            ],
        );
        (kio, log)
    }

    /// The trailer of request `req`, if it is in the log yet.
    pub fn end(log: &std::path::Path, req: u64) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(log).unwrap();
        text.lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|l| l["event"] == "end" && l["req"] == req)
    }

    /// `f()` once it is `Some`, polling for up to 10 s.
    pub fn eventually<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait until `/hits` reads `want`.
    pub fn hits_reach(port: u16, want: &str) {
        eventually(|| (send(port, "GET", "/hits", b"").1 == want.as_bytes()).then_some(()));
    }

    /// `APP` in this process with `workers` workers; returns the port.
    pub fn serve_in_process(workers: usize) -> u16 {
        let limits = Limits {
            timeout_secs: 2,
            fuel: 1_000_000_000_000,
            ..Limits::web()
        };
        let mut server = Server::new(vec![("app.kio".into(), APP.into())], limits)
            .unwrap()
            .unwrap();
        server.grant_kvs(vec![("store".into(), Arc::new(kv::Store::memory(1 << 20)))]);
        let dir = tempfile::tempdir().unwrap();
        let dirs = vec![("public".to_string(), open_dir(dir.path()).unwrap())];
        let (tx, rx) = std::sync::mpsc::channel();
        let limits = ListenLimits {
            workers,
            ..Default::default()
        };
        std::thread::spawn(move || {
            let _dir = dir;
            let _ = serve_with(server, dirs, "127.0.0.1:0".parse().unwrap(), limits, |a| {
                tx.send(a.port()).unwrap()
            });
        });
        rx.recv().unwrap()
    }
}

/// The response arrives while `main` still runs: its trailer is written only at the 2 s deadline.
#[test]
fn send_answers_before_main_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (kio, log) = after_send::start(dir.path());
    let start = std::time::Instant::now();
    assert_eq!(send(kio.port, "GET", "/slow", b""), (200, b"ok".to_vec()));
    let answered = start.elapsed();
    assert!(
        after_send::end(&log, 1).is_none(),
        "main ended before the response arrived"
    );
    let end = after_send::eventually(|| after_send::end(&log, 1));
    let ended = start.elapsed();
    println!("response after {answered:?}, main ended after {ended:?}");
    assert!(answered < std::time::Duration::from_millis(500), "{answered:?}");
    assert!(ended >= std::time::Duration::from_millis(1500), "{ended:?}");
    // Stopped by the deadline after the send: audited; the client already has its answer.
    assert_eq!(
        (end["outcome"].as_str(), end["after_send"].as_u64()),
        (Some("error"), Some(0))
    );
}

/// Work done after `send` reaches a `Kv` a later request reads; the trailer counts that call.
#[test]
fn work_after_send_is_visible_to_a_later_request() {
    let dir = tempfile::tempdir().unwrap();
    let (kio, log) = after_send::start(dir.path());
    assert_eq!(send(kio.port, "GET", "/count", b""), (200, b"ok".to_vec()));
    after_send::hits_reach(kio.port, "1");
    let end = after_send::eventually(|| after_send::end(&log, 1));
    assert_eq!(end["outcome"], "ok");
    assert_eq!((end["calls"].as_u64(), end["after_send"].as_u64()), (Some(3), Some(1)));
    // `/hits` sends last: nothing after it.
    let end = after_send::eventually(|| after_send::end(&log, 2));
    assert_eq!(end["after_send"], 0);
}

/// A trap after `send`: the client has its 200, and the audit log has the error.
#[test]
fn trap_after_send_is_audited_and_the_client_got_200() {
    let dir = tempfile::tempdir().unwrap();
    let (kio, log) = after_send::start(dir.path());
    assert_eq!(send(kio.port, "GET", "/trap", b""), (200, b"ok".to_vec()));
    let end = after_send::eventually(|| after_send::end(&log, 1));
    assert_eq!(
        (end["outcome"].as_str(), end["after_send"].as_u64()),
        (Some("error"), Some(0))
    );
    assert_eq!(send(kio.port, "GET", "/", b"").0, 200);
}

/// A request that never sends is a 500 once `main` ends, as before H18, with no `after_send`.
#[test]
fn never_sent_is_a_500() {
    let dir = tempfile::tempdir().unwrap();
    let (kio, log) = after_send::start(dir.path());
    assert_eq!(send(kio.port, "GET", "/none", b""), (500, vec![]));
    let end = after_send::end(&log, 1).expect("the trailer is written before the 500");
    assert_eq!(end["outcome"], "ok");
    assert!(end.get("after_send").is_none());
}

/// Keep-alive: the next request on a connection does not wait for the last one's background
/// work, and 200 counting requests in turn on one connection all count.
#[test]
fn keep_alive_does_not_wait_for_background_work() {
    let dir = tempfile::tempdir().unwrap();
    let (kio, log) = after_send::start(dir.path());
    let start = std::time::Instant::now();
    assert_eq!(send_keep_alive(kio.port, &["/slow".into(), "/".into()]), [200, 200]);
    assert!(start.elapsed() < std::time::Duration::from_millis(1000));
    assert!(after_send::end(&log, 1).is_none());
    let paths: Vec<String> = (0..200).map(|_| "/count".into()).collect();
    assert!(send_keep_alive(kio.port, &paths).iter().all(|&s| s == 200));
    after_send::hits_reach(kio.port, "200");
}

/// H18 review: one connection has at most two runs alive. Three `/slow` in turn on a keep-alive
/// connection: two are answered at once and hold their workers to the 2 s deadline, the third
/// waits for the first to end; a second client finds the third worker free meanwhile.
#[test]
fn a_connection_holds_at_most_two_runs() {
    let port = after_send::serve_in_process(3);
    let start = std::time::Instant::now();
    let slow: Vec<String> = vec!["/slow".into(); 3];
    let client = std::thread::spawn(move || send_keep_alive_timed(port, &slow));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(send(port, "GET", "/", b""), (200, b"ok".to_vec()));
    assert!(
        start.elapsed() < std::time::Duration::from_millis(1000),
        "{:?}",
        start.elapsed()
    );
    let timed = client.join().unwrap();
    assert!(timed.iter().all(|(s, _)| *s == 200), "{timed:?}");
    assert!(timed[0].1 < std::time::Duration::from_millis(500), "{timed:?}");
    assert!(timed[1].1 < std::time::Duration::from_millis(500), "{timed:?}");
    assert!(timed[2].1 > std::time::Duration::from_millis(1200), "{timed:?}");
}

/// Background work holds its worker: with the only worker busy after `send`, the next request
/// waits and is 503 after the 1 s queue wait; once `main` ends, requests are served again.
#[test]
fn background_work_holds_its_worker() {
    let port = after_send::serve_in_process(1);
    let start = std::time::Instant::now();
    assert_eq!(send(port, "GET", "/slow", b""), (200, b"ok".to_vec()));
    assert_eq!(send(port, "GET", "/", b"").0, 503);
    assert!(start.elapsed() < std::time::Duration::from_millis(1800));
    std::thread::sleep(std::time::Duration::from_millis(2300).saturating_sub(start.elapsed()));
    assert_eq!(send(port, "GET", "/", b""), (200, b"ok".to_vec()));
}
