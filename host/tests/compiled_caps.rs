//! C4: capabilities in compiled code. Grants are checked by the host as on the interpreter
//! path; capability calls go through the same host functions, quotas and report check; a
//! compiled program cannot use a handle it was not granted.

use kio_host::kv::Store;
use kio_host::{open_dir, Origin, RunResult};
use kio_host::{Interp, Kio, Limits};
use lang::ir::{ExprKind, LocalId, Module};
use std::sync::Arc;

/// A temporary directory holding `files`.
fn dir(files: &[(&str, &str)]) -> (tempfile::TempDir, cap_std::fs::Dir) {
    let t = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(t.path().join(name), text).unwrap();
    }
    let d = open_dir(t.path()).unwrap();
    (t, d)
}

/// The first diagnostic: message, hint and span.
type Diag = (String, Option<String>, usize, usize);

fn compiled_diag(r: RunResult) -> Diag {
    let d = &r.unwrap_err()[0];
    (d.message.clone(), d.hint.clone(), d.start as usize, d.end as usize)
}

const TWO_DIRS: &str = "fn main(input: Dir, out: Dir) -> Int { 1 }";

/// Every grant error gives the interpreter path's diagnostic, and no host call is made.
#[test]
fn grant_errors_match_the_interpreter() {
    let cases: [&[&str]; 5] = [
        &["input", "out", "out"],
        &["input", "out", "extra"],
        &["input"],
        &[],
        &["out", "input", "input", "x"],
    ];
    for names in cases {
        let grants = || names.iter().map(|n| (n.to_string(), dir(&[]).1)).collect::<Vec<_>>();
        let mut c = Kio::new(Limits::default()).unwrap();
        let got = compiled_diag(c.run_with(TWO_DIRS, grants(), vec![]).unwrap());
        assert_eq!(c.import_calls(), 0);
        let mut k = Interp::new(Limits::default()).unwrap();
        let d = &k.run(TWO_DIRS, grants()).unwrap().unwrap_err()[0];
        let want = (d.message.clone(), d.hint.clone(), d.start as usize, d.end as usize);
        assert_eq!(got, want, "{names:?}");
    }
    let mut c = Kio::new(Limits::default()).unwrap();
    let missing = compiled_diag(
        c.run_with(TWO_DIRS, vec![("input".into(), dir(&[]).1)], vec![])
            .unwrap(),
    );
    assert_eq!(
        missing,
        (
            "no grant for `out: Dir`".into(),
            Some("grant it: `--dir out=path`".into()),
            20,
            28
        )
    );
}

/// A grant of the wrong kind is no grant: an `Http` named `input` does not satisfy `input: Dir`.
#[test]
fn a_grant_of_the_wrong_kind_is_refused() {
    let src = "fn main(input: Dir) -> Int { 1 }";
    let http = || vec![("input".to_string(), Origin::parse("http://127.0.0.1:1").unwrap())];
    let got = compiled_diag(
        Kio::new(Limits::default())
            .unwrap()
            .run_with(src, vec![], http())
            .unwrap(),
    );
    let mut k = Interp::new(Limits::default()).unwrap();
    let d = &k.run_with(src, vec![], http()).unwrap().unwrap_err()[0];
    assert_eq!(got.0, "no grant for `input: Dir`");
    assert_eq!(
        got,
        (d.message.clone(), d.hint.clone(), d.start as usize, d.end as usize)
    );

    let kv = |c: &mut Kio| c.grant_kvs(vec![("input".into(), Arc::new(Store::memory(1 << 20)))]);
    let mut c = Kio::new(Limits::default()).unwrap();
    kv(&mut c);
    assert_eq!(
        compiled_diag(c.run_with(src, vec![], vec![]).unwrap()).0,
        "no grant for `input: Dir`"
    );
}

