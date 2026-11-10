//! H22: `Hmac` signs and verifies under a key the host read at start; the program never holds
//! it. Interpreted and compiled, through `kio run` and under `--listen`.
use kio_host::{HmacKey, Interp, Kio, Limits, RunResult, Sources};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};

const KEY: &str = "whsec_h22-env-5c1e9a07-never-shown";
const FILE_KEY: &str = "whsec_h22-file-d4b2f6e1-never-shown";

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

impl Buf {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// An independent HMAC-SHA256 (RFC 2104) over the `sha2` crate.
fn reference(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new()
        .chain_update(k.map(|b| b ^ 0x36))
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(k.map(|b| b ^ 0x5c))
        .chain_update(inner)
        .finalize()
        .to_vec()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn key(value: &[u8], verify_only: bool) -> Sources {
    let value = String::from_utf8(value.to_vec()).unwrap();
    Sources {
        hmacs: vec![("k".into(), HmacKey::new(value, verify_only).unwrap())],
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

/// Lowercase hex to `Bytes`, in kio.
const UNHEX: &str = r#"
fn digit(c: Str) -> Int { "0123456789abcdef".find(c).unwrap_or(0) }
fn unhex(s: Str) -> Bytes {
  let cs = s.chars();
  let mut b = "".to_bytes();
  let mut i = 0;
  while i + 1 < cs.len() { b.push(digit(cs[i]) * 16 + digit(cs[i + 1])); i += 2; }
  b
}
"#;

/// Signs `msg` (hex), then verifies the MAC, a MAC with its last byte changed, a short one, a
/// long one, an empty one, and the MAC against another message.
fn program(msg_hex: &str) -> String {
    format!(
        r#"{UNHEX}
fn main(k: Hmac) -> Str {{
  let msg = unhex("{msg_hex}");
  match k.sign(msg) {{
    Ok(mac) => {{
      let mut bad = mac.slice(0, 31);
      bad.push((mac[31] + 1) % 256);
      let mut long = mac.slice(0, 32);
      long.push(0);
      let checks = [k.verify(msg, mac), k.verify(msg, bad), k.verify(msg, mac.slice(0, 16)),
        k.verify(msg, long), k.verify(msg, "".to_bytes()), k.verify(unhex("00"), mac)];
      mac.to_hex() + " " + checks.map(|c| c.to_str()).join(",")
    }}
    Err(HmacErr::Denied) => "denied",
  }}
}}
"#
    )
}

/// RFC 4231's cases whose keys are UTF-8 (a key is the bytes of an environment variable or a
/// file's text): 1, 2, 4 and 5 (truncated to 128 bits). All seven are a unit test in lib.rs.
#[test]
fn rfc_4231_vectors_through_kio_on_both_paths() {
    let key4: Vec<u8> = (1..=25).collect();
    let cases: [(&[u8], &[u8], &str); 4] = [
        (
            &[0x0b; 20],
            b"Hi There",
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        ),
        (
            b"Jefe",
            b"what do ya want for nothing?",
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        ),
        (
            &key4,
            &[0xcd; 50],
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
        ),
        (&[0x0c; 20], b"Test With Truncation", "a3b6167473100ee06e0c796c2955552b"),
    ];
    for (k, msg, want) in cases {
        let got = both(&program(&hex(msg)), || key(k, false));
        let (mac, checks) = got.split_once(' ').unwrap();
        assert!(mac.starts_with(want), "{got}");
        assert_eq!(mac, hex(&reference(k, msg)));
        assert_eq!(checks, "true,false,false,false,false,false", "{got}");
    }
}

#[test]
fn messages_of_every_size_match_the_reference() {
    for n in [0, 1, 55, 56, 63, 64, 65, 119, 120, 1000, 100_000] {
        let msg: Vec<u8> = (0..n).map(|i| (i * 7 % 256) as u8).collect();
        let got = both(&program(&hex(&msg)), || key(KEY.as_bytes(), false));
        assert_eq!(
            got.split(' ').next().unwrap(),
            hex(&reference(KEY.as_bytes(), &msg)),
            "{n}"
        );
    }
}

/// Each call's line has the method, the message's length and the result; never the key or a MAC.
#[test]
fn audit_lines_on_both_paths() {
    for compiled in [true, false] {
        let audit = Buf::default();
        let v = shown(run(
            &program("6869"),
            key(KEY.as_bytes(), false),
            compiled,
            Some(audit.clone()),
        ));
        let text = audit.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 9, "{text}");
        assert!(
            lines[0].contains(r#""name":"k","kind":"Hmac","target":"sign,verify""#),
            "{text}"
        );
        assert!(
            lines[1]
                .contains(r#""cap":"k","kind":"Hmac","method":"sign","arg":"sign","bytes":2,"result":"ok","len":2"#),
            "{text}"
        );
        assert!(
            lines[2].contains(r#""method":"verify","arg":"verify","bytes":34,"result":"true","len":2"#),
            "{text}"
        );
        assert!(
            lines[3].contains(r#""method":"verify","arg":"verify","bytes":34,"result":"false","len":2"#),
            "{text}"
        );
        assert!(lines[4].contains(r#""bytes":18,"result":"false","len":2"#), "{text}");
        assert!(lines[7].contains(r#""bytes":33,"result":"false","len":1"#), "{text}");
        let mac = v.split(' ').next().unwrap();
        assert!(!text.contains(mac), "the audit log holds the MAC");
        assert_clean("the audit log", &text);
    }
}

#[test]
fn verify_only_refuses_sign_and_still_verifies() {
    assert_eq!(both(&program("6869"), || key(KEY.as_bytes(), true)), "denied");
    let mac = hex(&reference(KEY.as_bytes(), b"hi"));
    let src = format!(
        r#"{UNHEX}
fn main(k: Hmac) -> Str {{ k.verify("hi".to_bytes(), unhex("{mac}")).to_str() + " " + k.verify("ho".to_bytes(), unhex("{mac}")).to_str() }}"#
    );
    assert_eq!(both(&src, || key(KEY.as_bytes(), true)), "true false");
}

/// A Stripe-style `stripe-signature: t=<unix seconds>,v1=<hex MAC of "t.body">` checked in kio.
const STRIPE: &str = r#"
fn digit(c: Str) -> Int { "0123456789abcdef".find(c).unwrap_or(0) }
fn unhex(s: Str) -> Bytes {
  let cs = s.chars();
  let mut b = "".to_bytes();
  let mut i = 0;
  while i + 1 < cs.len() { b.push(digit(cs[i]) * 16 + digit(cs[i + 1])); i += 2; }
  b
}

/// Whether `header` holds a `v1` signature of `body` at its `t`.
fn signed(k: Hmac, header: Str, body: Bytes) -> Bool {
  let mut t = "";
  let mut sigs: List<Str> = [];
  for part in header.split(",") {
    if let Some(v) = part.strip_prefix("t=") { t = v; }
    if let Some(v) = part.strip_prefix("v1=") { sigs.push(v); }
  }
  let mut msg = (t + ".").to_bytes();
  msg.extend(body);
  let mut ok = false;
  for s in sigs { if k.verify(msg, unhex(s)) { ok = true; } }
  t != "" && ok
}

fn main(req: Incoming, res: Outgoing, k: Hmac) -> Result<(), WebErr> {
  let names = req.header_names();
  let values = req.header_values();
  let mut header = "";
  for i in 0..names.len() { if names[i] == "stripe-signature" { header = values[i]; } }
  if signed(k, header, req.body()) {
    res.send(200, [], [], "ok".to_bytes())
  } else {
    res.send(400, [], [], "bad signature".to_bytes())
  }
}
"#;

fn kio(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kio"));
    c.args(args).env_remove("H22_KEY").env_remove("H22_EMPTY");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

/// No byte of either key in `text`, nor its hex.
fn assert_clean(what: &str, text: &str) {
    for k in [KEY, FILE_KEY] {
        assert!(
            !text.contains(k) && !text.contains(&hex(k.as_bytes())),
            "{what} holds a key:\n{text}"
        );
    }
}

struct Server {
    child: Child,
    port: u16,
    stderr: Arc<Mutex<String>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(app: &str, extra: &[&str], env: &[(&str, &str)]) -> Server {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kio"));
    c.args(["run", app, "--listen", "127.0.0.1:0"])
        .args(extra)
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("listening on http://") {
        line.clear();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "kio exited before listening");
    }
    let port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    let seen = Arc::new(Mutex::new(line));
    let rest = seen.clone();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        rest.lock().unwrap().push_str(&s);
    });
    Server {
        child,
        port,
        stderr: seen,
    }
}

/// One POST with a `stripe-signature` header: the raw response.
fn post(port: u16, sig: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "POST /hook HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\nstripe-signature: {sig}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    raw
}

/// The Stripe header, checked by a kio server whose every request uses the one key read at
/// start; the audit log has each call's length and result, and no key.
#[test]
fn a_stripe_style_webhook_is_verified_under_listen() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "hook.kio", STRIPE);
    let audit = t.path().join("audit.log");
    let server = start(
        &app,
        &["--hmac", "k=env:H22_KEY:verify", "--audit", audit.to_str().unwrap()],
        &[("H22_KEY", KEY)],
    );
    let body = r#"{"id":"evt_1","type":"payment_intent.succeeded"}"#;
    let mac = hex(&reference(KEY.as_bytes(), format!("1700000000.{body}").as_bytes()));
    let good = format!("t=1700000000,v1=0000,v1={mac}");
    for _ in 0..3 {
        assert!(post(server.port, &good, body).contains("\r\n\r\nok"));
    }
    let tampered = post(server.port, &good, &body.replace("evt_1", "evt_2"));
    assert!(tampered.starts_with("HTTP/1.1 400"), "{tampered}");
    let other_t = post(server.port, &good.replace("t=1700000000", "t=1700000001"), body);
    assert!(other_t.starts_with("HTTP/1.1 400"), "{other_t}");
    let other_key = hex(&reference(FILE_KEY.as_bytes(), format!("1700000000.{body}").as_bytes()));
    let forged = post(server.port, &format!("t=1700000000,v1={other_key}"), body);
    assert!(forged.starts_with("HTTP/1.1 400"), "{forged}");
    let stderr = server.stderr.clone();
    drop(server);
    let log = std::fs::read_to_string(&audit).unwrap();
    assert_clean("the audit log", &log);
    assert_clean("stderr", &stderr.lock().unwrap());
    assert!(log.contains(r#""name":"k","kind":"Hmac","target":"verify""#), "{log}");
    let n = 11 + body.len();
    assert!(
        log.contains(&format!(
            r#""kind":"Hmac","method":"verify","arg":"verify","bytes":{},"result":"true","len":{n}"#,
            n + 32
        )),
        "{log}"
    );
    assert!(log.contains(r#""method":"verify","arg":"verify","bytes":"#) && log.contains(r#""result":"false""#));
}

const SIGNER: &str = r#"fn main(k: Hmac) -> Str {
  match k.sign("hello".to_bytes()) { Ok(m) => m.to_hex(), Err(HmacErr::Denied) => "denied" }
}"#;

#[test]
fn flags_grant_from_env_and_file_and_the_report_names_sign() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "sign.kio", SIGNER);
    let file = write(t.path(), "key", &format!("{FILE_KEY}\n"));
    let audit = t.path().join("audit.log");
    let out = kio(
        &[
            "run",
            &prog,
            "--hmac",
            "k=env:H22_KEY",
            "--audit",
            audit.to_str().unwrap(),
        ],
        &[("H22_KEY", KEY)],
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.trim(), hex(&reference(KEY.as_bytes(), b"hello")));
    let log = std::fs::read_to_string(&audit).unwrap();
    assert_clean("the audit log", &log);
    assert!(log.contains(r#""kind":"Hmac","target":"sign,verify""#), "{log}");
    assert!(
        log.contains(r#""method":"sign","arg":"sign","bytes":5,"result":"ok","len":5"#),
        "{log}"
    );
    // A file's one trailing newline is not part of the key.
    let out = kio(&["run", &prog, "--hmac", &format!("k=file:{file}")], &[]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.trim(), hex(&reference(FILE_KEY.as_bytes(), b"hello")));
    // `:verify` on a file: sign is refused, and the audit log says so.
    let out = kio(
        &[
            "run",
            &prog,
            "--hmac",
            &format!("k=file:{file}:verify"),
            "--audit",
            audit.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "denied");
    let log = std::fs::read_to_string(&audit).unwrap();
    assert!(
        log.contains(r#""method":"sign","arg":"sign","bytes":5,"result":"denied","len":5"#),
        "{log}"
    );
    // The report names the methods; adding `sign` is new authority.
    let out = kio(&["check", &prog], &[]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "main : k sign");
    let base = write(
        t.path(),
        "base.kio",
        r#"fn main(k: Hmac) -> Bool { k.verify("hello".to_bytes(), "".to_bytes()) }"#,
    );
    let out = kio(&["check", &prog, "--base", &base], &[]);
    let diff = String::from_utf8(out.stdout).unwrap();
    assert!(
        diff.contains("+ main: k sign") && diff.contains("- main: k verify"),
        "{diff}"
    );
    // Ungranted: the hint names the flag.
    let out = kio(&["run", &prog], &[]);
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("no grant for `k: Hmac`"), "{err}");
    assert!(
        err.contains("grant it: `--hmac k=env:VAR` or `--hmac k=file:PATH` (add `:verify` to refuse `sign`)"),
        "{err}"
    );
}

#[test]
fn bad_keys_are_refused_at_start_and_never_shown() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "sign.kio", SIGNER);
    let granted = t.path().join("granted");
    std::fs::create_dir(&granted).unwrap();
    let inside = write(&granted, "key", FILE_KEY);
    let blank = write(t.path(), "blank", " \n");
    let huge = write(t.path(), "huge", &format!("{FILE_KEY}{}", "k".repeat(9000)));
    let audit = write(t.path(), "audit.log", "");
    let logf = write(t.path(), "app.log", "");
    let cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec!["k=env:H22_KEY".into()],
            "--hmac k: `H22_KEY` is not set\n  hint: export H22_KEY",
        ),
        (
            vec!["k=env:H22_EMPTY".into()],
            "--hmac k: env:H22_EMPTY is empty or only whitespace\n  hint: set it to the key",
        ),
        (
            vec![format!("k=file:{blank}")],
            "is empty or only whitespace\n  hint: set it to the key",
        ),
        (vec![format!("k=file:{huge}")], "the key is over 8192 bytes\n  hint:"),
        (
            vec![format!("k=file:{}", t.path().display())],
            "not a regular file\n  hint: the file holds the secret alone",
        ),
        (vec![format!("k=file:{}/missing", t.path().display())], "cannot read"),
        (
            vec![
                format!("k=file:{inside}"),
                "--dir".into(),
                format!("d={}", granted.display()),
            ],
            "the program could read it\n  hint: keep the key outside every granted directory",
        ),
        (
            vec![format!("k=file:{audit}"), "--audit".into(), audit.clone()],
            "is the `--audit` file\n  hint: keep the key in a file of its own",
        ),
        (
            vec![format!("k=file:{logf}"), "--log".into(), format!("l={logf}")],
            "is the `--log l` file\n  hint:",
        ),
        (
            vec![format!("k={KEY}")],
            "--hmac k: expected name=env:VAR or name=file:PATH",
        ),
        (vec![format!("={KEY}")], "--hmac : expected name=env:VAR"),
        (vec!["k=env:".into()], "expected name=env:VAR"),
        (vec!["k=file:".into()], "expected name=env:VAR"),
    ];
    for (args, want) in cases {
        let mut all = vec!["run".to_string(), prog.clone(), "--hmac".into()];
        all.extend(args);
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        let out = kio(&all, &[("H22_EMPTY", "  ")]);
        let err = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(2), "{all:?}: {err}");
        assert!(err.contains(want), "{all:?}: {err}");
        assert_clean("an error", &err);
    }
}
