//! WS1: WebSockets under `--listen`, a fresh sandbox per event.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{client, Message, WebSocket};

/// A `kio run --listen` child; killed on drop.
struct Server(Child, u16);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `kio run app --listen 127.0.0.1:0 extra..` and read the port from its stderr.
fn listen(app: &str, extra: &[&str]) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["run", app, "--listen", "127.0.0.1:0"])
        .args(extra)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("listening on ws://") {
        line.clear();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "kio exited before listening");
    }
    let port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    Server(child, port)
}

fn write(dir: &Path, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.to_str().unwrap().into()
}

/// `kio` with `args`: exit code and stderr.
fn kio(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stderr).unwrap())
}

type Ws = WebSocket<TcpStream>;

fn connect(port: u16) -> Ws {
    let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    client(format!("ws://127.0.0.1:{port}/"), tcp).unwrap().0
}

/// The next text or binary message, skipping pings and pongs.
fn next(ws: &mut Ws) -> Message {
    loop {
        match ws.read().unwrap() {
            Message::Ping(_) | Message::Pong(_) => {}
            m => return m,
        }
    }
}

fn text(ws: &mut Ws, s: &str) -> String {
    ws.send(Message::text(s)).unwrap();
    next(ws).into_text().unwrap().to_string()
}

/// The code of the close frame the server sends next.
fn close_code(ws: &mut Ws) -> u16 {
    loop {
        match ws.read() {
            Ok(Message::Close(Some(f))) => return f.code.into(),
            Ok(Message::Close(None)) => panic!("close without a code"),
            Ok(_) => {}
            Err(e) => panic!("no close frame: {e}"),
        }
    }
}

/// Close from our side and wait for the server's answer.
fn hang_up(mut ws: Ws) {
    ws.close(None).unwrap();
    while ws.read().is_ok() {}
}

#[test]
fn echoes_text_and_binary() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "hi ü"), "hi ü");
    ws.send(Message::binary(vec![0u8, 1, 255])).unwrap();
    assert_eq!(next(&mut ws), Message::binary(vec![0u8, 1, 255]));
    hang_up(ws);
}

/// Counts each kind of event in `c` (one key per kind, so concurrent events of different
/// connections do not race) and answers a text message with the counts.
const COUNT: &str = r#"fn count(c: Kv, key: Str) -> Str {
  match c.get(key).unwrap_or(None) { Some(b) => b.to_str(), None => "0" }
}
fn main(msg: WsMessage, conn: WsSend, c: Kv) -> Result<(), WsErr> {
  let kind = msg.kind();
  let n = count(c, kind).parse_int().unwrap_or(0) + 1;
  let r = c.set(kind, n.to_str().to_bytes());
  if kind == "text" { conn.text("o" + count(c, "open") + " c" + count(c, "close") + " t" + count(c, "text")) } else { Ok(()) }
}"#;

#[test]
fn open_and_close_events_are_delivered() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "count.kio", COUNT);
    let server = listen(&app, &["--kv", "c=:memory:"]);
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "x"), "o1 c0 t1");
    hang_up(ws);
    // The close event runs after the server answered our close.
    std::thread::sleep(Duration::from_millis(300));
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "x"), "o2 c1 t2");
    // A connection that drops without a close frame gets a close event too.
    drop(ws);
    std::thread::sleep(Duration::from_millis(300));
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "x"), "o3 c2 t3");
    hang_up(ws);
}

/// Answers `conn:data` so a reply shows which connection it came from.
const TAG: &str = r#"fn main(msg: WsMessage, conn: WsSend) -> Result<(), WsErr> {
  if msg.kind() == "text" { conn.text(msg.conn().to_str() + ":" + msg.data().to_str()) } else { Ok(()) }
}"#;

#[test]
fn rapid_sends_on_one_connection_stay_in_order() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let mut ws = connect(server.1);
    for i in 0..200 {
        ws.send(Message::text(i.to_string())).unwrap();
    }
    for i in 0..200 {
        assert_eq!(next(&mut ws).into_text().unwrap().as_str(), i.to_string());
    }
    hang_up(ws);
}

