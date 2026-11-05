use kio_host::{Kio, Limits, Request, Response};

const ECHO: &str = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
    let names: List<Str> = ["content-type"];
    let values: List<Str> = ["text/plain"];
    res.send(200, names, values, req.body())
}"#;

fn request(body: &[u8]) -> Request {
    Request {
        method: "POST".into(),
        target: "/echo?x=1".into(),
        header_names: vec!["host".into()],
        header_values: vec!["localhost".into()],
        body: body.to_vec(),
    }
}

/// Run `src` on a request with body `ping`; returns the printed value and the sent response.
fn serve(src: &str) -> (String, Option<Response>) {
    let mut kio = Kio::new(Limits::default()).unwrap();
    let (outcome, sent) = kio.run_web(src, vec![], request(b"ping")).unwrap();
    (outcome.unwrap().value, sent)
}

#[test]
fn echoes_the_body() {
    let mut kio = Kio::new(Limits::default()).unwrap();
    let (outcome, sent) = kio.run_web(ECHO, vec![], request(b"ping")).unwrap();
    assert_eq!(outcome.unwrap().value, "Ok(())");
    let sent = sent.unwrap();
    assert_eq!(sent.status, 200);
    assert_eq!(sent.headers, [("content-type".to_string(), "text/plain".to_string())]);
    assert_eq!(sent.body, b"ping");
    assert_eq!(kio.import_calls(), 2);
}

#[test]
fn request_parts_are_copies() {
    let src = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
        let out = req.method() + " " + req.target() + " " + req.header_names()[0] + "=" + req.header_values()[0];
        res.send(204, [], [], out.to_bytes())
    }"#;
    let (_, sent) = serve(src);
    assert_eq!(sent.unwrap().body, b"POST /echo?x=1 host=localhost");
}

#[test]
fn report_lists_sorted_methods() {
    let mut kio = Kio::new(Limits::default()).unwrap();
    let (outcome, _) = kio.run_web(ECHO, vec![], request(b"")).unwrap();
    assert_eq!(outcome.unwrap().report.lines, ["main : req body, res send"]);
}

#[test]
fn second_send_is_already_sent() {
    let src = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
        res.send(200, [], [], "one".to_bytes())?;
        res.send(500, [], [], "two".to_bytes())
    }"#;
    let (value, sent) = serve(src);
    assert_eq!(value, "WebErr::AlreadySent");
    assert_eq!(sent.unwrap().body, b"one");
}

#[test]
fn bad_responses_are_invalid() {
    for (status, name, value) in [
        (200, "x\ry", "v"),
        (200, "x", "a\\nb"),
        (99, "x", "v"),
        (101, "x", "v"),
        (600, "x", "v"),
        (-1, "x", "v"),
    ] {
        let src = format!(
            "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {{ res.send({status}, [\"{name}\"], [\"{value}\"], \"\".to_bytes()) }}"
        );
        let mut kio = Kio::new(Limits::default()).unwrap();
        let (outcome, sent) = kio.run_web(&src, vec![], request(b"")).unwrap();
        assert_eq!(outcome.unwrap().value, "WebErr::Invalid", "{src}");
        assert!(sent.is_none(), "{src}");
    }
}

#[test]
fn storing_the_request_is_rejected() {
    let src =
        "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> { let r = req; res.send(200, [], [], r.body()) }";
    let mut kio = Kio::new(Limits::default()).unwrap();
    let (outcome, sent) = kio.run_web(src, vec![], request(b"")).unwrap();
    let diags = outcome.unwrap_err();
    assert!(diags[0].hint.is_some(), "{diags:?}");
    assert!(sent.is_none());
}

#[test]
fn body_bytes_count_against_io() {
    let limits = Limits {
        io: 5,
        ..Limits::default()
    };
    let mut kio = Kio::new(limits).unwrap();
    // `body` reads 4 bytes and `send` writes 4 more: 8 > 5.
    let e = kio.run_web(ECHO, vec![], request(b"ping")).unwrap_err();
    assert!(kio.explain(&e).unwrap().contains("too much I/O"));
}
