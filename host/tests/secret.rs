//! H33: `Secret` is a value the host read at start that the program can compare, never read.
//! Interpreted and compiled, through `kio run`, `kio test` and under `--listen`.
use kio_host::{Interp, Kio, Limits, RunResult, SecretValue, Sources};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "tok_h33-env-7f3a1c92-never-shown";
const FILE_TOKEN: &str = "tok_h33-file-0b9e4d57-never-shown";

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

fn secret(value: &str) -> Sources {
    Sources {
        secrets: vec![("t".into(), SecretValue::new(value.into()).unwrap())],
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

/// Compares the value, a wrong value of the same length, the empty string, a longer and a
/// shorter one, and a value differing in its last byte only.
fn program(value: &str) -> String {
    let same_len: String = value.chars().map(|c| if c == '#' { '$' } else { '#' }).collect();
    let shorter: String = value.chars().take(value.chars().count() - 1).collect();
    let last = format!("{shorter}!");
    format!(
        r#"fn main(t: Secret) -> Str {{
  let checks = [t.matches("{value}"), t.matches("{same_len}"), t.matches(""), t.matches("{value}x"),
    t.matches("{shorter}"), t.matches("{last}")];
  checks.map(|c| c.to_str()).join(",")
}}
"#
    )
}

#[test]
fn match_mismatch_empty_and_other_lengths_on_both_paths() {
    for value in [TOKEN, "a", "pässwörd 密码", "x".repeat(8192).as_str()] {
        assert_eq!(
            both(&program(value), || secret(value)),
            "true,false,false,false,false,false"
        );
    }
    // The value is never readable: the type has one method.
    let src = r#"fn main(t: Secret) -> Str { t.to_str() }"#;
    let e = shown(run(src, secret(TOKEN), false, None));
    assert!(e.starts_with("error: "), "{e}");
    assert!(!e.contains(TOKEN), "{e}");
}

/// Each call's line has the method, the presented length (as `bytes`) and the result; never the
/// value or the presented string.
#[test]
fn audit_lines_hold_the_result_not_the_argument() {
    for compiled in [true, false] {
        let audit = Buf::default();
        let src = format!(
            r#"fn main(t: Secret) -> Str {{ t.matches("{TOKEN}").to_str() + " " + t.matches("guess-9c2e").to_str() }}"#
        );
        let v = shown(run(&src, secret(TOKEN), compiled, Some(audit.clone())));
        assert_eq!(v, "true false");
        let text = audit.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(
            lines[0].contains(r#""name":"t","kind":"Secret","target":"matches""#),
            "{text}"
        );
        assert!(
            lines[1].contains(&format!(
                r#""cap":"t","kind":"Secret","method":"matches","arg":"matches","bytes":{},"result":"true""#,
                TOKEN.len()
            )),
            "{text}"
        );
        assert!(
            lines[2].contains(r#""method":"matches","arg":"matches","bytes":10,"result":"false""#),
            "{text}"
        );
        assert!(
            !text.contains("guess-9c2e"),
            "the audit log holds the argument:\n{text}"
        );
        assert_clean("the audit log", &text);
    }
}

fn kio(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kio"));
    c.args(args).env_remove("H33_TOKEN").env_remove("H33_EMPTY");
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

/// No byte of either value in `text`.
fn assert_clean(what: &str, text: &str) {
    for v in [TOKEN, FILE_TOKEN] {
        assert!(!text.contains(v), "{what} holds the secret:\n{text}");
    }
}

const CHECKER: &str = r#"fn main(t: Secret) -> Str {
  if t.matches("tok_h33-env-7f3a1c92-never-shown") { "env" }
  else if t.matches("tok_h33-file-0b9e4d57-never-shown") { "file" }
  else { "neither" }
}"#;

#[test]
fn flags_grant_from_env_and_file_and_the_report_names_matches() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "check.kio", CHECKER);
    let file = write(t.path(), "token", &format!("{FILE_TOKEN}\n"));
    let audit = t.path().join("audit.log");
    let out = kio(
        &[
            "run",
            &prog,
            "--secret",
            "t=env:H33_TOKEN",
            "--audit",
            audit.to_str().unwrap(),
        ],
        &[("H33_TOKEN", TOKEN)],
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "env");
    let log = std::fs::read_to_string(&audit).unwrap();
    assert_clean("the audit log", &log);
    assert!(log.contains(r#""kind":"Secret","target":"matches""#), "{log}");
    assert!(
        log.contains(r#""method":"matches","arg":"matches","bytes":32,"result":"true""#),
        "{log}"
    );
    // A file's one trailing newline is not part of the value.
    let out = kio(&["run", &prog, "--secret", &format!("t=file:{file}")], &[]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "file");
    // `--json` carries the value no more than the text does.
    let out = kio(
        &["run", &prog, "--json", "--secret", "t=env:H33_TOKEN"],
        &[("H33_TOKEN", TOKEN)],
    );
    let json = String::from_utf8(out.stdout).unwrap();
    assert!(json.contains(r#""value":"env""#), "{json}");
    assert_clean("--json", &json);
    // The report names the one method; `kio check --grants` binds the flag without reading it.
    let out = kio(&["check", &prog], &[]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "main : t matches");
    let g = write(t.path(), "t.grants", "--secret t=env:H33_TOKEN\n");
    let out = kio(&["check", &prog, "--grants", &g], &[]);
    assert_eq!(
        (out.status.code(), String::from_utf8(out.stdout).unwrap().trim()),
        (Some(0), "main : t matches")
    );
    // Ungranted: the hint names the flag.
    let out = kio(&["run", &prog], &[]);
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("no grant for `t: Secret`"), "{err}");
    assert!(
        err.contains("grant it: `--secret t=env:VAR` or `--secret t=file:PATH`"),
        "{err}"
    );
}

#[test]
fn bad_values_are_refused_at_start_and_never_shown() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "check.kio", CHECKER);
    let granted = t.path().join("granted");
    std::fs::create_dir(&granted).unwrap();
    let inside = write(&granted, "token", FILE_TOKEN);
    let blank = write(t.path(), "blank", " \n");
    let huge = write(t.path(), "huge", &format!("{FILE_TOKEN}{}", "v".repeat(9000)));
    let cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec!["t=env:H33_TOKEN".into()],
            "--secret t: `H33_TOKEN` is not set\n  hint: export H33_TOKEN",
        ),
        (
            vec!["t=env:H33_EMPTY".into()],
            "--secret t: env:H33_EMPTY is empty or only whitespace\n  hint: set it to the secret",
        ),
        (
            vec![format!("t=file:{blank}")],
            "is empty or only whitespace\n  hint: set it to the secret",
        ),
        (vec![format!("t=file:{huge}")], "the value is over 8192 bytes\n  hint:"),
        (vec![format!("t=file:{}/missing", t.path().display())], "cannot read"),
        (
            vec![
                format!("t=file:{inside}"),
                "--dir".into(),
                format!("d={}", granted.display()),
            ],
            "the program could read it\n  hint: keep the key outside every granted directory",
        ),
        // A value given inline by mistake: the error shows the name only.
        (
            vec![format!("t={FILE_TOKEN}")],
            "--secret t: expected name=env:VAR or name=file:PATH",
        ),
    ];
    for (flags, want) in cases {
        let mut args = vec!["run", &prog, "--secret"];
        args.extend(flags.iter().map(String::as_str));
        let out = kio(&args, &[("H33_EMPTY", "  ")]);
        let err = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(2), "{flags:?}: {err}");
        assert!(err.contains(want), "{flags:?}:\n{err}");
        assert!(out.stdout.is_empty());
        assert_clean(&format!("stderr for {flags:?}"), &err);
        let mut args = vec!["run", &prog, "--json", "--secret"];
        args.extend(flags.iter().map(String::as_str));
        let out = kio(&args, &[("H33_EMPTY", "  ")]);
        assert_clean("--json", &String::from_utf8(out.stdout).unwrap());
    }
}

