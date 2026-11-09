//! H13, H14, H15: the `Rng`, `Clock` and `Log` capabilities, interpreted and compiled.
use kio_host::{ClockSpec, Interp, Kio, Limits, LogSink, RngSpec, RunResult, Sources};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// A log, or an audit log, in memory.
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

fn rng(spec: RngSpec) -> Sources {
    Sources {
        rngs: vec![("r".into(), spec)],
        ..Sources::default()
    }
}

fn clock(spec: ClockSpec) -> Sources {
    Sources {
        clocks: vec![("c".into(), spec)],
        ..Sources::default()
    }
}

fn log(buf: &Buf) -> Sources {
    Sources {
        logs: vec![("log".into(), LogSink::new(Box::new(buf.clone()), "mem"))],
        ..Sources::default()
    }
}

/// `src` run with `sources`, compiled or interpreted, with an audit log in `audit`.
fn run(src: &str, sources: Sources, compiled: bool, limits: Limits, audit: Option<Buf>) -> RunResult {
    if compiled {
        let mut k = Kio::new(limits).unwrap();
        if let Some(a) = audit {
            k.audit(a, vec![]);
        }
        k.grant_sources(sources);
        k.run(src, vec![]).unwrap()
    } else {
        let mut k = Interp::new(limits).unwrap();
        if let Some(a) = audit {
            k.audit(a, vec![]);
        }
        k.grant_sources(sources);
        k.run(src, vec![]).unwrap()
    }
}

/// `main`'s value, or `error: <message> @<start>..<end>`.
fn shown(r: RunResult) -> String {
    match r {
        Ok(o) => o.value,
        Err(d) => format!("error: {} @{}..{}", d[0].message, d[0].start, d[0].end),
    }
}

/// `src`'s value from both paths, which must agree.
fn both(src: &str, sources: impl Fn() -> Sources) -> String {
    let c = shown(run(src, sources(), true, Limits::default(), None));
    let i = shown(run(src, sources(), false, Limits::default(), None));
    assert_eq!(c, i, "compiled and interpreted disagree on:\n{src}");
    c
}

// ---- Rng

#[test]
fn rng_ranges() {
    let src = r#"fn main(r: Rng) -> Str {
      let a = r.bytes(0).len();
      let b = r.bytes(65536).len();
      let mut ok = true;
      for i in 0..500 { let x = r.int(-5, 5); if x < -5 || x >= 5 { ok = false; } }
      for i in 0..50 { if r.int(0, 1) != 0 { ok = false; } }
      for i in 0..50 { if r.int(9223372036854775806, 9223372036854775807) != 9223372036854775806 { ok = false; } }
      let min = -9223372036854775807 - 1;
      let full = r.int(min, 9223372036854775807);
      let neg = r.int(min, min + 1);
      a.to_str() + " " + b.to_str() + " " + ok.to_str() + " " + (full < 9223372036854775807).to_str() + " " + (neg == min).to_str()
    }"#;
    for spec in [RngSpec::Os, RngSpec::Seed(3)] {
        assert_eq!(both(src, || rng(spec)), "0 65536 true true true");
    }
}

/// A count outside 0 to 65,536 or an empty range is a runtime error at the call, the same message
/// and span from both paths.
#[test]
fn rng_argument_errors() {
    // (the expression, the call the error spans, the message)
    let cases = [
        (
            "r.bytes(65537).len()",
            "r.bytes(65537)",
            "Rng.bytes(65537): the count must be 0 to 65536",
        ),
        (
            "r.bytes(0 - 1).len()",
            "r.bytes(0 - 1)",
            "Rng.bytes(-1): the count must be 0 to 65536",
        ),
        ("r.int(3, 3)", "r.int(3, 3)", "Rng.int(3, 3): lo must be less than hi"),
        (
            "r.int(5, -5)",
            "r.int(5, -5)",
            "Rng.int(5, -5): lo must be less than hi",
        ),
    ];
    for (expr, call, msg) in cases {
        let src = format!("fn main(r: Rng) -> Int {{ {expr} }}");
        let start = src.find(call).unwrap();
        let got = both(&src, || rng(RngSpec::Seed(1)));
        assert_eq!(got, format!("error: {msg} @{start}..{}", start + call.len()), "{src}");
    }
}