/// The primer, compiled, with real directories: the value, the file it wrote, and what a
/// missing input gives.
#[test]
fn the_primer_runs_compiled() {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let (_a, input) = dir(&[("invoices.csv", "a,1\nb,2\nbad\n")]);
    let (b, out) = dir(&[]);
    let mut c = Kio::new(Limits::default()).unwrap();
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    let o = c.run_with(&src, grants, vec![]).unwrap().unwrap();
    assert_eq!((o.value.as_str(), o.failed), ("Ok(())", false));
    assert_eq!(std::fs::read_to_string(b.path().join("total.txt")).unwrap(), "3");
    assert_eq!(c.import_calls(), 2);

    let (_a, input) = dir(&[]);
    let (_b, out) = dir(&[]);
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    let o = c.run_with(&src, grants, vec![]).unwrap().unwrap();
    assert_eq!((o.value.as_str(), o.failed), ("IoErr::NotFound", true));
}

/// Every `Dir` method's values, including `List<Str>` and `Bool` results and each error.
#[test]
fn dir_values_match_the_interpreter() {
    let src = r#"struct R {
        a: Option<Str>, b: Result<Bytes, IoErr>, c: Result<(), IoErr>, e: Result<(), IoErr>,
        f: Result<List<Str>, IoErr>, g: Result<Bool, IoErr>, h: Result<(), IoErr>,
        i: Result<Bytes, IoErr>, j: Result<Bytes, IoErr>,
    }
    fn main(d: Dir) -> R {
        let a: Option<Str> = match d.read("a.txt") { Ok(b) => Some(b.to_str()), Err(_) => None };
        let b = d.read("../x");
        let c = d.create("a.txt", "x".to_bytes());
        let e = d.write("new.txt", "".to_bytes());
        let f = d.list("");
        let g = d.exists("new.txt");
        let h = d.remove("nope");
        let i = d.read("");
        let j = d.read("new.txt");
        R { a: a, b: b, c: c, e: e, f: f, g: g, h: h, i: i, j: j }
    }"#;
    let run = |compiled: bool| {
        let (_t, d) = dir(&[("a.txt", "héllo")]);
        let grants = vec![("d".to_string(), d)];
        if compiled {
            Kio::new(Limits::default())
                .unwrap()
                .run_with(src, grants, vec![])
                .unwrap()
                .unwrap()
                .value
        } else {
            Interp::new(Limits::default())
                .unwrap()
                .run(src, grants)
                .unwrap()
                .unwrap()
                .value
        }
    };
    let v = run(true);
    assert_eq!(v, run(false));
    assert_eq!(
        v,
        r#"R { a: Some("héllo"), b: Err(IoErr::InvalidPath), c: Err(IoErr::Exists), e: Ok(()), f: Ok(["a.txt", "new.txt"]), g: Ok(true), h: Err(IoErr::NotFound), i: Err(IoErr::InvalidPath), j: Ok(<0 bytes>) }"#
    );
}

/// `Http` against an origin that refuses connections: `Err(HttpErr::Denied)` both ways, after
/// the host checked the path.
#[test]
fn http_errors_match_the_interpreter() {
    let src = r#"fn main(h: Http) -> List<Result<Bytes, HttpErr>> { [h.get("/x"), h.get("x")] }"#;
    let o = || vec![("h".to_string(), Origin::parse("http://127.0.0.1:1").unwrap())];
    let mut c = Kio::new(Limits::default()).unwrap();
    let got = c.run_with(src, vec![], o()).unwrap().unwrap().value;
    let want = Interp::new(Limits::default())
        .unwrap()
        .run_with(src, vec![], o())
        .unwrap()
        .unwrap()
        .value;
    assert_eq!((got.as_str(), c.import_calls()), (want.as_str(), 2));
    assert_eq!(got, "[Err(HttpErr::Denied), Err(HttpErr::Denied)]");
}