#[test]
fn two_connections_run_concurrently() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "tag.kio", TAG);
    let server = listen(&app, &[]);
    let (mut a, mut b) = (connect(server.1), connect(server.1));
    for i in 0..20 {
        a.send(Message::text(format!("a{i}"))).unwrap();
        b.send(Message::text(format!("b{i}"))).unwrap();
    }
    let ids = |ws: &mut Ws, who: char| {
        let mut id = None;
        for i in 0..20 {
            let got = next(ws).into_text().unwrap().to_string();
            let (conn, data) = got.split_once(':').unwrap();
            assert_eq!(data, format!("{who}{i}"));
            assert!(id.is_none() || id == Some(conn.to_string()));
            id = Some(conn.to_string());
        }
        id.unwrap()
    };
    let (ia, ib) = (ids(&mut a, 'a'), ids(&mut b, 'b'));
    assert_ne!(ia, ib);
    hang_up(a);
    hang_up(b);
}

#[test]
fn a_request_that_is_not_an_upgrade_is_426() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let mut s = TcpStream::connect(("127.0.0.1", server.1)).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 426"), "{raw}");
    assert!(raw.to_ascii_lowercase().contains("upgrade: websocket"), "{raw}");
    assert!(raw.ends_with("send an upgrade request (RFC 6455)\n"), "{raw}");
}

#[test]
fn a_message_over_one_megabyte_closes_1009() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let mut ws = connect(server.1);
    ws.send(Message::binary(vec![7u8; 1 << 20])).unwrap();
    assert_eq!(next(&mut ws), Message::binary(vec![7u8; 1 << 20]));
    // The write may fail once the server has closed.
    let _ = ws.send(Message::binary(vec![7u8; (1 << 20) + 1]));
    assert_eq!(close_code(&mut ws), 1009);
}

/// `boom` divides by zero; anything else is echoed.
const TRAP: &str = r#"fn main(msg: WsMessage, conn: WsSend) -> Result<(), WsErr> {
  if msg.kind() != "text" { return Ok(()); }
  let d = msg.data();
  let n = 10 / (d.len() - 4);
  conn.text(d.to_str())
}"#;

#[test]
fn a_trapping_handler_closes_1011_and_others_still_work() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "trap.kio", TRAP);
    let server = listen(&app, &[]);
    let mut other = connect(server.1);
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "hello"), "hello");
    ws.send(Message::text("boom")).unwrap();
    // No detail goes to the client.
    let Message::Close(Some(code)) = ws.read().unwrap() else {
        panic!("expected a close frame")
    };
    assert_eq!((code.code, code.reason.as_str()), (CloseCode::Error, ""));
    assert_eq!(text(&mut other, "still"), "still");
    let mut fresh = connect(server.1);
    assert_eq!(text(&mut fresh, "fresh"), "fresh");
}

/// Probes `WsSend`'s errors and reports them: in a close code (after a close nothing else can
/// go out) or, for a later connection, through `c`.
const PROBE: &str = r#"fn show(r: Result<(), WsErr>) -> Str {
  match r { Ok(_) => "ok", Err(e) => if e == WsErr::Closed { "closed" } else if e == WsErr::TooLarge { "too-large" } else { "invalid" } }
}
fn big(n: Int) -> Str {
  let mut s = "x";
  for i in 0..n { s = s + s; }
  s
}
fn main(msg: WsMessage, conn: WsSend, c: Kv) -> Result<(), WsErr> {
  let kind = msg.kind();
  if kind == "close" { let r = c.set("on_close", show(conn.text("late")).to_bytes()); return Ok(()); }
  if kind != "text" { return Ok(()); }
  let d = msg.data().to_str();
  if d == "close-then-send" {
    conn.close(4000)?;
    let r = c.set("after", (show(conn.text("x")) + " " + show(conn.close(4001))).to_bytes());
    return Ok(());
  }
  if d == "bad-codes" {
    let mut out = "";
    for code in [999, 1004, 1005, 1006, 1015, 2000, 5000, 70000] { out = out + show(conn.close(code)) + " "; }
    return conn.text(out);
  }
  if d == "too-large" {
    let r = conn.text(big(20));
    if show(conn.binary("y".to_bytes())) == "too-large" { return conn.close(4002); }
    return conn.close(4003);
  }
  let v = match c.get(d).unwrap_or(None) { Some(b) => b.to_str(), None => "none" };
  conn.text(v)
}"#;

