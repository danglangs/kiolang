//! L35: each date form `parse_iso_utc` accepts or refuses, and `header` on `HttpResponse` and
//! `Incoming`, give the same value compiled as interpreted.

use kio_host::{Interp, Kio, Limits, Request};

type Run = Result<String, String>;

fn shown(r: kio_host::RunResult) -> Run {
    r.map(|o| o.value).map_err(|d| d[0].message.clone())
}

fn both(src: &str) -> (Run, Run) {
    let c = shown(Kio::new(Limits::default()).unwrap().run(src, vec![]).unwrap());
    let i = shown(Interp::new(Limits::default()).unwrap().run(src, vec![]).unwrap());
    (c, i)
}

#[test]
fn each_date_form_agrees() {
    let forms = [
        ("2026-11-20T10:00:00.000Z", "Some(1795168800000)"),
        ("2026-11-20T10:00:00Z", "Some(1795168800000)"),
        ("2026-11-20T10:00:00.1Z", "Some(1795168800100)"),
        ("2026-11-20T10:00:00.12Z", "Some(1795168800120)"),
        ("2026-11-20T10:00:00.1239Z", "Some(1795168800123)"),
        ("2026-11-20T10:00:00.123456789Z", "Some(1795168800123)"),
        ("2026-11-20T10:00:00+01:00", "Some(1795165200000)"),
        ("2026-11-20T10:00:00-05:30", "Some(1795188600000)"),
        ("2026-11-20T10:00:00.5+00:00", "Some(1795168800500)"),
        ("2026-11-20T10:00:00+23:59", "Some(1795082460000)"),
        ("2026-11-20", "Some(1795132800000)"),
        ("2026-11-20T10:00:00z", "None"),
        ("2026-11-20T10:00:00+0100", "None"),
        ("2026-11-20 10:00:00Z", "None"),
        ("2026-11-20T23:59:60Z", "None"),
        ("2026-11-20T10:00:00.Z", "None"),
        ("2026-11-20T10:00:00.1234567890Z", "None"),
        ("2026-11-20T10:00:00+24:00", "None"),
        ("2026-11-20T10:00:00+01:60", "None"),
        ("2026-11-20T10:00:00", "None"),
        ("2026-11-20T10:00:00Zx", "None"),
    ];
    for (form, want) in forms {
        let src = format!("fn main() -> Option<Int> {{ \"{form}\".parse_iso_utc() }}");
        let (c, i) = both(&src);
        assert_eq!(c, Ok(want.to_string()), "{form}");
        assert_eq!(c, i, "{form}");
    }
}

#[test]
fn response_header_agrees() {
    let src = r#"fn main() -> List<Option<Str>> {
    let r = HttpResponse { status: 200, body: "".to_bytes(), headers: ["Content-Type: a/b", "x: 1", "X: 2", "e:", "bare"] };
    [r.header("content-type"), r.header("X"), r.header("e"), r.header("bare"), r.header("none")]
}"#;
    let (c, i) = both(src);
    assert_eq!(c, Ok(r#"[Some("a/b"), Some("1"), Some(""), None, None]"#.to_string()));
    assert_eq!(c, i);
}

#[test]
fn incoming_header_agrees() {
    let src = r#"fn main(req: Incoming, res: Outgoing) -> List<Option<Str>> {
    [req.header("Authorization"), req.header("x-n"), req.header("X-N"), req.header("cookie")]
}"#;
    let request = || Request {
        method: "GET".into(),
        target: "/".into(),
        header_names: vec!["authorization".into(), "x-n".into(), "x-n".into()],
        header_values: vec!["Bearer t".into(), "1".into(), "2".into()],
        body: vec![],
    };
    let (c, _) = Kio::new(Limits::default())
        .unwrap()
        .run_web(src, vec![], request())
        .unwrap();
    let (i, _) = Interp::new(Limits::default())
        .unwrap()
        .run_web(src, vec![], request())
        .unwrap();
    let (c, i) = (shown(c), shown(i));
    assert_eq!(c, Ok(r#"[Some("Bearer t"), Some("1"), Some("1"), None]"#.to_string()));
    assert_eq!(c, i);
}
