//! S1: `--http-header name=Header:env:VAR|file:PATH` adds a header the program never holds to
//! every request on the grant. The value must reach the upstream and nothing else.

mod upstream;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

const SECRET: &str = "s1-env-7f3a9c2e-never-shown";
const FILE_SECRET: &str = "s1-file-b81d04aa-never-shown";

/// Every way a program can make a request on `up`, then three requests that name the injected
/// header themselves (each refused). Returns what it saw, and writes it to `log` too.
const PROGRAM: &str = r#"fn show(r: Result<HttpResponse, HttpErr>) -> Str {
    match r {
        Ok(r) => r.status.to_str() + " " + r.headers.join("|") + " " + r.body.to_str(),
        Err(HttpErr::Invalid) => "invalid",
        Err(_) => "error",
    }
}

fn req(path: Str, headers: List<Str>) -> HttpRequest {
    HttpRequest { method: "GET", path: path, headers: headers, body: "".to_bytes(), timeout_ms: 0 }
}

fn main(up: Http, log: Log) -> Str {
    let mut out: List<Str> = [];
    out.push(match up.get("/get") { Ok(b) => b.to_str(), Err(_) => "error" });
    out.push(match up.get("/redirect") { Ok(b) => b.to_str(), Err(_) => "error" });
    out.push(show(up.send("POST", "/post", ["x-trace: 1"], "hi".to_bytes())));
    for r in up.send_all([req("/all/0", []), req("/all/1", ["x-trace: 2"])]) { out.push(show(r)); }
    out.push(show(up.send("GET", "/mine", ["Authorization: Bearer mine"], "".to_bytes())));
    out.push(show(up.send("GET", "/mine", ["X-API-KEY: mine"], "".to_bytes())));
    for r in up.send_all([req("/mine-all", ["authorization: mine"])]) { out.push(show(r)); }
    let text = out.join("\n");
    match log.write(text) { Ok(_) => text, Err(_) => "log full" }
}
"#;

fn kio(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kio"));
    c.arg("run").args(args).env_remove("S1_SECRET").env_remove("S1_EMPTY");
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

/// No byte of either secret in `text`.
fn assert_clean(what: &str, text: &str) {
    for s in [SECRET, FILE_SECRET] {
        assert!(!text.contains(s), "{what} holds a secret:\n{text}");
    }
}

/// The value of header `name` (any case) in each request the upstream got for `path`.
fn received(up: &upstream::Upstream, path: &str, name: &str) -> Vec<Vec<String>> {
    let heads = up.heads.lock().unwrap();
    (heads.iter().filter(|(p, _)| p == path))
        .map(|(_, lines)| {
            (lines.iter())
                .filter_map(|l| l.split_once(": "))
                .filter(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.to_string())
                .collect()
        })
        .collect()
}

#[test]
fn the_header_reaches_every_request_and_hop_and_nothing_else() {
    let up = upstream::start();
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "app.kio", PROGRAM);
    let key = write(t.path(), "key", &format!("{FILE_SECRET}\n"));
    let audit = t.path().join("audit.log");
    let log = t.path().join("out.log");
    let out = kio(
        &[
            &prog,
            "--http",
            &format!("up={}", up.grant),
            "--http-methods",
            "up=POST",
            "--http-header",
            "up=Authorization:env:S1_SECRET",
            "--http-header",
            &format!("up=X-Api-Key:file:{key}"),
            "--log",
            &format!("log={}", log.display()),
            "--audit",
            audit.to_str().unwrap(),
        ],
        &[("S1_SECRET", &format!("Bearer {SECRET}"))],
    );
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    // get, a redirect and its hop, send (POST), and each request of send_all.
    for path in ["/get", "/redirect", "/landed", "/post", "/all/0", "/all/1"] {
        assert_eq!(
            received(&up, path, "authorization"),
            vec![vec![format!("Bearer {SECRET}")]],
            "{path}"
        );
        assert_eq!(
            received(&up, path, "x-api-key"),
            vec![vec![FILE_SECRET.to_string()]],
            "{path}"
        );
    }
    assert_eq!(received(&up, "/post", "x-trace"), vec![vec!["1".to_string()]]);
    // A program naming an injected header, any case, sends nothing.
    assert!(received(&up, "/mine", "authorization").is_empty());
    assert!(received(&up, "/mine-all", "authorization").is_empty());
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "GET /get ");
    assert_eq!(lines[1], "GET /landed ");
    assert!(
        lines[2].starts_with("200 ") && lines[2].ends_with(" POST /post hi"),
        "{stdout}"
    );
    assert_eq!(&lines[5..], ["invalid", "invalid", "invalid"]);

    let audit = std::fs::read_to_string(&audit).unwrap();
    let parsed: Vec<serde_json::Value> = audit.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let grant = (parsed[0]["grants"].as_array().unwrap().iter())
        .find(|g| g["name"] == "up")
        .unwrap();
    assert_eq!(
        grant["target"],
        format!("{} methods=GET,POST injects=authorization,x-api-key", up.grant)
    );
    let calls: Vec<&serde_json::Value> = parsed.iter().filter(|l| l["kind"] == "Http").collect();
    // get, get, send, 2 + 1 batch requests, send, send, 1 + 1 batch.
    assert_eq!(calls.len(), 10, "{audit}");
    // Each request sent lists the injected names; one refused before sending does not.
    for c in calls.iter().filter(|c| c["method"] != "send_all") {
        let want = match c["result"] == "invalid" {
            true => serde_json::Value::Null,
            false => serde_json::json!(["authorization", "x-api-key"]),
        };
        assert_eq!(c["injected"], want, "{c}");
    }
    let refused: Vec<_> = calls.iter().filter(|c| c["result"] == "invalid").collect();
    assert_eq!(refused.len(), 3, "{audit}");

    assert_clean("stdout", &stdout);
    assert_clean("stderr", &stderr);
    assert_clean("the audit log", &audit);
    assert_clean("the log", &std::fs::read_to_string(&log).unwrap());
}