#[test]
fn send_errors_closed_invalid_and_too_large() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "probe.kio", PROBE);
    let server = listen(&app, &["--kv", "c=:memory:"]);

    let mut ws = connect(server.1);
    assert_eq!(
        text(&mut ws, "bad-codes"),
        "invalid invalid invalid invalid invalid invalid invalid invalid "
    );
    ws.send(Message::text("close-then-send")).unwrap();
    assert_eq!(close_code(&mut ws), 4000);
    while ws.read().is_ok() {}

    let mut ws = connect(server.1);
    ws.send(Message::text("too-large")).unwrap();
    assert_eq!(next(&mut ws).into_text().unwrap().len(), 1 << 20);
    assert_eq!(close_code(&mut ws), 4002);
    while ws.read().is_ok() {}

    // A send after close in the same run, and any send on the close event, is `closed`.
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "after"), "closed closed");
    let on_close = (0..50)
        .map(|_| {
            std::thread::sleep(Duration::from_millis(20));
            text(&mut ws, "on_close")
        })
        .find(|v| v != "none");
    assert_eq!(on_close.as_deref(), Some("closed"));
    hang_up(ws);
}

/// `slow` keeps its sandbox busy; meanwhile the queue fills.
const SLOW: &str = r#"fn main(msg: WsMessage, conn: WsSend) -> Result<(), WsErr> {
  if msg.data().to_str() == "slow" { let mut i = 0; while i < 2000000 { i += 1; } }
  Ok(())
}"#;

#[test]
fn more_than_64_kb_waiting_closes_1008() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "slow.kio", SLOW);
    let server = listen(&app, &[]);
    let mut ws = connect(server.1);
    ws.send(Message::text("slow")).unwrap();
    for _ in 0..100 {
        if ws.send(Message::binary(vec![0u8; 1024])).is_err() {
            break;
        }
    }
    assert_eq!(close_code(&mut ws), 1008);
}

#[test]
fn an_idle_connection_closes_1000() {
    use kio_host::listen::{serve_with, ListenLimits};
    use kio_host::{Limits, Server};
    let src = std::fs::read_to_string("../examples/ws/echo.kio").unwrap();
    let server = Server::new(vec![("echo.kio".into(), src)], Limits::web())
        .unwrap()
        .unwrap();
    let limits = ListenLimits {
        ws_idle: Duration::from_millis(300),
        ..Default::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = serve_with(server, vec![], "127.0.0.1:0".parse().unwrap(), limits, |a| {
            tx.send(a.port()).unwrap()
        });
    });
    let mut ws = connect(rx.recv().unwrap());
    assert_eq!(text(&mut ws, "a"), "a");
    assert_eq!(close_code(&mut ws), 1000);
}

#[test]
fn grant_rules_are_checked_at_startup() {
    let t = tempfile::tempdir().unwrap();
    let cases = [
        (
            "fn main(msg: WsMessage) -> Result<(), WsErr> { Ok(()) }",
            "hint: write `fn main(msg: WsMessage, conn: WsSend, ..)`",
        ),
        (
            "fn main(msg: WsMessage, conn: WsSend, req: Incoming) -> Result<(), WsErr> { Ok(()) }",
            "hint: a WebSocket server answers through WsSend; drop the Incoming and Outgoing parameters",
        ),
    ];
    for (src, hint) in cases {
        let app = write(t.path(), "bad.kio", src);
        let (code, err) = kio(&["run", &app, "--listen", "127.0.0.1:0"]);
        assert_eq!(code, 2, "{src}");
        assert!(err.contains(hint), "{err}");
    }
    let (code, err) = kio(&["run", "../examples/ws/echo.kio"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("main takes WsMessage/WsSend but there is no address\n  hint: add `--listen 127.0.0.1:8080`"),
        "{err}"
    );
    let (code, err) = kio(&[
        "run",
        "../examples/serve/app.kio",
        "--dir",
        "public=../examples/serve",
        "--listen",
        "127.0.0.1:0",
        "--ws-origin",
        "https://app.example",
    ]);
    assert_eq!(code, 2);
    assert!(
        err.contains("--ws-origin without a WebSocket server\n  hint: "),
        "{err}"
    );
}

/// A raw client frame, masked unless `mask` is false.
fn frame(fin: bool, rsv: u8, opcode: u8, payload: &[u8], mask: bool) -> Vec<u8> {
    let mut f = vec![(if fin { 0x80 } else { 0 }) | (rsv << 4) | opcode];
    let m = if mask { 0x80 } else { 0 };
    let n = payload.len();
    if n < 126 {
        f.push(m | n as u8);
    } else if n < 65536 {
        f.push(m | 126);
        f.extend((n as u16).to_be_bytes());
    } else {
        f.push(m | 127);
        f.extend((n as u64).to_be_bytes());
    }
    if mask {
        let key = [1u8, 2, 3, 4];
        f.extend(key);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
    } else {
        f.extend(payload);
    }
    f
}

/// An upgrade request with `extra` headers, then `trailer`: the socket and the response head.
fn raw_upgrade(port: u16, extra: &str, trailer: &[u8]) -> (TcpStream, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut req = format!(
        "GET / HTTP/1.1\r\nhost: localhost\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n{extra}\r\n"
    )
    .into_bytes();
    req.extend(trailer);
    s.write_all(&req).unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap() == 1 {
        head.push(b[0]);
    }
    (s, String::from_utf8_lossy(&head).to_string())
}

/// One server frame: opcode and payload.
fn read_frame(s: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).ok()?;
    let mut n = (h[1] & 0x7f) as usize;
    if n == 126 {
        let mut e = [0u8; 2];
        s.read_exact(&mut e).ok()?;
        n = u16::from_be_bytes(e) as usize;
    } else if n == 127 {
        let mut e = [0u8; 8];
        s.read_exact(&mut e).ok()?;
        n = u64::from_be_bytes(e) as usize;
    }
    let mut p = vec![0u8; n];
    s.read_exact(&mut p).ok()?;
    Some((h[0] & 0x0f, p))
}