/// H16, H16b: `send` and `send_all` against an origin that refuses connections: the host's
/// checks (method, grant, headers, batch size) and the connection failure agree both ways, and
/// a response struct and a batch's list are built the same.
#[test]
fn http_send_matches_the_interpreter() {
    let src = r#"fn main(h: Http) -> List<Result<HttpResponse, HttpErr>> {
        let req = |m: Str, p: Str| HttpRequest { method: m, path: p, headers: ["a: b"], body: "x".to_bytes(), timeout_ms: 1 };
        let mut out = [
            h.send("GET", "/x", [], "".to_bytes(), 0),
            h.send("POST", "/x", [], "".to_bytes(), 0),
            h.send("PUT", "/x", ["bad"], "".to_bytes(), 0),
            h.send("get", "/x", [], "".to_bytes(), 0),
            h.send("GET", "x", [], "".to_bytes(), 0),
        ];
        for r in h.send_all([req("GET", "/a"), req("DELETE", "/b"), req("PUT", "/c"), req("GET", "nope")]) { out.push(r); }
        let mut many: List<HttpRequest> = [];
        for i in 0..65 { many.push(req("GET", "/")); }
        for r in h.send_all(many).slice(0, 2) { out.push(r); }
        out
    }"#;
    let o = || {
        let mut origin = Origin::parse("http://127.0.0.1:1").unwrap();
        origin.allow_methods("PUT").unwrap();
        vec![("h".to_string(), origin)]
    };
    let mut c = Kio::new(Limits::default()).unwrap();
    let got = c.run_with(src, vec![], o()).unwrap().unwrap().value;
    let mut i = Interp::new(Limits::default()).unwrap();
    let want = i.run_with(src, vec![], o()).unwrap().unwrap().value;
    assert_eq!((got.as_str(), c.import_calls()), (want.as_str(), i.import_calls()));
    assert_eq!(c.import_calls(), 5 + 4 + 1);
    let d = "Err(HttpErr::Denied)";
    let v = "Err(HttpErr::Invalid)";
    assert_eq!(got, format!("[{d}, {d}, {v}, {v}, {d}, {d}, {d}, {d}, {d}, {v}, {v}]"));
}

/// Quotas and the report check stop a compiled run with the interpreter's message.
#[test]
fn quotas_and_the_report_check_stop_a_compiled_run() {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let run = |limits: Limits, hide: bool| {
        let (_a, input) = dir(&[("invoices.csv", "a,1\n")]);
        let (_b, out) = dir(&[]);
        let mut c = Kio::new(limits).unwrap();
        if hide {
            c.hide_from_report("out", "create");
        }
        let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
        format!("{:#}", c.run_with(&src, grants, vec![]).unwrap_err())
    };
    let calls = Limits {
        calls: 1,
        ..Limits::default()
    };
    assert!(run(calls, false).starts_with("too many host calls (limit 1; raise it with --calls)"));
    let io = Limits {
        io: 2,
        ..Limits::default()
    };
    assert!(run(io, false).starts_with("too much I/O (limit 0 MB; raise it with --io)"));
    let e = run(Limits::default(), true);
    assert!(
        e.starts_with("audit: call `out.create` not in the authority report (a checker bug)"),
        "{e}"
    );
}

/// The run's deadline bounds a host call that blocks.
#[test]
fn the_deadline_stops_a_blocked_host_call() {
    let (_t, d) = dir(&[]);
    let limits = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let src = r#"fn main(d: Dir) -> Result<Bytes, IoErr> { d.read("pipe") }"#;
    let start = std::time::Instant::now();
    let e = Kio::new(limits)
        .unwrap()
        .run_with(src, vec![("d".into(), d)], vec![])
        .unwrap_err();
    assert!(format!("{e:#}").starts_with("timed out after 1 s"), "{e:#}");
    // Compiling is not charged to the deadline, so under load the whole run can take longer;
    // the bound only shows the blocked read did not hang the run.
    assert!(start.elapsed().as_secs() < 30);
}

