//! Differential test: every program in the lang tests and examples that codegen supports gives
//! the same result compiled as interpreted (docs/codegen.md "Testing"). A program that takes
//! capabilities gets the same fresh grants on each side (`World`), and must also make the same
//! host calls (its audit log) and the same response or WebSocket frames.

use kio_host::kv::Store;
use kio_host::ws::WsEvent;
use kio_host::{ClockSpec, Interp, Kio, Limits, LogSink, Origin, Request, RngSpec, Sources};
use lang::CapTable;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[path = "../../lang/tests/common/corpus.rs"]
mod corpus;

/// What a run gave: the displayed value and failed flag, or the first error as (message, span).
type Run = Result<(String, bool), (String, usize, usize)>;

/// A finished run, or the diagnostic it gave, or a host error's text.
type Res<T> = Result<Result<T, (String, usize, usize)>, String>;

/// A run ended early: by a limit (decision 6: the two paths limit differently), or for any
/// other reason, which is a mismatch.
#[derive(Debug, PartialEq)]
enum End {
    Done(Run),
    Limit,
}

/// A run's end, what it sent (a response or frames, debug-printed) and its audit log.
type Seen = (End, String, Vec<String>);

fn is_limit(msg: &str) -> bool {
    ["out of fuel", "stack overflow", "evaluation is too deep"]
        .iter()
        .any(|l| msg.contains(l))
}

fn limits() -> Limits {
    Limits {
        fuel: 2_000_000_000,
        ..Limits::default()
    }
}

fn finish<T>(r: Res<T>, f: impl Fn(T) -> Run) -> End {
    match r {
        Ok(Ok(v)) => End::Done(f(v)),
        Ok(Err(d)) => match d {
            (m, _, _) if is_limit(&m) => End::Limit,
            (m, s, e) => End::Done(Err((m, s, e))),
        },
        Err(m) if is_limit(&m) => End::Limit,
        Err(m) => End::Done(Err((format!("host error: {m}"), 0, 0))),
    }
}

/// An audit log in memory.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    /// The lines without what differs between the paths: each call's duration, and the
    /// trailer's peak memory and fuel.
    fn lines(&self) -> Vec<String> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|l| {
                let cut = l.find(r#","ms":"#).or_else(|| l.find(r#","peak_memory":"#));
                match cut {
                    Some(i) => format!("{}}}", &l[..i]),
                    None => l.to_string(),
                }
            })
            .collect()
    }
}

/// Fresh grants for `main`'s parameters: each `Dir` a new directory holding the same files, each
/// `Http` an origin that refuses connections, each `Kv` a new store; a request for
/// `Incoming`/`Outgoing` and an event for `WsMessage`/`WsSend`.
struct World {
    _temps: Vec<tempfile::TempDir>,
    dirs: Vec<(String, cap_std::fs::Dir)>,
    https: Vec<(String, Origin)>,
    kvs: kio_host::KvGrants,
    /// Each `Rng` seeded 1, each `Clock` fixed, each `Log` writing to `log`.
    sources: Sources,
    log: Log,
    web: bool,
    ws: Option<(String, String)>,
}