/// The code of the next close frame, or `None` if the socket ends first.
fn close_of(s: &mut TcpStream) -> Option<u16> {
    loop {
        let (op, p) = read_frame(s)?;
        if op == 8 {
            return Some(u16::from_be_bytes([p[0], p[1]]));
        }
    }
}

/// Serve `src` in-process with `limits` and a `c` store, as `kio run --listen` would.
fn serve(src: &str, fuel: u64, limits: kio_host::listen::ListenLimits) -> u16 {
    use kio_host::kv::open_all;
    use kio_host::{Limits, Server};
    let run = Limits { fuel, ..Limits::web() };
    let mut server = Server::new(vec![("app.kio".into(), src.into())], run).unwrap().unwrap();
    server.grant_kvs(open_all(&[("c".into(), ":memory:".into())], 64).unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = kio_host::listen::serve_with(server, vec![], "127.0.0.1:0".parse().unwrap(), limits, |a| {
            tx.send(a.port()).unwrap()
        });
    });
    rx.recv().unwrap()
}

/// Counts opens and closes in `c`; `?` answers the counts, `slow` runs for about two seconds,
/// any other text is echoed.
const SLOW_COUNT: &str = r#"fn count(c: Kv, key: Str) -> Int {
  match c.get(key).unwrap_or(None) { Some(b) => b.to_str().parse_int().unwrap_or(0), None => 0 }
}
fn main(msg: WsMessage, conn: WsSend, c: Kv) -> Result<(), WsErr> {
  let kind = msg.kind();
  if kind == "text" && msg.data().to_str() == "?" {
    return conn.text("o" + count(c, "open").to_str() + " c" + count(c, "close").to_str());
  }
  let r = c.set(kind, (count(c, kind) + 1).to_str().to_bytes());
  if kind == "text" && msg.data().to_str() == "slow" { let mut i = 0; while i < 2000000 { i += 1; } }
  if kind == "text" { conn.text(msg.data().to_str()) } else { Ok(()) }
}"#;

