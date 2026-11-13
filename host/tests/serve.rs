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
    assert!(server.shown_report().contains("main"));
    let public = tempfile::tempdir().unwrap();
    let mut times = vec![];
    for _ in 0..1000 {
        let dirs = vec![("public".to_string(), open_dir(public.path()).unwrap())];
        let start = Instant::now();
        let r = server.handle(get("/hello/kio"), dirs).unwrap();
        times.push(start.elapsed());
        assert_eq!(r.status, 200);
        assert_eq!(r.body, br#"{"hello": "kio"}"#);
    }
    times.sort();
    // docs/perf.md records the release median: `cargo test --release --test serve -- --nocapture`.
    println!("Server::handle median {:?}, p99 {:?}", times[500], times[990]);
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

#[test]
fn program_hash_is_sha256_of_names_and_sources() {
    let f = |n: &str, s: &str| (n.to_string(), s.to_string());
    let h = kio_host::program_hash(&[f("a", "x"), f("b", "y")]);
    assert_eq!(h.len(), 64);
    assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(h, kio_host::program_hash(&[f("a", "x"), f("b", "y")]));
    assert_ne!(h, kio_host::program_hash(&[f("a", "x"), f("b", "z")]));
    assert_ne!(
        h,
        kio_host::program_hash(&[f("a", "xb"), f("", "y")]),
        "file boundaries count"
    );
}

mod upstream;

/// H19: every request resolves the granted host for itself, once, however many calls it makes:
/// no request connects to an address another request's lookup returned.
#[test]
fn http_dns_is_pinned_per_request() {
    let up = upstream::start();
    let src = r#"fn main(req: Incoming, res: Outgoing, up: Http) -> Result<(), WebErr> {
        let a = match up.get("/a") { Ok(b) => b.to_str(), Err(_) => "failed " };
        let b = match up.get("/b") { Ok(b) => b.to_str(), Err(_) => "failed " };
        res.send(200, [], [], (a + b).to_bytes())
    }"#;
    let mut origin = kio_host::Origin::parse(&up.grant.replace("127.0.0.1", "localhost")).unwrap();
    origin.allow_private_addrs();
    let mut server = Server::new(vec![("main".into(), src.into())], Limits::web())
        .unwrap()
        .unwrap();
    server.grant_https(vec![("up".into(), origin.clone())]);
    for n in 1..=3 {
        let r = server.handle(get("/"), vec![]).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"GET /a GET /b "[..]));
        assert_eq!(origin.lookups(), n);
    }
}