/// `Kv` values and errors, compiled, as interpreted.
#[test]
fn kv_values_match_the_interpreter() {
    let src = r#"struct R {
        a: Result<(), KvErr>, b: Result<Option<Bytes>, KvErr>, c: Result<Option<Bytes>, KvErr>,
        d: Result<Bool, KvErr>, e: Result<Bool, KvErr>, f: Result<List<Str>, KvErr>,
        g: Result<(), KvErr>, h: Result<(), KvErr>,
    }
    fn main(store: Kv) -> R {
        let a = store.set("k", "v".to_bytes());
        let b = store.get("k");
        let c = store.get("none");
        let d = store.swap("k", Some("v".to_bytes()), None);
        let e = store.swap("k", None, Some("w".to_bytes()));
        let f = store.keys("");
        let g = store.delete("k");
        let h = store.set("", "x".to_bytes());
        R { a: a, b: b, c: c, d: d, e: e, f: f, g: g, h: h }
    }"#;
    let kvs = || vec![("store".to_string(), Arc::new(Store::memory(1 << 20)))];
    let mut c = Kio::new(Limits::default()).unwrap();
    c.grant_kvs(kvs());
    let got = c.run_with(src, vec![], vec![]).unwrap().unwrap().value;
    let mut k = Interp::new(Limits::default()).unwrap();
    k.grant_kvs(kvs());
    let want = k.run(src, vec![]).unwrap().unwrap().value;
    assert_eq!(got, want);
    assert!(
        got.starts_with(
            r#"R { a: Ok(()), b: Ok(Some(<1 bytes>)), c: Ok(None), d: Ok(true), e: Ok(true), f: Ok(["k"])"#
        ),
        "{got}"
    );
}

/// The lowered body of `main`'s first statement-free tail call `f(..)`: its arguments.
fn tail_args(m: &mut Module) -> &mut Vec<lang::ir::Expr> {
    let main = &mut m.funcs[m.main.0 as usize];
    match &mut main.body.tail.as_mut().unwrap().kind {
        ExprKind::Call(_, args) => args,
        k => panic!("{k:?}"),
    }
}

const FORGE: &str = r#"fn read(d: Dir) -> Result<Bytes, IoErr> { d.read("a.txt") }
fn main(d: Dir, h: Http) -> Result<Bytes, IoErr> { read(d) }"#;

/// A crafted program (its IR edited after the checker) that calls `Dir.read` with its `Http`
/// handle, or with a handle it was never given: the component's handle table refuses the call
/// before any host function runs.
#[test]
fn a_handle_not_granted_is_refused() {
    let edits: [fn(&mut Module); 3] = [
        |_| {},
        |m| tail_args(m)[0].kind = ExprKind::Local(LocalId(1)),
        |m| tail_args(m)[0].kind = ExprKind::Bool(false),
    ];
    for (i, edit) in edits.into_iter().enumerate() {
        let (_t, d) = dir(&[("a.txt", "secret")]);
        let h = Origin::parse("http://127.0.0.1:1").unwrap();
        let mut c = Kio::new(Limits::default()).unwrap();
        let r = c.run_edited(FORGE, vec![("d".into(), d)], vec![("h".into(), h)], edit);
        if i == 0 {
            assert_eq!(r.unwrap().unwrap().value, "Ok(<6 bytes>)");
            assert_eq!(c.import_calls(), 1);
        } else {
            let e = format!("{:#}", r.unwrap_err());
            let why = ["used with the wrong type", "unknown handle index 0"][i - 1];
            assert!(e.contains(why), "{e}");
            assert_eq!(c.import_calls(), 0, "no host function ran");
        }
    }
}

/// Every `Incoming` method and `send`'s values and errors, compiled, as interpreted; the
/// response is the same. A status that is not a `u16` never reaches the host.
#[test]
fn web_values_and_response_match_the_interpreter() {
    let src = r#"struct R { m: Str, t: Str, n: List<Str>, v: List<Str>, b: Bytes,
        big: Result<(), WebErr>, low: Result<(), WebErr>, ok: Result<(), WebErr>, again: Result<(), WebErr> }
    fn main(req: Incoming, res: Outgoing) -> R {
        let big = res.send(70000, [], [], "".to_bytes());
        let low = res.send(99, [], [], "".to_bytes());
        let ok = res.send(201, ["x-a"], [req.method()], req.body());
        let again = res.send(200, [], [], "".to_bytes());
        R { m: req.method(), t: req.target(), n: req.header_names(), v: req.header_values(), b: req.body(),
            big: big, low: low, ok: ok, again: again }
    }"#;
    let request = || kio_host::Request {
        method: "POST".into(),
        target: "/a?b=1".into(),
        header_names: vec!["host".into(), "x-n".into()],
        header_values: vec!["example".into(), "1".into()],
        body: b"ping".to_vec(),
    };
    let mut c = Kio::new(Limits::default()).unwrap();
    let (got, got_sent) = c.run_web(src, vec![], request()).unwrap();
    let mut k = Interp::new(Limits::default()).unwrap();
    let (want, want_sent) = k.run_web(src, vec![], request()).unwrap();
    let got = got.unwrap().value;
    assert_eq!(got, want.unwrap().value);
    assert_eq!(
        got,
        r#"R { m: "POST", t: "/a?b=1", n: ["host", "x-n"], v: ["example", "1"], b: <4 bytes>, big: Err(WebErr::Invalid), low: Err(WebErr::Invalid), ok: Ok(()), again: Err(WebErr::AlreadySent) }"#
    );
    assert_eq!(got_sent, want_sent);
    let sent = got_sent.unwrap();
    assert_eq!(
        (sent.status, sent.headers, sent.body),
        (201, vec![("x-a".into(), "POST".into())], b"ping".to_vec())
    );
    // `send(70000, ..)` made no host call: three sends, seven reads.
    assert_eq!(c.import_calls(), 3 + 7);
}

