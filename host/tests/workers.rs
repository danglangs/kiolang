//! H5/W8: detached host-call workers are capped. Own test binary: the cap is process-wide.

use kio_host::{open_dir, Kio, Limits};

/// 64 reads blocked (the `pipe` test hook) each leave a detached worker; the next host call,
/// even one that would not block, is refused as a timeout.
#[cfg(unix)]
#[test]
fn blocked_reads_stop_at_the_worker_cap() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a"), "x").unwrap();
    let limits = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let run = |src: &str| {
        let mut kio = Kio::new(limits).unwrap();
        let err = kio
            .run(src, vec![("d".into(), open_dir(t.path()).unwrap())])
            .unwrap_err();
        kio.explain(&err).unwrap()
    };
    let blocked = "fn main(d: Dir) -> Int { let r = d.read(\"pipe\"); 0 }";
    std::thread::scope(|s| {
        for _ in 0..64 {
            s.spawn(|| assert!(run(blocked).starts_with("timed out")));
        }
    });
    let quick = "fn main(d: Dir) -> Int { let r = d.read(\"a\"); 0 }";
    assert!(run(quick).starts_with("timed out"));
}
