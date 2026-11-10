//! H19: HTTP workers. Own test binary: the ceiling is process-wide, and one test lowers it.

use kio_host::{host_workers, http_workers, open_dir, set_http_worker_ceiling, Kio, Limits, Origin, MAX_HTTP_WORKERS};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod upstream;

/// The tests here run one at a time: one lowers the ceiling the other relies on.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// `main(h: Http)` sending `n` GETs of `path` at once; each result's status or error.
fn batch(n: usize, path: &str) -> String {
    format!(
        r#"fn main(h: Http) -> List<Result<Int, HttpErr>> {{ let mut rs: List<HttpRequest> = []; for i in 0..{n} {{ rs.push(HttpRequest {{ method: "GET", path: "{path}", headers: [], body: "".to_bytes(), timeout_ms: 0 }}); }} h.send_all(rs).map(|r| r.map(|r| r.status)) }}"#
    )
}

fn run(src: &str, grant: &str, limits: Limits) -> String {
    let origin = Origin::parse(grant).unwrap();
    let mut kio = Kio::new(limits).unwrap();
    kio.run_with(src, vec![], vec![("h".into(), origin)])
        .unwrap()
        .unwrap()
        .value
}

fn statuses(s: &str, n: usize) -> String {
    format!("[{}]", vec![s; n].join(", "))
}

/// A run holds up to 64 workers at once, its own: two runs' full batches run side by side, so
/// the upstream sees more than 64 at once, and each batch takes one wait, not two.
#[test]
fn the_cap_is_per_run() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let up = upstream::start();
    let start = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| {
                assert_eq!(
                    run(&batch(64, "/wait"), &up.grant, Limits::default()),
                    statuses("Ok(200)", 64)
                )
            });
        }
    });
    let most = up.most.load(std::sync::atomic::Ordering::SeqCst);
    assert!(most > 64 && most <= 128, "{most}");
    assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// With the process at its HTTP ceiling, a new batch and a new `get` are `timeout` at once,
/// nothing sent, audited `busy`; another run's `Dir` read still succeeds. Once the hung workers
/// end at their run's deadline, batches go through again.
#[test]
fn the_ceiling_refuses_new_requests_not_dir_reads() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let up = upstream::start();
    // A worker gives its place back just after its result, so the last test's may linger briefly.
    no_http_workers();
    set_http_worker_ceiling(16);
    let _restore = Ceiling;
    let hung = {
        let grant = up.grant.clone();
        let limits = Limits {
            timeout_secs: 3,
            ..Limits::default()
        };
        std::thread::spawn(move || {
            let origin = Origin::parse(&grant).unwrap();
            let mut kio = Kio::new(limits).unwrap();
            // At the deadline the run stops, or its workers answer `timeout` first: either way.
            let _ = kio.run_with(&batch(16, "/hang"), vec![], vec![("h".into(), origin)]);
        })
    };
    let start = Instant::now();
    while http_workers() < 16 || up.seen.lock().unwrap().len() < 16 {
        assert!(start.elapsed() < Duration::from_secs(2), "the hung batch never started");
        std::thread::sleep(Duration::from_millis(10));
    }

    let log = Log::default();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(log.clone(), vec![]);
    let src = r#"fn main(h: Http) -> List<Result<Int, HttpErr>> {
        let one = HttpRequest { method: "GET", path: "/ok", headers: [], body: "".to_bytes(), timeout_ms: 0 };
        let mut out = h.send_all([one, one]).map(|r| r.map(|r| r.status));
        out.push(h.get("/ok").map(|b| b.len()));
        out
    }"#;
    let quick = Instant::now();
    let out = kio
        .run_with(src, vec![], vec![("h".into(), Origin::parse(&up.grant).unwrap())])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, statuses("Err(HttpErr::Timeout)", 3));
    assert!(quick.elapsed() < Duration::from_secs(1), "{:?}", quick.elapsed());
    assert_eq!(up.seen.lock().unwrap().len(), 16);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let busy: Vec<&str> = text.lines().filter(|l| l.contains(r#""result":"timeout","#)).collect();
    assert_eq!(busy.len(), 3, "{text}");
    assert!(busy.iter().all(|l| l.contains(r#""busy":true"#)), "{text}");

    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a"), "x").unwrap();
    let read = "fn main(d: Dir) -> Result<Str, IoErr> { Ok(d.read(\"a\")?.to_str()) }";
    let mut kio = Kio::new(Limits::default()).unwrap();
    let out = kio
        .run(read, vec![("d".into(), open_dir(t.path()).unwrap())])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, r#"Ok("x")"#);

    hung.join().unwrap();
    no_http_workers();
    assert_eq!(
        run(&batch(2, "/ok"), &up.grant, Limits::default()),
        statuses("Ok(200)", 2)
    );
}

/// H19 review: under mixed load (full batches, small batches, hung and stalled requests that
/// outlive their 1 s runs, file reads beside them) both pools return to zero as soon as the runs
/// end: a worker the deadline left behind ends at the same instant (ureq's global timeout is the
/// run's deadline), so it never counts against a later run.
#[test]
fn pools_balance_under_mixed_load() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    no_http_workers();
    let up = upstream::start();
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a"), "x").unwrap();
    let short = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let one = |path: &str| format!("fn main(h: Http) -> Result<Int, HttpErr> {{ Ok(h.get(\"{path}\")?.len()) }}");
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                assert_eq!(
                    run(&batch(64, "/wait"), &up.grant, Limits::default()),
                    statuses("Ok(200)", 64)
                );
                assert_eq!(
                    run(&batch(16, "/ok"), &up.grant, Limits::default()),
                    statuses("Ok(200)", 16)
                );
            });
        }
        for _ in 0..4 {
            // Each stops at its deadline, or its requests answer `timeout` first: either way.
            for src in [batch(8, "/hang"), one("/trickle"), one("/hang")] {
                let grant = up.grant.clone();
                s.spawn(move || {
                    let mut kio = Kio::new(short).unwrap();
                    let _ = kio.run_with(&src, vec![], vec![("h".into(), Origin::parse(&grant).unwrap())]);
                });
            }
        }
        for _ in 0..4 {
            s.spawn(|| {
                let read = "fn main(d: Dir) -> Result<Str, IoErr> { Ok(d.read(\"a\")?.to_str()) }";
                for _ in 0..10 {
                    let mut kio = Kio::new(Limits::default()).unwrap();
                    let out = kio
                        .run(read, vec![("d".into(), open_dir(t.path()).unwrap())])
                        .unwrap()
                        .unwrap();
                    assert_eq!(out.value, r#"Ok("x")"#);
                }
            });
        }
    });
    let start = Instant::now();
    while http_workers() > 0 || host_workers() > 0 {
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "workers linger: {} HTTP, {} other",
            http_workers(),
            host_workers()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wait up to 2 s for every HTTP worker in the process to end.
fn no_http_workers() {
    let start = Instant::now();
    while http_workers() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "HTTP workers outlived their run"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Puts the ceiling back when the test ends, even if it fails.
struct Ceiling;

impl Drop for Ceiling {
    fn drop(&mut self) {
        set_http_worker_ceiling(MAX_HTTP_WORKERS);
    }
}