/// Each refusal at start says why and how to fix it, and never prints the value.
#[test]
fn bad_flags_are_refused_with_a_hint() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "app.kio", "fn main(up: Http) -> Int { 1 }");
    let empty = write(t.path(), "empty", "\n");
    let two_lines = write(t.path(), "two", &format!("{SECRET}\n{FILE_SECRET}\n"));
    let good = write(t.path(), "good", FILE_SECRET);
    let long = write(t.path(), "long", &"k".repeat(9000));
    let fifo = t.path().join("fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let fifo = fifo.to_str().unwrap();
    let conf = t.path().join("conf");
    std::fs::create_dir(&conf).unwrap();
    let in_dir = write(&conf, "key", FILE_SECRET);
    let dir_grant = format!("d={}", conf.display());
    let inline = format!("up=Authorization:Bearer {SECRET}");
    let inline_no_colon = format!("up=Bearer{SECRET}");
    let no_eq = format!("up_{SECRET}");
    let linked = write(t.path(), "linked", FILE_SECRET);
    std::fs::hard_link(&linked, conf.join("linked")).unwrap();
    let loopback = "up=http://127.0.0.1:1";
    let cases: &[(&[&str], &str)] = &[
        (
            &["--http", loopback, "--http-header", "up=Authorization:env:S1_SECRET"],
            "`S1_SECRET` is not set\n  hint: export S1_SECRET",
        ),
        (
            &["--http", loopback, "--http-header", "up=Authorization:env:S1_EMPTY"],
            "env:S1_EMPTY is empty or only whitespace\n  hint:",
        ),
        (
            &[
                "--http",
                loopback,
                "--http-header",
                &format!("up=Authorization:file:{empty}"),
            ],
            "is empty or only whitespace\n  hint:",
        ),
        (
            &[
                "--http",
                loopback,
                "--http-header",
                "up=Authorization:file:/no/such/file",
            ],
            "cannot read /no/such/file",
        ),
        (
            &[
                "--http",
                loopback,
                "--http-header",
                &format!("up=Authorization:file:{two_lines}"),
            ],
            "control character",
        ),
        (
            &["--http", loopback, "--http-header", "up=Authorization:vault:x"],
            "expected name=Header:env:VAR",
        ),
        (
            &[
                "--http",
                loopback,
                "--http-header",
                &format!("other=Authorization:file:{good}"),
            ],
            "no `--http other=host` grant\n  hint:",
        ),
        (
            &["--http", loopback, "--http-header", &format!("up=Bad Name:file:{good}")],
            "is not a header name\n  hint:",
        ),
        (
            &["--http", loopback, "--http-header", &format!("up=Host:file:{good}")],
            "`Host` is set by the host\n  hint:",
        ),
        (
            &[
                "--http",
                loopback,
                "--http-header",
                &format!("up=Authorization:file:{good}"),
                "--http-header",
                &format!("up=authorization:file:{good}"),
            ],
            "injected twice on this grant\n  hint:",
        ),
        (
            &[
                "--http",
                "up=http://example.com",
                "--http-header",
                &format!("up=Authorization:file:{good}"),
            ],
            "`http://example.com:80` would carry the header in clear\n  hint: grant `https://example.com`",
        ),
        // A name never connects to loopback (only an IP literal does), so it would only carry
        // the header if it resolved to a public address.
        (
            &["--http", "up=http://localhost:1", "--http-header", &format!("up=Authorization:file:{good}")],
            "`http://localhost:1` would carry the header in clear\n  hint: grant `https://localhost`, or `http://127.0.0.1:PORT`",
        ),
        (&["--http", loopback, "--http-header", "up=Authorization:env:A=B"], "expected name=Header:env:VAR"),
        (&["--http", loopback, "--http-header", &format!("up=Authorization:file:{long}")], "over 8192 bytes"),
        (&["--http", loopback, "--http-header", &format!("up=Authorization:file:{fifo}")], "not a regular file"),
        (&["--http", loopback, "--http-header", &format!("up=Authorization:file:{}", t.path().display())], "not a regular file"),
        (
            &["--http", loopback, "--dir", &dir_grant, "--http-header", &format!("up=Authorization:file:{in_dir}")],
            "the program could read it\n  hint: keep the key outside every granted directory",
        ),
        (
            &["--http", loopback, "--audit", &good, "--http-header", &format!("up=Authorization:file:{good}")],
            "is the `--audit` file\n  hint: keep the key in a file of its own",
        ),
        (
            &["--http", loopback, "--log", &format!("l={good}"), "--http-header", &format!("up=Authorization:file:{good}")],
            "is the `--log l` file\n  hint:",
        ),
        // A value given inline by mistake is not echoed: only `name=Header`.
        (&["--http", loopback, "--http-header", &inline], "--http-header up=Authorization: expected"),
        (&["--http", loopback, "--http-header", &inline_no_colon], "--http-header up: expected"),
        // Without `=` the whole spec could be the value: nothing of it is echoed.
        (&["--http", loopback, "--http-header", &no_eq], "--http-header: expected"),
        (
            &["--http", loopback, "--http-header", &format!("up=Authorization:file:{linked}")],
            "has other hard links; the program could read one\n  hint: give kio a key file of its own",
        ),
    ];
    for (flags, want) in cases {
        let mut args = vec![prog.as_str()];
        args.extend_from_slice(flags);
        let out = kio(&args, &[("S1_EMPTY", " \t ")]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{flags:?}: {stderr}");
        assert!(stderr.contains(want), "{flags:?}: {stderr}");
        assert_clean("stderr", &stderr);
    }
    // A loopback IP literal over http and any https origin are fine.
    for grant in ["up=http://127.0.0.1:1", "up=http://[::1]:1", "up=example.com"] {
        let out = kio(
            &[
                &prog,
                "--http",
                grant,
                "--http-header",
                &format!("up=Authorization:file:{good}"),
            ],
            &[],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{grant}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The grant's `Debug` (as a panic or error might print it) shows no value.
#[test]
fn a_grant_prints_no_value() {
    let mut o = kio_host::Origin::parse("https://example.com").unwrap();
    o.inject("Authorization", SECRET.to_string()).unwrap();
    let shown = format!("{o:?} {o:#?}");
    assert!(shown.contains("<redacted>"), "{shown}");
    assert_clean("Debug", &shown);
}

/// Under `--listen` every run's requests carry the header; the client, the audit log and stderr
/// never see it.
#[test]
fn under_listen_every_run_gets_the_header() {
    const APP: &str = r#"fn main(req: Incoming, res: Outgoing, up: Http) -> Result<(), WebErr> {
        let r = up.send("POST", req.target(), ["x-trace: 1"], req.body());
        let mine = up.send("GET", "/mine", ["authorization: x"], "".to_bytes());
        let out = match r { Ok(r) => r.status.to_str() + " " + r.headers.join("|") + " " + r.body.to_str(), Err(_) => "error" };
        let out = out + match mine { Ok(_) => " sent", Err(HttpErr::Invalid) => " invalid", Err(_) => " error" };
        res.send(200, [], [], out.to_bytes())
    }"#;
    let up = upstream::start();
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "app.kio", APP);
    let key = write(t.path(), "key", &format!("Bearer {FILE_SECRET}\n"));
    let audit = t.path().join("audit.log");
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args([
            "run",
            &app,
            "--listen",
            "127.0.0.1:0",
            "--http",
            &format!("up={}", up.grant),
        ])
        .args([
            "--http-methods",
            "up=POST",
            "--http-header",
            &format!("up=Authorization:file:{key}"),
        ])
        .args(["--audit", audit.to_str().unwrap()])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("listening on http://") {
        line.clear();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "kio exited before listening");
    }
    let port: u16 = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    let rest = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    for i in 0..3 {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = format!("run {i}");
        write!(
            s,
            "POST /api/{i} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let raw = String::from_utf8_lossy(&raw);
        assert!(raw.contains(&format!("POST /api/{i} run {i} invalid")), "{raw}");
        assert_clean("the response", &raw);
        assert_eq!(
            received(&up, &format!("/api/{i}"), "authorization"),
            vec![vec![format!("Bearer {FILE_SECRET}")]]
        );
    }
    assert!(received(&up, "/mine", "authorization").is_empty());
    let _ = child.kill();
    let _ = child.wait();
    assert_clean("stderr", &rest.join().unwrap());
    let audit = std::fs::read_to_string(&audit).unwrap();
    assert_clean("the audit log", &audit);
    let calls: Vec<serde_json::Value> = (audit.lines())
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|l: &serde_json::Value| l["kind"] == "Http")
        .collect();
    assert_eq!(calls.len(), 6, "{audit}");
    for c in &calls {
        let sent = c["result"] != "invalid";
        assert_eq!(c["injected"] == serde_json::json!(["authorization"]), sent, "{c}");
    }
}