const DRAW: &str = r#"fn main(r: Rng) -> Str {
  r.bytes(16).to_hex() + " " + r.int(0, 1000000).to_str() + " " + r.int(-9223372036854775807 - 1, 9223372036854775807).to_str()
}"#;

#[test]
fn a_seed_replays_and_the_os_does_not() {
    let a = shown(run(DRAW, rng(RngSpec::Seed(1)), true, Limits::default(), None));
    let b = shown(run(DRAW, rng(RngSpec::Seed(1)), false, Limits::default(), None));
    let c = shown(run(DRAW, rng(RngSpec::Seed(1)), true, Limits::default(), None));
    assert_eq!(a, b);
    assert_eq!(a, c);
    assert_ne!(
        a,
        shown(run(DRAW, rng(RngSpec::Seed(2)), true, Limits::default(), None))
    );
    let x = shown(run(DRAW, rng(RngSpec::Os), true, Limits::default(), None));
    let y = shown(run(DRAW, rng(RngSpec::Os), true, Limits::default(), None));
    assert_ne!(x, y);
    assert_ne!(x, a);
}

/// Pearson's chi-square of `counts` against a uniform expectation.
fn chi_square(counts: &[u64]) -> f64 {
    let n: u64 = counts.iter().sum();
    let expected = n as f64 / counts.len() as f64;
    counts.iter().map(|&c| (c as f64 - expected).powi(2) / expected).sum()
}

/// 9,000 draws of `int(0, 10)` and 65,536 bytes look uniform: chi-square under 40 (9 degrees of
/// freedom; the 99.99% point is 33.7) and under 400 (255 degrees; 99.99% is about 345).
#[test]
fn draws_are_uniform() {
    let ints = r#"fn main(r: Rng) -> Str { let mut out: List<Str> = []; for i in 0..9000 { out.push(r.int(0, 10).to_str()); } out.join(",") }"#;
    let bytes = r#"fn main(r: Rng) -> Str { r.bytes(65536).to_hex() }"#;
    for spec in [RngSpec::Os, RngSpec::Seed(7)] {
        let drawn = shown(run(ints, rng(spec), true, Limits::default(), None));
        let mut bins = [0u64; 10];
        for d in drawn.split(',') {
            bins[d.parse::<usize>().unwrap()] += 1;
        }
        assert_eq!(bins.iter().sum::<u64>(), 9000);
        let x = chi_square(&bins);
        assert!(x < 40.0, "{spec:?}: chi-square {x} over {bins:?}");
        let hex = shown(run(bytes, rng(spec), true, Limits::default(), None));
        let mut bins = [0u64; 256];
        for i in (0..hex.len()).step_by(2) {
            bins[usize::from_str_radix(&hex[i..i + 2], 16).unwrap()] += 1;
        }
        let x = chi_square(&bins);
        assert!(x < 400.0, "{spec:?}: chi-square {x} over bytes");
    }
}

