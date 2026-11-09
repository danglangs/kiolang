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
    s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
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