#[test]
fn under_overload_an_event_waits_for_a_worker() {
    use kio_host::listen::ListenLimits;
    let limits = ListenLimits {
        workers: 1,
        queued: 1,
        ..Default::default()
    };
    let port = serve(SLOW_COUNT, 100_000_000_000, limits);
    let (mut a1, mut a2, mut victim, mut probe) = (connect(port), connect(port), connect(port), connect(port));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(text(&mut probe, "?"), "o4 c0");
    // a1's event takes the one worker and a2's the one HTTP queue place; the victim waits too.
    a1.send(Message::text("slow")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    a2.send(Message::text("slow")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(text(&mut victim, "victim"), "victim");
    assert_eq!(next(&mut a1).into_text().unwrap().as_str(), "slow");
    assert_eq!(next(&mut a2).into_text().unwrap().as_str(), "slow");
    // Every open gets its close.
    hang_up(victim);
    drop(a1);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(text(&mut probe, "?"), "o4 c2");
    hang_up(a2);
}

/// Counts each kind of event in `c`; `boom` traps and `err` returns `Err`; any other text
/// answers the open and close counts.
const FAIL: &str = r#"fn count(c: Kv, key: Str) -> Str {
  match c.get(key).unwrap_or(None) { Some(b) => b.to_str(), None => "0" }
}
fn main(msg: WsMessage, conn: WsSend, c: Kv) -> Result<(), WsErr> {
  let kind = msg.kind();
  let n = count(c, kind).parse_int().unwrap_or(0) + 1;
  let r = c.set(kind, n.to_str().to_bytes());
  if kind != "text" { return Ok(()); }
  let d = msg.data();
  if d.to_str() == "boom" { let x = 10 / (d.len() - 4); }
  if d.to_str() == "err" { return Err(WsErr::Invalid); }
  conn.text("o" + count(c, "open") + " c" + count(c, "close") + " t" + count(c, "text"))
}"#;

#[test]
fn a_failed_run_still_gets_its_close_event() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "fail.kio", FAIL);
    let server = listen(&app, &["--kv", "c=:memory:"]);
    // A trap, then the closing handshake.
    let mut ws = connect(server.1);
    ws.send(Message::text("boom")).unwrap();
    assert_eq!(close_code(&mut ws), 1011);
    while ws.read().is_ok() {}
    // `Err` from main, with messages waiting behind it, then the peer vanishes.
    let mut ws = connect(server.1);
    ws.send(Message::text("err")).unwrap();
    for _ in 0..10 {
        let _ = ws.send(Message::text("dropped"));
    }
    assert_eq!(close_code(&mut ws), 1011);
    drop(ws);
    let mut ws = connect(server.1);
    for asked in 1..=50 {
        std::thread::sleep(Duration::from_millis(20));
        // The failed runs' texts and these, not the dropped ones.
        if text(&mut ws, "x") == format!("o3 c2 t{}", 2 + asked) {
            return hang_up(ws);
        }
    }
    panic!("no close event after a failed run");
}

/// Counts each kind of event; `?` answers the counts.
const TALLY: &str = r#"fn count(c: Kv, key: Str) -> Str {
  match c.get(key).unwrap_or(None) { Some(b) => b.to_str(), None => "0" }
}
fn main(msg: WsMessage, conn: WsSend, c: Kv) -> Result<(), WsErr> {
  let kind = msg.kind();
  let n = count(c, kind).parse_int().unwrap_or(0) + 1;
  let r = c.set(kind, n.to_str().to_bytes());
  if kind == "text" && msg.data().to_str() == "?" { conn.text("o" + count(c, "open") + " c" + count(c, "close") + " t" + count(c, "text")) } else { Ok(()) }
}"#;

/// Asks `port` for the tally once the departed connection's close has run: (opens, closes, texts).
fn tally(port: u16) -> (u32, u32, u32) {
    std::thread::sleep(Duration::from_millis(500));
    let mut ws = connect(port);
    let r = text(&mut ws, "?");
    hang_up(ws);
    let n: Vec<u32> = r.split(' ').map(|p| p[1..].parse().unwrap()).collect();
    (n[0], n[1], n[2])
}

#[test]
fn messages_waiting_at_the_peers_close_are_dropped() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "tally.kio", TALLY);
    let server = listen(&app, &["--kv", "c=:memory:"]);
    let (mut s, _) = raw_upgrade(server.1, "", &[]);
    let mut buf = Vec::new();
    for _ in 0..100 {
        buf.extend(frame(true, 0, 1, b"", true));
    }
    buf.extend(frame(true, 0, 8, &[0x03, 0xe8], true));
    // Data after the close frame is never delivered.
    for _ in 0..100 {
        buf.extend(frame(true, 0, 1, b"", true));
    }
    s.write_all(&buf).unwrap();
    assert_eq!(close_of(&mut s), Some(1000));
    drop(s);
    // At most the message already running when the close arrived, plus the probe's.
    let (opens, closes, texts) = tally(server.1);
    assert_eq!((opens, closes), (2, 1));
    assert!(texts <= 5, "{texts} texts ran");
}