/// Every `WsMessage` and `WsSend` method, compiled, as interpreted; the frames are the same. A
/// close code that is not a `u16` never reaches the host.
#[test]
fn ws_values_and_frames_match_the_interpreter() {
    let src = r#"struct R { k: Str, c: Int, d: Bytes, t: Result<(), WsErr>, b: Result<(), WsErr>,
        big: Result<(), WsErr>, bad: Result<(), WsErr>, close: Result<(), WsErr>, after: Result<(), WsErr>,
        tg: Str, h: Option<Str>, none: Option<Str> }
    fn main(m: WsMessage, w: WsSend) -> R {
        let t = w.text(m.kind());
        let b = w.binary(m.data());
        let big = w.close(70000);
        let bad = w.close(1005);
        let close = w.close(1000);
        let after = w.text("late");
        R { k: m.kind(), c: m.conn(), d: m.data(), t: t, b: b, big: big, bad: bad, close: close, after: after,
            tg: m.target(), h: m.header("X-Token"), none: m.header("cookie") }
    }"#;
    let event = || kio_host::ws::WsEvent {
        kind: "text",
        conn: 7,
        data: b"hi".to_vec(),
        upgrade: std::sync::Arc::new(kio_host::ws::Upgrade {
            target: "/a%20b?q=1".into(),
            headers: vec![("x-token".into(), "s3cret".into()), ("x-token".into(), "second".into())],
        }),
    };
    let names = ("m".to_string(), "w".to_string());
    let mut c = Kio::new(Limits::default()).unwrap();
    let (got, got_frames) = c.run_ws(src, vec![], event(), false, &names).unwrap();
    let mut k = Interp::new(Limits::default()).unwrap();
    let (want, want_frames) = k.run_ws(src, vec![], event(), false, &names).unwrap();
    let got = got.unwrap().value;
    assert_eq!(got, want.unwrap().value);
    assert_eq!(
        got,
        r#"R { k: "text", c: 7, d: <2 bytes>, t: Ok(()), b: Ok(()), big: Err(WsErr::Invalid), bad: Err(WsErr::Invalid), close: Ok(()), after: Err(WsErr::Closed), tg: "/a%20b?q=1", h: Some("s3cret"), none: None }"#
    );
    assert_eq!(got_frames, want_frames);
    use kio_host::ws::WsFrame;
    assert_eq!(
        got_frames,
        [
            WsFrame::Text("text".into()),
            WsFrame::Binary(b"hi".to_vec()),
            WsFrame::Close(1000)
        ]
    );
}

/// A run past `--memory` reads as it does for `Interp`, not as an internal error.
#[test]
fn out_of_memory_is_explained() {
    let limits = Limits {
        memory: 4 << 20,
        ..Limits::default()
    };
    let src = r#"fn main() -> Int { let mut s = "x"; for i in 0..30 { s = s + s; } s.len() }"#;
    let e = Kio::new(limits).unwrap().run(src, vec![]).unwrap_err();
    assert!(
        format!("{e:#}").starts_with("out of memory (limit 4 MB; raise it with --memory)"),
        "{e:#}"
    );
}
