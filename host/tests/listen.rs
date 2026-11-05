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
    for (i, t) in threads.into_iter().enumerate() {
        let (status, body) = t.join().unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, format!(r#"{{"hello": "n{i}"}}"#).into_bytes());
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