fn world(params: &[(String, String)]) -> World {
    let mut w = World {
        _temps: vec![],
        dirs: vec![],
        https: vec![],
        kvs: vec![],
        sources: Sources::default(),
        log: Log::default(),
        web: false,
        ws: None,
    };
    let named = |ty: &str| params.iter().find(|(_, t)| t == ty).map(|(n, _)| n.clone());
    for (name, ty) in params {
        match ty.as_str() {
            "Rng" => w.sources.rngs.push((name.clone(), RngSpec::Seed(1))),
            "Clock" => w
                .sources
                .clocks
                .push((name.clone(), ClockSpec::Fixed(1_794_225_600_000))),
            "Log" => {
                let sink = LogSink::new(Box::new(w.log.clone()), "mem");
                w.sources.logs.push((name.clone(), sink));
            }
            "Dir" => {
                let t = tempfile::tempdir().unwrap();
                std::fs::write(t.path().join("invoices.csv"), "a,1\nb,2\n").unwrap();
                std::fs::write(t.path().join("notes.txt"), "hello\n").unwrap();
                std::fs::create_dir(t.path().join("sub")).unwrap();
                std::fs::write(t.path().join("sub/x.txt"), "x").unwrap();
                w.dirs.push((name.clone(), kio_host::open_dir(t.path()).unwrap()));
                w._temps.push(t);
            }
            "Http" => w
                .https
                .push((name.clone(), Origin::parse("http://127.0.0.1:1").unwrap())),
            "Kv" => w.kvs.push((name.clone(), Arc::new(Store::memory(1 << 20)))),
            "Db" => w.sources.dbs.push((
                name.clone(),
                Arc::new(kio_host::db::Db::memory(kio_host::db::DB_MB).unwrap()),
            )),
            _ => {}
        }
    }
    w.web = named("Incoming").is_some() && named("Outgoing").is_some();
    w.ws = named("WsMessage").zip(named("WsSend"));
    w
}

fn request() -> Request {
    Request {
        method: "POST".into(),
        target: "/a?b=1".into(),
        header_names: vec!["host".into(), "x-n".into()],
        header_values: vec!["example".into(), "1".into()],
        body: b"ping".to_vec(),
    }
}

fn event() -> WsEvent {
    WsEvent {
        kind: "text",
        conn: 7,
        data: b"hi".to_vec(),
        upgrade: Default::default(),
    }
}

/// A run's result, and what it sent printed.
fn sent<R, S: std::fmt::Debug>(r: wasmtime::Result<(R, S)>) -> (wasmtime::Result<R>, String) {
    match r {
        Ok((r, s)) => (Ok(r), format!("{s:?}")),
        Err(e) => (Err(e), String::new()),
    }
}

/// The audit header's `Dir` targets.
fn paths(w: &World) -> Vec<(String, String)> {
    w.dirs.iter().map(|(n, _)| (n.clone(), format!("/{n}"))).collect()
}

fn compiled(src: &str, params: &[(String, String)]) -> Seen {
    let w = world(params);
    let log = Log::default();
    let mut c = Kio::new(limits()).unwrap();
    c.audit(log.clone(), paths(&w));
    c.grant_kvs(w.kvs);
    c.grant_sources(w.sources);
    let (r, sent) = if w.web {
        sent(c.run_web(src, w.dirs, request()))
    } else if let Some(names) = &w.ws {
        sent(c.run_ws(src, w.dirs, event(), false, names))
    } else {
        (c.run_with(src, w.dirs, w.https), String::new())
    };
    let r = match r {
        Ok(Ok(o)) => Ok(Ok(o)),
        Ok(Err(d)) => Ok(Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize))),
        Err(e) => Err(format!("{e:#}")),
    };
    let sent = format!("{sent}{}", w.log.lines().join("\n"));
    (finish(r, |o| Ok((o.value, o.failed))), sent, log.lines())
}

fn interpreted(src: &str, params: &[(String, String)]) -> Seen {
    let w = world(params);
    let log = Log::default();
    let mut k = Interp::new(limits()).unwrap();
    k.audit(log.clone(), paths(&w));
    k.grant_kvs(w.kvs);
    k.grant_sources(w.sources);
    let (r, sent) = if w.web {
        sent(k.run_web(src, w.dirs, request()))
    } else if let Some(names) = &w.ws {
        sent(k.run_ws(src, w.dirs, event(), false, names))
    } else {
        (k.run_with(src, w.dirs, w.https), String::new())
    };
    let r = match r {
        Ok(Ok(o)) => Ok(Ok(o)),
        Ok(Err(d)) => Ok(Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize))),
        Err(e) => Err(k.explain(&e).unwrap_or_else(|| format!("{e:#}"))),
    };
    let sent = format!("{sent}{}", w.log.lines().join("\n"));
    (finish(r, |o| Ok((o.value, o.failed))), sent, log.lines())
}