#[test]
fn rng_audit_lines_carry_the_count_or_range_never_the_values() {
    for compiled in [true, false] {
        let audit = Buf::default();
        let v = shown(run(
            DRAW,
            rng(RngSpec::Seed(1)),
            compiled,
            Limits::default(),
            Some(audit.clone()),
        ));
        let hex = v.split(' ').next().unwrap();
        let text = audit.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(
            lines[0].contains(r#""name":"r","kind":"Rng","target":"seed:1""#),
            "{}",
            lines[0]
        );
        assert!(lines[1]
            .contains(r#""cap":"r","kind":"Rng","method":"bytes","arg":"bytes","bytes":16,"result":"ok","n":16"#));
        assert!(lines[2].contains(r#""method":"int","arg":"int","bytes":0,"result":"ok","lo":0,"hi":1000000"#));
        assert!(lines[3].contains(r#""lo":-9223372036854775808,"hi":9223372036854775807"#));
        assert!(!text.contains(hex), "the audit log holds a drawn value");
    }
}

// ---- Clock

#[test]
fn clock_fixed_and_system() {
    let src = "fn main(c: Clock) -> Int { c.now_ms() }";
    assert_eq!(both(src, || clock(ClockSpec::Fixed(1794225600000))), "1794225600000");
    assert_eq!(both(src, || clock(ClockSpec::Fixed(-1))), "-1");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    for compiled in [true, false] {
        let v: i64 = shown(run(src, clock(ClockSpec::System), compiled, Limits::default(), None))
            .parse()
            .unwrap();
        assert!((v - now).abs() < 10_000, "{v} is not within 10 s of {now}");
    }
}

#[test]
fn clock_specs_parse() {
    assert_eq!(ClockSpec::parse(None), Ok(ClockSpec::System));
    assert_eq!(
        ClockSpec::parse(Some("fixed:1700000000000")),
        Ok(ClockSpec::Fixed(1700000000000))
    );
    assert_eq!(ClockSpec::parse(Some("fixed:-5")), Ok(ClockSpec::Fixed(-5)));
    assert_eq!(
        ClockSpec::parse(Some("fixed:2026-11-09T12:00:00Z")),
        Ok(ClockSpec::Fixed(1794225600000))
    );
    assert_eq!(
        ClockSpec::parse(Some("fixed:2000-02-29T23:59:59.123Z")),
        Ok(ClockSpec::Fixed(951868799123))
    );
    assert_eq!(
        ClockSpec::parse(Some("fixed:1969-12-31T23:59:59Z")),
        Ok(ClockSpec::Fixed(-1000))
    );
    assert_eq!(
        ClockSpec::parse(Some("fixed:2100-03-01T00:00:00Z")),
        Ok(ClockSpec::Fixed(4107542400000))
    );
    assert_eq!(
        ClockSpec::parse(Some("fixed:1970-01-01T00:00:00.5Z")),
        Ok(ClockSpec::Fixed(500))
    );
    for bad in [
        "now",
        "fixed:",
        "fixed:1.5",
        "fixed:2026-02-30T00:00:00Z",
        "fixed:2026-11-09T12:00:00",
        "fixed:2026-11-09T12:00:00+01:00",
        "fixed:2026-11-09 12:00:00Z",
        "fixed:2026-11-09T24:00:00Z",
        "fixed:2026-13-09T12:00:00Z",
        "fixed:2026-11-09T12:00:00.1234Z",
        "fixed:26-11-09T12:00:00Z",
    ] {
        let e = ClockSpec::parse(Some(bad)).unwrap_err();
        assert!(e.contains("expected `fixed:<ms>`"), "{bad}: {e}");
    }
    assert_eq!(RngSpec::parse(None), Ok(RngSpec::Os));
    assert_eq!(RngSpec::parse(Some("seed:42")), Ok(RngSpec::Seed(42)));
    for bad in ["seed:", "seed:-1", "os", "42", "seed:x"] {
        assert!(
            RngSpec::parse(Some(bad)).unwrap_err().contains("expected `seed:N`"),
            "{bad}"
        );
    }
}

#[test]
fn clock_audit_line_carries_the_value() {
    for compiled in [true, false] {
        let audit = Buf::default();
        let src = "fn main(c: Clock) -> Int { c.now_ms() }";
        shown(run(
            src,
            clock(ClockSpec::Fixed(1794225600000)),
            compiled,
            Limits::default(),
            Some(audit.clone()),
        ));
        let text = audit.text();
        assert!(
            text.contains(r#""name":"c","kind":"Clock","target":"fixed:1794225600000""#),
            "{text}"
        );
        assert!(
            text.contains(r#""method":"now_ms","arg":"now_ms","bytes":0,"result":"ok","now":1794225600000"#),
            "{text}"
        );
    }
}

// ---- Log

const SHOW: &str = r#"
fn show(r: Result<(), LogErr>) -> Str { match r { Ok(_) => "ok", Err(e) => "too-large" } }
fn big(n: Int) -> Str { let mut s = "x"; for i in 0..n { s = s + s; } s }
"#;

#[test]
fn log_escapes_control_characters() {
    let src = format!(
        r#"{SHOW}fn main(log: Log) -> Str {{
      show(log.write("a\tb\\c\nd\re\0f")) + " " + show(log.write("plain")) + " " + show(log.write(""))
    }}"#
    );
    for compiled in [true, false] {
        let buf = Buf::default();
        let v = shown(run(&src, log(&buf), compiled, Limits::default(), None));
        assert_eq!(v, "ok ok ok");
        assert_eq!(buf.text(), "a\\tb\\\\c\\nd\\re\\u{00}f\nplain\n\n");
    }
}

/// 64 KB a line and 1 MB a run, counted after escaping; a refused line writes nothing.
#[test]
fn log_limits() {
    let src = format!(
        r#"{SHOW}fn main(log: Log) -> Str {{
      let a = show(log.write(big(16)));
      let b = show(log.write(big(16) + "x"));
      let c = show(log.write(big(16).slice(0, 65535) + "\t"));
      let mut n = 0;
      for i in 0..15 {{ if show(log.write(big(16))) == "ok" {{ n = n + 1; }} }}
      let d = show(log.write("x"));
      let e = show(log.write(""));
      a + " " + b + " " + c + " " + n.to_str() + " " + d + " " + e
    }}"#
    );
    for compiled in [true, false] {
        let buf = Buf::default();
        let audit = Buf::default();
        let v = shown(run(&src, log(&buf), compiled, Limits::default(), Some(audit.clone())));
        assert_eq!(v, "ok too-large too-large 15 too-large ok");
        let text = buf.text();
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 18, "16 big lines, one empty, and the end");
        assert!(lines[..16]
            .iter()
            .all(|l| l.len() == 65536 && l.bytes().all(|b| b == b'x')));
        assert_eq!(lines[16], "");
        let audit = audit.text();
        assert!(
            audit.contains(r#""method":"write","arg":"write","bytes":65536,"result":"ok""#),
            "{audit}"
        );
        assert!(
            audit.contains(r#""method":"write","arg":"write","bytes":0,"result":"too-large""#),
            "{audit}"
        );
        assert!(!audit.contains("xxxx"), "the audit log holds a line's text");
    }
}

/// The run's budget is per run, so a new run of the same process starts over.
#[test]
fn log_budget_is_per_run() {
    let src = format!(
        r#"{SHOW}fn main(log: Log) -> Str {{ let mut n = 0; for i in 0..17 {{ if show(log.write(big(16))) == "ok" {{ n = n + 1; }} }} n.to_str() }}"#
    );
    let buf = Buf::default();
    let mut k = Kio::new(Limits::default()).unwrap();
    for _ in 0..2 {
        k.grant_sources(log(&buf));
        assert_eq!(k.run(&src, vec![]).unwrap().unwrap().value, "16");
    }
    assert_eq!(buf.text().len(), 32 * 65537);
}

// ---- The binary: `--rng`, `--clock`, `--log` flags, `--listen`, refusals

/// `kio` with `args`: exit code, stdout, stderr.
fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (out.status.code().unwrap(), text(out.stdout), text(out.stderr))
}

fn write(dir: &std::path::Path, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.to_str().unwrap().into()
}

#[test]
fn flags_grant_and_the_report_lists_the_calls() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(
        t.path(),
        "p.kio",
        r#"fn main(r: Rng, c: Clock, l: Log) -> Str {
          let w = l.write("hello");
          r.bytes(4).to_hex() + " " + r.int(0, 10).to_str() + " " + c.now_ms().to_str()
        }"#,
    );
    let (code, out, _) = kio(&["check", &prog]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "main : r bytes int, c now_ms, l write");
    let (code, out, err) = kio(&[
        "run",
        &prog,
        "--rng",
        "r=seed:1",
        "--clock",
        "c=fixed:2026-11-09T12:00:00Z",
        "--log",
        "l=-",
    ]);
    assert_eq!(code, 0, "{err}");
    assert!(out.trim().ends_with(" 1794225600000"), "{out}");
    assert!(err.contains("hello\n"), "{err}");
    assert!(
        !err.contains("req "),
        "a one-shot run's log line has no request prefix: {err}"
    );
    let (code2, out2, _) = kio(&[
        "run",
        &prog,
        "--rng",
        "r=seed:1",
        "--clock",
        "c=fixed:0",
        "--log",
        "l=-",
    ]);
    assert_eq!(code2, 0);
    assert_eq!(
        out.split(' ').take(2).collect::<Vec<_>>(),
        out2.split(' ').take(2).collect::<Vec<_>>()
    );
    // Ungranted: the usual error, with the flag in the hint.
    let (code, _, err) = kio(&["run", &prog]);
    assert_eq!(code, 1);
    assert!(err.contains("no grant for `r: Rng`"), "{err}");
    assert!(
        err.contains("grant it: `--rng r` (the OS's randomness) or `--rng r=seed:N`"),
        "{err}"
    );
    let (code, _, err) = kio(&["run", &prog, "--rng", "r"]);
    assert!(
        err.contains("grant it: `--clock c` (the system clock) or `--clock c=fixed:<ms or RFC 3339 time>`"),
        "{err}"
    );
    assert_eq!(code, 1);
    let (code, _, err) = kio(&["run", &prog, "--rng", "r", "--clock", "c"]);
    assert!(
        err.contains("grant it: `--log l=path` (appended) or `--log l=-` (stderr)"),
        "{err}"
    );
    assert_eq!(code, 1);
    // Bad specs.
    let (code, _, err) = kio(&["run", &prog, "--rng", "r=seed:x"]);
    assert_eq!(
        (code, err.contains("--rng r=seed:x: \"seed:x\": expected `seed:N`")),
        (2, true),
        "{err}"
    );
    let (code, _, err) = kio(&["run", &prog, "--clock", "c=now"]);
    assert_eq!(
        (code, err.contains("--clock c=now: \"now\": expected `fixed:<ms>`")),
        (2, true),
        "{err}"
    );
    let (code, _, err) = kio(&["run", &prog, "--log", "l"]);
    assert_eq!(
        (code, err.contains("--log l: expected name=path or name=- (stderr)")),
        (2, true),
        "{err}"
    );
    let (code, _, err) = kio(&["run", &prog, "--rng", "=seed:1"]);
    assert_eq!(
        (code, err.contains("--rng =seed:1: expected a name")),
        (2, true),
        "{err}"
    );
}

#[test]
fn a_log_file_is_appended_and_its_path_rules_hold() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(
        t.path(),
        "p.kio",
        r#"fn main(l: Log) -> Str { let w = l.write("one"); "done" }"#,
    );
    let file = t.path().join("app.log");
    let path = file.to_str().unwrap();
    for _ in 0..2 {
        let (code, out, err) = kio(&["run", &prog, "--log", &format!("l={path}")]);
        assert_eq!((code, out.trim()), (0, "done"), "{err}");
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\none\n");
    // Not a symlink.
    let link = t.path().join("link.log");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let (code, _, err) = kio(&["run", &prog, "--log", &format!("l={}", link.display())]);
    assert_eq!(code, 2, "{err}");
    assert!(err.starts_with(&format!("kio: --log l={}: ", link.display())), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\none\n");
    // Not inside a granted dir, not a store, not the audit log, not another log.
    let data = t.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let inside = data.join("x.log");
    let prog2 = write(
        t.path(),
        "q.kio",
        r#"fn main(l: Log, d: Dir, s: Kv) -> Str { let w = l.write("one"); "done" }"#,
    );
    let dir = format!("d={}", data.display());
    let store = t.path().join("s.kv");
    let kv = format!("s={}", store.display());
    let (code, _, err) = kio(&[
        "run",
        &prog2,
        "--dir",
        &dir,
        "--kv",
        &kv,
        "--log",
        &format!("l={}", inside.display()),
    ]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        format!("kio: `--log l {}` is inside `--dir d`\n  hint: keep the log outside granted directories, where the program cannot read or replace it\n", inside.display())
    );
    let (code, _, err) = kio(&[
        "run",
        &prog2,
        "--dir",
        &dir,
        "--kv",
        &kv,
        "--log",
        &format!("l={}", store.display()),
    ]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        format!("kio: `--log l {}` is the same file as `--kv s`\n  hint: give the log its own file; writing it would corrupt the store\n", store.display())
    );
    let (code, _, err) = kio(&["run", &prog, "--audit", path, "--log", &format!("l={path}")]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        format!("kio: `--log l {path}` is the same file as `--audit`\n  hint: give each log its own file\n")
    );
    let prog3 = write(t.path(), "r.kio", r#"fn main(a: Log, b: Log) -> Str { "done" }"#);
    let (code, _, err) = kio(&[
        "run",
        &prog3,
        "--log",
        &format!("a={path}"),
        "--log",
        &format!("b={path}"),
    ]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        format!("kio: `--log b {path}` is the same file as `--log a`\n  hint: give each log its own file\n")
    );
    // `--audit` keeps its own words.
    let (code, _, err) = kio(&[
        "run",
        &prog2,
        "--dir",
        &dir,
        "--kv",
        &kv,
        "--audit",
        inside.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert!(
        err.contains("hint: keep the audit log outside granted directories"),
        "{err}"
    );
}

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

/// `kio run app --listen 127.0.0.1:0 <extra>`, once it listens.
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
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    Server { child, port }
}

/// One GET on a fresh connection: status and body.
fn get(port: u16, target: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET {target} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8(raw).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (head.split(' ').nth(1).unwrap().parse().unwrap(), body.to_string())
}

/// Fifty requests at once each write a line: every line is whole, prefixed with its request
/// number, and the numbers are distinct.
#[test]
fn listen_log_lines_never_interleave() {
    let t = tempfile::tempdir().unwrap();
    let app = write(
        t.path(),
        "app.kio",
        r#"fn main(req: Incoming, res: Outgoing, log: Log) -> Result<(), WebErr> {
          let w = log.write("hello " + req.target() + "\n" + "second");
          res.send(200, [], [], "ok".to_bytes())
        }"#,
    );
    let file = t.path().join("app.log");
    let server = start(&app, &["--log", &format!("log={}", file.display())]);
    let port = server.port;
    let threads: Vec<_> = (0..50)
        .map(|i| std::thread::spawn(move || get(port, &format!("/n{i}"))))
        .collect();
    let ok = threads
        .into_iter()
        .map(|t| t.join().unwrap().0)
        .filter(|&status| status == 200)
        .count();
    drop(server);
    let text = std::fs::read_to_string(&file).unwrap();
    let mut nums = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for line in text.lines() {
        let rest = line.strip_prefix("req ").unwrap_or_else(|| panic!("{line:?}"));
        let (n, rest) = rest.split_once(": hello /").unwrap();
        nums.insert(n.parse::<u64>().unwrap());
        let (target, tail) = rest.split_once("\\n").unwrap();
        assert_eq!(tail, "second");
        targets.insert(target.to_string());
    }
    assert_eq!(text.lines().count(), ok);
    assert_eq!(nums.len(), ok);
    assert_eq!(targets.len(), ok);
    assert!(ok >= 45, "only {ok} of 50 succeeded");
}

/// Under `--listen` a seeded `Rng` is keyed by the request number: requests differ, and a
/// restarted server replays them in order.
#[test]
fn listen_seeded_streams_differ_per_request_and_replay() {
    let t = tempfile::tempdir().unwrap();
    let app = write(
        t.path(),
        "app.kio",
        r#"fn main(req: Incoming, res: Outgoing, r: Rng) -> Result<(), WebErr> {
          res.send(200, [], [], (r.bytes(8).to_hex() + " " + r.int(0, 1000).to_str()).to_bytes())
        }"#,
    );
    let first: Vec<String> = {
        let s = start(&app, &["--rng", "r=seed:1"]);
        (0..3).map(|_| get(s.port, "/").1).collect()
    };
    assert_ne!(first[0], first[1]);
    assert_ne!(first[1], first[2]);
    let again: Vec<String> = {
        let s = start(&app, &["--rng", "r=seed:1"]);
        (0..3).map(|_| get(s.port, "/").1).collect()
    };
    assert_eq!(first, again);
    let other = {
        let s = start(&app, &["--rng", "r=seed:2"]);
        get(s.port, "/").1
    };
    assert_ne!(first[0], other);
    // The one-shot stream (0) is none of the requests' (1..).
    let one_shot = shown(run(
        "fn main(r: Rng) -> Str { r.bytes(8).to_hex() + \" \" + r.int(0, 1000).to_str() }",
        rng(RngSpec::Seed(1)),
        true,
        Limits::default(),
        None,
    ));
    assert!(!first.contains(&one_shot));
}