#[test]
fn a_test_needs_the_grant_from_a_grants_file() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(
        t.path(),
        "app.kio",
        &format!(
            r#"#[test]
fn checks(tok: Secret) {{ assert(tok.matches("{TOKEN}"), "matches"); assert(!tok.matches("other"), "other"); }}
"#
        ),
    );
    let out = kio(&["test", &prog], &[]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("test `checks` needs `tok: Secret`")
            && text.contains("hint: name it in a --grants file: `--secret tok=..`"),
        "{text}"
    );
    let g = write(t.path(), "t.grants", "--secret tok=env:H33_TOKEN\n");
    let out = kio(&["test", &prog, "--grants", &g], &[("H33_TOKEN", TOKEN)]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");
    // An unset variable in the grants file is one refusal, with no value in it.
    let out = kio(&["test", &prog, "--grants", &g], &[]);
    let (code, text) = (out.status.code(), String::from_utf8(out.stdout).unwrap());
    assert!(
        text.contains("--secret tok: `H33_TOKEN` is not set"),
        "{code:?}: {text}"
    );
}

/// A bearer-token server: every request compares against the one value read at start.
const BEARER: &str = r#"fn main(req: Incoming, res: Outgoing, t: Secret) -> Result<(), WebErr> {
  let names = req.header_names();
  let values = req.header_values();
  let mut presented = "";
  for i in 0..names.len() {
    if names[i] == "authorization" {
      if let Some(v) = values[i].strip_prefix("Bearer ") { presented = v; }
    }
  }
  if t.matches(presented) { res.send(200, [], [], "ok".to_bytes()) } else { res.send(401, [], [], "no".to_bytes()) }
}"#;

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

fn get(port: u16, auth: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\nauthorization: {auth}\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    raw
}

#[test]
fn a_bearer_token_is_checked_under_listen() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "app.kio", BEARER);
    let audit = t.path().join("audit.log");
    let server = start(
        &app,
        &["--secret", "t=env:H33_TOKEN", "--audit", audit.to_str().unwrap()],
        &[("H33_TOKEN", TOKEN)],
    );
    for _ in 0..3 {
        assert!(get(server.port, &format!("Bearer {TOKEN}")).starts_with("HTTP/1.1 200"));
    }
    assert!(get(server.port, &format!("Bearer {FILE_TOKEN}")).starts_with("HTTP/1.1 401"));
    assert!(get(server.port, &format!("Basic {TOKEN}")).starts_with("HTTP/1.1 401"));
    assert!(get(server.port, "Bearer ").starts_with("HTTP/1.1 401"));
    let stderr = server.stderr.clone();
    drop(server);
    let log = std::fs::read_to_string(&audit).unwrap();
    assert_clean("the audit log", &log);
    assert_clean("stderr", &stderr.lock().unwrap());
    assert!(
        log.contains(r#""name":"t","kind":"Secret","target":"matches""#),
        "{log}"
    );
    assert!(
        log.contains(&format!(
            r#""kind":"Secret","method":"matches","arg":"matches","bytes":{},"result":"true""#,
            TOKEN.len()
        )),
        "{log}"
    );
    assert!(
        log.contains(r#""method":"matches","arg":"matches","bytes":0,"result":"false""#),
        "{log}"
    );
}
