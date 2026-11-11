//! #164 in a process of its own: the HTTP pool's ceiling is process-wide and every in-process
//! server sizes it from its own workers (`size_http_pool`), so another test's server in the
//! same binary (listen.rs has several, at the machine's default workers) could shrink it under
//! this one's fan-outs: on a 4-core runner to 1,024, exactly what they hold.

mod upstream;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// One GET on a fresh connection; returns the status and body.
fn get(port: u16, target: &str) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!("GET {target} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n");
    s.write_all(head.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("no response head") + 4;
    let status = std::str::from_utf8(&raw[..split]).unwrap().split(' ').nth(1).unwrap();
    (status.parse().unwrap(), raw[split..].to_vec())
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
    // The fan-outs hang until this deadline; starting 1,024 upstream connections on a slow
    // runner took over 2 s.
    let mut server = Server::new(
        vec![("app.kio".into(), APP.into())],
        Limits {
            timeout_secs: 10,
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
    let fans: Vec<_> = (0..16).map(|_| std::thread::spawn(move || get(port, "/fan"))).collect();
    let start = Instant::now();
    while up.seen.lock().unwrap().len() < 16 * 64 {
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "the fan-outs never all started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let one = Instant::now();
    assert_eq!(get(port, "/one"), (200, b"GET /ok ".to_vec()));
    let took = one.elapsed();
    // Answered while every fan-out still hangs, not after their deadline freed the pool.
    assert!(fans.iter().all(|f| !f.is_finished()), "a fan-out ended first");
    assert!(took < Duration::from_secs(5), "{took:?}");
    for f in fans {
        let (status, _) = f.join().unwrap();
        assert!(matches!(status, 200 | 500), "{status}");
    }
}
