use kio_host::{open_dir, Limits, Request, Server};
use std::time::Instant;

fn get(target: &str) -> Request {
    Request {
        method: "GET".into(),
        target: target.into(),
        header_names: vec!["host".into()],
        header_values: vec!["localhost".into()],
        body: vec![],
    }
}

#[test]
fn server_is_send_and_sync() {
    fn is<T: Send + Sync>() {}
    is::<Server>();
}

#[test]
fn thousand_requests_get_the_right_body() {
    let read = |p: &str| std::fs::read_to_string(format!("../examples/serve/{p}")).unwrap();
    let files = vec![
        ("app.kio".to_string(), read("app.kio")),
        ("web.kio".to_string(), read("web.kio")),
    ];
    let server = Server::new(files, Limits::web()).unwrap().unwrap();
    assert!(server.report().contains("main"));
    let public = tempfile::tempdir().unwrap();
    let start = Instant::now();
    for _ in 0..1000 {
        let dirs = vec![("public".to_string(), open_dir(public.path()).unwrap())];
        let r = server.handle(get("/hello/kio"), dirs).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, br#"{"hello": "kio"}"#);
    }
    println!("per request: {:?}", start.elapsed() / 1000);
}

#[test]
fn out_of_fuel_is_a_500_and_the_next_request_works() {
    let src = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
        if req.target() == "/loop" { while true {} }
        res.send(200, [], [], "ok".to_bytes())
    }"#;
    let limits = Limits {
        fuel: 10_000_000,
        ..Limits::web()
    };
    let server = Server::new(vec![("main".into(), src.into())], limits).unwrap().unwrap();
    let r = server.handle(get("/loop"), vec![]).unwrap();
    assert_eq!((r.status, r.body.len()), (500, 0));
    let r = server.handle(get("/"), vec![]).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"ok"[..]));
}

#[test]
fn no_response_is_a_500() {
    let src = "fn main(req: Incoming, res: Outgoing) -> Int { 0 }";
    let server = Server::new(vec![("main".into(), src.into())], Limits::web())
        .unwrap()
        .unwrap();
    assert_eq!(server.handle(get("/"), vec![]).unwrap().status, 500);
}

#[test]
fn type_error_returns_the_diags() {
    let src = "fn main(req: Incoming, res: Outgoing) -> Int { \"x\" }";
    let Err(diags) = Server::new(vec![("main".into(), src.into())], Limits::web()).unwrap() else {
        panic!("expected diags");
    };
    assert!(!diags.is_empty());
}
