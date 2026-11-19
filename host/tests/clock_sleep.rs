//! H36: `Clock.sleep_ms(ms)` waits on the system clock, moves a fixed clock forward, and never
//! outlives the run's deadline. Interpreted and compiled.
use kio_host::{ClockSpec, Interp, Kio, Limits, RunResult, Sources};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

fn clock(spec: ClockSpec) -> Sources {
    Sources {
        clocks: vec![("c".into(), spec)],
        ..Sources::default()
    }
}

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

/// A real sleep takes at least its time and well under the run's timeout; each path's audit
/// line has the duration asked for.
#[test]
fn a_system_clock_waits() {
    let src = r#"fn main(c: Clock) -> Int { let a = c.now_ms(); c.sleep_ms(200); c.sleep_ms(0); c.now_ms() - a }"#;
    for compiled in [true, false] {
        let audit = Buf::default();
        let start = Instant::now();
        let v = shown(run(
            src,
            clock(ClockSpec::System),
            compiled,
            Limits::default(),
            Some(audit.clone()),
        ));
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(200), "{took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        let n: i64 = v.parse().unwrap();
        assert!((200..5000).contains(&n), "{v}");
        let text = String::from_utf8(audit.0.lock().unwrap().clone()).unwrap();
        assert!(
            text.contains(
                r#""kind":"Clock","method":"sleep_ms","arg":"sleep_ms","bytes":0,"result":"ok","sleep_ms":200"#
            ),
            "{text}"
        );
        assert!(text.contains(r#""result":"ok","sleep_ms":0"#), "{text}");
    }
}

/// A fixed clock moves forward by `ms` and waits for nothing, so a test of a retry after a
/// backoff needs one clock.
#[test]
fn a_fixed_clock_moves_forward_without_waiting() {
    let src = r#"fn main(c: Clock) -> Str {
      let mut out: List<Str> = [c.now_ms().to_str()];
      for i in 0..3 { c.sleep_ms(60000); out.push(c.now_ms().to_str()); }
      c.sleep_ms(0);
      out.push(c.now_ms().to_str());
      out.join(",")
    }"#;
    let start = Instant::now();
    assert_eq!(
        both(src, || clock(ClockSpec::Fixed(1_000))),
        "1000,61000,121000,181000,181000"
    );
    assert!(start.elapsed() < Duration::from_secs(5));
}

/// Outside 0 to 60,000 is a runtime error at the call, on both paths, with the same message.
#[test]
fn the_range_is_checked_at_the_call() {
    for (ms, want) in [
        ("-1", "Clock.sleep_ms(-1): the duration must be 0 to 60000"),
        ("60001", "Clock.sleep_ms(60001): the duration must be 0 to 60000"),
        (
            "0 - 9223372036854775807 - 1",
            "Clock.sleep_ms(-9223372036854775808): the duration must be 0 to 60000",
        ),
    ] {
        let src = format!("fn main(c: Clock) -> Int {{ c.sleep_ms({ms}); 1 }}");
        let start = Instant::now();
        let got = both(&src, || clock(ClockSpec::Fixed(0)));
        assert!(got.starts_with(&format!("error: {want} @")), "{got}");
        assert_eq!(got, both(&src, || clock(ClockSpec::System)));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}

/// A sleep past `--timeout` ends the run at the deadline as a timeout, not after the sleep, on
/// both paths (a limit stops a run as a trap does: `explain` has the message).
#[test]
fn a_sleep_past_the_timeout_ends_the_run_at_the_deadline() {
    let src = r#"fn main(c: Clock) -> Int { c.sleep_ms(60000); 1 }"#;
    let limits = || Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    for compiled in [true, false] {
        let audit = Buf::default();
        let start = Instant::now();
        let explained = if compiled {
            let mut k = Kio::new(limits()).unwrap();
            k.audit(audit.clone(), vec![]);
            k.grant_sources(clock(ClockSpec::System));
            let e = k.run(src, vec![]).unwrap_err();
            k.explain(&e)
        } else {
            let mut k = Interp::new(limits()).unwrap();
            k.audit(audit.clone(), vec![]);
            k.grant_sources(clock(ClockSpec::System));
            let e = k.run(src, vec![]).unwrap_err();
            k.explain(&e)
        };
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(900), "{took:?}");
        assert!(
            took < Duration::from_secs(5),
            "{took:?}: the sleep outlived the deadline"
        );
        assert_eq!(
            explained.as_deref(),
            Some("timed out after 1 s (raise it with --timeout)"),
            "compiled {compiled}"
        );
        let text = String::from_utf8(audit.0.lock().unwrap().clone()).unwrap();
        assert!(
            text.contains(r#""method":"sleep_ms","arg":"sleep_ms","bytes":0,"result":"stopped:time","sleep_ms":60000"#),
            "{text}"
        );
    }
}