fn differential() {
    let caps = CapTable::v0();
    let mut ran = 0;
    let mut agreed = 0;
    let mut with_caps = 0;
    let mut limited = 0;
    let mut unsupported: BTreeMap<String, usize> = BTreeMap::new();
    let mut mismatches = Vec::new();
    let mut leaks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for files in corpus::sources(Path::new("../")) {
        let [(_, src)] = files.as_slice() else { continue };
        if !seen.insert(src.clone()) {
            continue;
        }
        let Ok(program) = lang::check(src, &caps) else { continue };
        let params = program.main_params();
        let c = compiled(src, &params);
        // Only IR codegen does not handle yet (its `unimplemented!`) is a skip; any other codegen
        // panic is an "internal compiler error", which fails as a mismatch.
        if let End::Done(Err((m, _, _))) = &c.0 {
            if let Some(node) = m.strip_prefix("host error: not compiled yet: ") {
                *unsupported.entry(node.to_string()).or_default() += 1;
                continue;
            }
        }
        ran += 1;
        // The runtime traps when objects are left alive at the end of a run that did not trap.
        if let End::Done(Err((m, _, _))) = &c.0 {
            if m.contains("compiled program leaked") {
                leaks.push(format!("{m}\n{src}"));
                continue;
            }
        }
        // A program stopped by a limit compiled is not compared (decision 6). The native
        // interpreter has no fuel, so it is not run on it: it may loop forever.
        if c.0 == End::Limit {
            limited += 1;
            continue;
        }
        let i = interpreted(src, &params);
        if i.0 == End::Limit {
            limited += 1;
        } else if c == i {
            agreed += 1;
            with_caps += !params.is_empty() as usize;
        } else {
            mismatches.push(format!("compiled: {c:?}\ninterpreted: {i:?}\n{src}"));
        }
    }
    println!(
        "ran both ways {ran}, agreed {agreed} ({with_caps} with capabilities), excluded by limits {limited}, leaked {}, unsupported by node kind {unsupported:?}",
        leaks.len()
    );
    assert!(leaks.is_empty(), "{} leaked:\n{}", leaks.len(), leaks.join("\n----\n"));
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n----\n")
    );
    assert!(agreed > 20, "only {agreed} programs agreed");
    assert!(with_caps > 20, "only {with_caps} programs with capabilities agreed");
}

/// H13-H15: programs on `Rng` (seeded), `Clock` (fixed) and `Log` give the same values, the same
/// log lines and the same audit log both ways, runtime errors included.
#[test]
fn sources_agree() {
    let programs = [
        r#"fn main(r: Rng, c: Clock, log: Log) -> Str {
            let w = log.write("t=" + c.now_ms().to_str() + "\t" + r.bytes(4).to_hex());
            let mut s = "";
            for i in 0..20 { s = s + r.int(-3, 3).to_str() + ","; }
            match log.write(s) { Ok(_) => s, Err(e) => "too-large" }
        }"#,
        r#"fn main(r: Rng) -> Int { r.int(0, 1000000) + r.bytes(65536).len() }"#,
        r#"fn main(r: Rng) -> Int { let n = r.int(0, 2) + 65536; r.bytes(n).len() }"#,
        r#"fn main(r: Rng) -> Int { let lo = r.int(0, 1); r.int(lo, lo) }"#,
        r#"fn big(n: Int) -> Str { let mut s = "x"; for i in 0..n { s = s + s; } s }
           fn main(log: Log) -> Int { let mut n = 0; for i in 0..17 { if log.write(big(16)).is_ok() { n = n + 1; } } n }"#,
    ];
    let caps = CapTable::v0();
    for src in programs {
        let params = lang::check(src, &caps).unwrap().main_params();
        let (c, i) = (compiled(src, &params), interpreted(src, &params));
        assert_eq!(c, i, "{src}");
        assert!(matches!(c.0, End::Done(_)), "{src}");
    }
}

#[test]
fn compiled_agrees_with_the_interpreter() {
    // Codegen's `unimplemented!` panics are expected and counted, so keep them quiet.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(differential);
    std::panic::set_hook(hook);
    if let Err(e) = r {
        std::panic::resume_unwind(e);
    }
}