#[test]
fn a_peer_gone_without_a_close_frame_gets_no_close_and_its_messages_are_dropped() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "tally.kio", TALLY);
    let server = listen(&app, &["--kv", "c=:memory:"]);
    let (mut s, _) = raw_upgrade(server.1, "", &[]);
    let mut buf = Vec::new();
    for _ in 0..100 {
        buf.extend(frame(true, 0, 1, b"", true));
    }
    s.write_all(&buf).unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    assert_eq!(close_of(&mut s), None);
    drop(s);
    let (opens, closes, texts) = tally(server.1);
    assert_eq!((opens, closes), (2, 1));
    assert!(texts <= 5, "{texts} texts ran");
}

#[test]
fn pipelined_frames_after_upgrade_are_read() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let (mut s, head) = raw_upgrade(server.1, "", &frame(true, 0, 1, b"pipelined", true));
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let (op, p) = read_frame(&mut s).unwrap();
    assert_eq!((op, p.as_slice()), (1, &b"pipelined"[..]));
}

#[test]
fn invalid_utf8_across_fragments_closes_1007() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let (mut s, head) = raw_upgrade(server.1, "", &[]);
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    // "é" is c3 a9: its first byte ends one fragment and a bad continuation starts the next.
    s.write_all(&frame(false, 0, 1, b"ab\xc3", true)).unwrap();
    s.write_all(&frame(true, 0, 0, b"\xff", true)).unwrap();
    assert_eq!(close_of(&mut s), Some(1007));
}

#[test]
fn protocol_errors_close_1002() {
    let server = listen("../examples/ws/echo.kio", &[]);
    for (name, bytes) in [
        ("rsv1", frame(true, 1, 1, b"x", true)),
        ("unmasked", frame(true, 0, 1, b"x", false)),
        ("continue without start", frame(true, 0, 0, b"x", true)),
        ("reserved opcode", frame(true, 0, 3, b"x", true)),
        ("close with 1 byte", frame(true, 0, 8, b"\x03", true)),
        ("close code 1005", frame(true, 0, 8, &[0x03, 0xed], true)),
        ("fragmented ping", frame(false, 0, 9, b"", true)),
        ("ping > 125", frame(true, 0, 9, &[0u8; 126], true)),
        (
            "interleaved data",
            [frame(false, 0, 1, b"a", true), frame(true, 0, 2, b"b", true)].concat(),
        ),
    ] {
        let (mut s, head) = raw_upgrade(server.1, "", &[]);
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        s.write_all(&bytes).unwrap();
        assert_eq!(close_of(&mut s), Some(1002), "{name}");
    }
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "still"), "still");
}

