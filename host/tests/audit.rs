use kio_host::{Kio, Limits};
use std::io::Write;
use std::sync::{Arc, Mutex};

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
    /// The log's lines, with the `ms` field removed so runs compare equal.
    fn lines(&self) -> Vec<String> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|l| match l.find(r#","ms":"#) {
                Some(i) => format!("{}}}", &l[..i]),
                None => l.to_string(),
            })
            .collect()
    }
}

/// Run the primer with `input` holding `invoices.csv` and `out` holding `out_files`.
fn primer(limits: Limits, out_files: &[&str], hide: Option<(&str, &str)>) -> (Log, bool) {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("invoices.csv"), "a,1\nb,2\n").unwrap();
    for f in out_files {
        std::fs::write(b.path().join(f), "old").unwrap();
    }
    let dirs = vec![
        ("input".to_string(), kio_host::open_dir(a.path()).unwrap()),
        ("out".to_string(), kio_host::open_dir(b.path()).unwrap()),
    ];
    let log = Log::default();
    let mut kio = Kio::new(limits).unwrap();
    kio.audit(
        log.clone(),
        vec![("input".into(), "/in".into()), ("out".into(), "/out".into())],
    );
    if let Some((c, m)) = hide {
        kio.hide_from_report(c, m);
    }
    let stopped = kio.run(&src, dirs).is_err();
    (log, stopped)
}

#[test]
fn primer_log_has_header_two_calls_and_trailer() {
    let (log, stopped) = primer(Limits::default(), &[], None);
    let lines = log.lines();
    assert!(!stopped);
    assert_eq!(lines.len(), 4);
    let hash = lines[0].split(r#""program_hash":""#).nth(1).unwrap();
    assert_eq!(hash.find('"'), Some(64));
    assert!(lines[0].starts_with(r#"{"event":"start","program_hash":""#));
    assert!(lines[0].contains(
        r#""grants":[{"name":"input","kind":"Dir","target":"/in"},{"name":"out","kind":"Dir","target":"/out"}]"#
    ));
    assert_eq!(
        lines[1],
        r#"{"event":"call","seq":1,"cap":"input","kind":"Dir","method":"read","arg":"invoices.csv","bytes":8,"result":"ok"}"#
    );
    assert_eq!(
        lines[2],
        r#"{"event":"call","seq":2,"cap":"out","kind":"Dir","method":"create","arg":"total.txt","bytes":1,"result":"ok"}"#
    );
    assert!(lines[3].starts_with(r#"{"event":"end","outcome":"ok","calls":2,"io":9,"peak_memory":"#));
}

#[test]
fn denied_create_is_logged() {
    let (log, _) = primer(Limits::default(), &["total.txt"], None);
    let lines = log.lines();
    assert!(
        lines[2].contains(r#""method":"create""#) && lines[2].contains(r#""result":"denied""#),
        "{lines:?}"
    );
    assert!(lines[3].contains(r#""outcome":"failed""#), "main returned Err");
}

#[test]
fn quota_stop_is_logged() {
    let limits = Limits {
        calls: 1,
        ..Limits::default()
    };
    let (log, stopped) = primer(limits, &[], None);
    let lines = log.lines();
    assert!(stopped);
    assert!(lines[2].contains(r#""result":"stopped:calls""#), "{lines:?}");
    assert!(lines[3].contains(r#""outcome":"error""#) && lines[3].contains(r#""calls":2"#));
}

#[test]
fn same_program_same_log() {
    let (a, _) = primer(Limits::default(), &[], None);
    let (b, _) = primer(Limits::default(), &[], None);
    assert_eq!(a.lines(), b.lines());
}

#[test]
fn call_missing_from_the_report_stops_the_run() {
    let (log, stopped) = primer(Limits::default(), &[], Some(("out", "create")));
    let lines = log.lines();
    assert!(stopped);
    assert!(
        lines[2].contains(r#""result":"stopped:unexpected","unexpected":true"#),
        "{lines:?}"
    );
    assert!(lines[3].contains(r#""outcome":"error""#));
}

#[test]
fn report_check_holds_without_a_log() {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("invoices.csv"), "a,1\n").unwrap();
    let dirs = vec![
        ("input".to_string(), kio_host::open_dir(a.path()).unwrap()),
        ("out".to_string(), kio_host::open_dir(b.path()).unwrap()),
    ];
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.hide_from_report("input", "read");
    let e = kio.run(&src, dirs).unwrap_err();
    let msg = kio.explain(&e).unwrap();
    assert!(
        msg.contains("`input.read` not in the authority report (a checker bug)"),
        "{msg}"
    );
}