#[test]
fn the_connection_slot_is_freed_in_each_state() {
    use kio_host::listen::{serve_with, ListenLimits};
    use kio_host::{Limits, Server};
    let src = std::fs::read_to_string("../examples/ws/echo.kio").unwrap();
    let server = Server::new(vec![("echo.kio".into(), src)], Limits::web())
        .unwrap()
        .unwrap();
    let limits = ListenLimits {
        connections: 1,
        ..Default::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = serve_with(server, vec![], "127.0.0.1:0".parse().unwrap(), limits, |a| {
            tx.send(a.port()).unwrap()
        });
    });
    let port = rx.recv().unwrap();
    // The server drops a connection past the cap without reading it.
    let full = |why: &str| {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let r = s.read(&mut [0u8; 1]);
        assert!(matches!(r, Ok(0) | Err(_)), "{why}: slot was free: {r:?}");
    };
    let free = |why: &str| {
        let start = std::time::Instant::now();
        loop {
            let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            if let Ok((ws, _)) = client(format!("ws://127.0.0.1:{port}/"), tcp) {
                return ws;
            }
            assert!(start.elapsed() < Duration::from_secs(15), "{why}: slot never freed");
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    // An open WebSocket holds the slot; a clean close frees it.
    let mut ws = free("start");
    assert_eq!(text(&mut ws, "a"), "a");
    full("open socket");
    hang_up(ws);
    // A 426, then a disconnect.
    drop(free("after close"));
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n").unwrap();
    let _ = s.read(&mut [0u8; 100]);
    drop(s);
    // Gone halfway through the upgrade's headers.
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: upg")
        .unwrap();
    drop(s);
    let mut ws = free("after 426 and half an upgrade");
    // The peer closes but keeps the TCP connection open.
    ws.send(Message::Close(None)).unwrap();
    let _ = ws.read();
    let ws2 = free("peer close without FIN");
    drop((ws, ws2));
    // Gone without a close frame.
    drop(free("after the peer's close"));
    drop(free("after a vanished peer"));
}

#[test]
fn a_browser_origin_must_be_the_servers_host_or_allowed() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let status = |port, extra| raw_upgrade(port, extra, &[]).1;
    // No Origin (not a browser), and a page served from the same host, are accepted.
    assert!(status(server.1, "").starts_with("HTTP/1.1 101"));
    assert!(status(server.1, "origin: http://localhost\r\n").starts_with("HTTP/1.1 101"));
    assert!(status(server.1, "origin: https://LOCALHOST\r\n").starts_with("HTTP/1.1 101"));
    // A page on any other site is not.
    for origin in [
        "http://evil.example",
        "null",
        "http://localhost.evil.example",
        "http://localhost:81",
    ] {
        let (mut s, head) = raw_upgrade(server.1, &format!("origin: {origin}\r\n"), &[]);
        assert!(head.starts_with("HTTP/1.1 403"), "{origin}: {head}");
        let mut body = vec![0u8; 200];
        let n = s.read(&mut body).unwrap();
        let body = String::from_utf8_lossy(&body[..n]);
        assert!(body.contains("allows it with `--ws-origin <origin>`"), "{body}");
    }
    drop(server);
    let server = listen("../examples/ws/echo.kio", &["--ws-origin", "https://app.example"]);
    assert!(status(server.1, "origin: https://app.example\r\n").starts_with("HTTP/1.1 101"));
    assert!(status(server.1, "origin: http://app.example\r\n").starts_with("HTTP/1.1 403"));
}

/// H11: a WebSocket upgrade with a foreign `Host` is 421 before any sandbox runs; `--host` allows it.
#[test]
fn a_foreign_host_cannot_upgrade() {
    let upgrade = |port, host: &str| {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let req = format!("GET / HTTP/1.1\r\nhost: {host}\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n");
        s.write_all(req.as_bytes()).unwrap();
        let mut buf = vec![0u8; 400];
        let n = s.read(&mut buf).unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    };
    let server = listen("../examples/ws/echo.kio", &[]);
    for host in ["localhost", "127.0.0.1:1", "[::1]:2", "LocalHost"] {
        assert!(upgrade(server.1, host).starts_with("HTTP/1.1 101"), "{host}");
    }
    let head = upgrade(server.1, "evil.example");
    assert!(head.starts_with("HTTP/1.1 421"), "{head}");
    assert!(head.contains("`--host <name>`"), "{head}");
    drop(server);
    let server = listen("../examples/ws/echo.kio", &["--host", "evil.example"]);
    assert!(upgrade(server.1, "evil.example:80").starts_with("HTTP/1.1 101"));
    assert!(upgrade(server.1, "other.example").starts_with("HTTP/1.1 421"));
}

/// Records what each event sees of the upgrade request under `<kind><conn>`; a text message `n`
/// answers with what connection `n`'s open, text and close events saw.
const UPGRADE: &str = r#"fn show(o: Option<Str>) -> Str { match o { Some(s) => s, None => "none" } }
fn get(c: Kv, key: Str) -> Str { match c.get(key).unwrap_or(None) { Some(b) => b.to_str(), None => "-" } }
fn main(msg: WsMessage, conn: WsSend, c: Kv) -> Result<(), WsErr> {
  let kind = msg.kind();
  let seen = msg.target() + " " + show(msg.header("x-token")) + " " + show(msg.header("X-TOKEN")) + " " + show(msg.header("x-absent"));
  let r = c.set(kind + msg.conn().to_str(), seen.to_bytes());
  if kind == "text" {
    let n = msg.data().to_str();
    conn.text(get(c, "open" + n) + "|" + get(c, "text" + n) + "|" + get(c, "close" + n))
  } else { Ok(()) }
}"#;

/// WS1b: every event of a connection sees its upgrade request's target and headers: the first
/// value of a header, in any case, `None` when absent.
#[test]
fn every_event_sees_the_upgrade_request() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "upgrade.kio", UPGRADE);
    let server = listen(&app, &["--kv", "c=:memory:"]);
    let port = server.1;
    let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("ws://127.0.0.1:{port}/room%201?x=y")
        .into_client_request()
        .unwrap();
    req.headers_mut().append("X-Token", "abc".parse().unwrap());
    req.headers_mut().append("x-token", "def".parse().unwrap());
    let mut ws = client(req, tcp).unwrap().0;
    let seen = "/room%201?x=y abc abc none";
    assert_eq!(text(&mut ws, "1"), format!("{seen}|{seen}|-"));
    hang_up(ws);
    std::thread::sleep(Duration::from_millis(300));
    // The close event saw the same; another connection sees its own request.
    let mut ws = connect(port);
    assert_eq!(text(&mut ws, "1"), format!("{seen}|{seen}|{seen}"));
    assert_eq!(text(&mut ws, "2"), "/ none none none|/ none none none|-");
    hang_up(ws);
}

/// WS1b: an upgrade with a header value that is not UTF-8 is 400, and no event runs.
#[test]
fn a_non_utf8_header_refuses_the_upgrade() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "count.kio", COUNT);
    let server = listen(&app, &["--kv", "c=:memory:"]);
    let mut s = TcpStream::connect(("127.0.0.1", server.1)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let req = b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nx-bad: caf\xe9\r\n\r\n";
    s.write_all(req).unwrap();
    let mut buf = vec![0u8; 400];
    let n = s.read(&mut buf).unwrap();
    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    std::thread::sleep(Duration::from_millis(300));
    let mut ws = connect(server.1);
    assert_eq!(text(&mut ws, "x"), "o1 c0 t1");
    hang_up(ws);
}

/// Release only: `cargo test --release -p kio --test ws -- --ignored --nocapture`.
#[test]
#[ignore]
fn echo_events_per_second() {
    let server = listen("../examples/ws/echo.kio", &[]);
    let mut ws = connect(server.1);
    let n = 5000;
    let start = std::time::Instant::now();
    for i in 0..n {
        assert_eq!(text(&mut ws, &i.to_string()), i.to_string());
    }
    let one = n as f64 / start.elapsed().as_secs_f64();
    let start = std::time::Instant::now();
    for i in 0..n {
        ws.send(Message::text(i.to_string())).unwrap();
        if i % 100 == 99 {
            for j in i - 99..=i {
                assert_eq!(next(&mut ws).into_text().unwrap().as_str(), j.to_string());
            }
        }
    }
    let piped = n as f64 / start.elapsed().as_secs_f64();
    println!("echo over one connection: {one:.0} events/s one at a time, {piped:.0} events/s 100 in flight");
}

/// H12: under `--audit` each event is a request in the server's one log, and its lines name the
/// connection it came from.
#[test]
fn audit_lines_name_the_connection() {
    let t = tempfile::tempdir().unwrap();
    let log = t.path().join("audit.log");
    let server = listen("../examples/ws/echo.kio", &["--audit", log.to_str().unwrap()]);
    for _ in 0..2 {
        let mut ws = connect(server.1);
        assert_eq!(text(&mut ws, "hi"), "hi");
        hang_up(ws);
    }
    // The close events run after the server answered our close.
    let read = || -> Vec<serde_json::Value> {
        let text = std::fs::read_to_string(&log).unwrap();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    };
    let mut lines = read();
    for _ in 0..50 {
        if lines.iter().filter(|l| l["event"] == "end").count() == 6 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        lines = read();
    }
    assert_eq!(lines[0]["event"], "start");
    let ends: Vec<_> = lines.iter().filter(|l| l["event"] == "end").collect();
    assert_eq!(ends.len(), 6, "{lines:?}");
    let mut reqs: Vec<u64> = ends.iter().map(|l| l["req"].as_u64().unwrap()).collect();
    reqs.sort();
    assert_eq!(reqs, [1, 2, 3, 4, 5, 6]);
    for conn in [1, 2] {
        assert_eq!(ends.iter().filter(|l| l["conn"] == conn).count(), 3);
    }
    // Every call line carries the same connection as its request's trailer.
    for l in &lines[1..] {
        let end = ends.iter().find(|e| e["req"] == l["req"]).unwrap();
        assert_eq!(l["conn"], end["conn"]);
    }
}
