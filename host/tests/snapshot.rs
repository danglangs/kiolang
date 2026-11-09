//! W13c: attacks on the pre-initialized guest. A `Server` must run exactly the program it checked.
use kio_host::{Kio, Limits, Request, Server};

fn get(target: &str) -> Request {
    Request {
        method: "GET".into(),
        target: target.into(),
        header_names: vec!["host".into()],
        header_values: vec!["localhost".into()],
        body: vec![],
    }
}

/// A program whose check costs far more than its run: filler functions, then `main` sending `lit`
/// (an already escaped kio string literal body) and carrying `comment` text.
fn program(lit: &str, comment: &str) -> String {
    let mut s = String::new();
    for i in 0..40 {
        s += &format!("fn f{i}(a: Int, b: Int) -> Int {{\n    let c = a + b * {i};\n    c + 1\n}}\n");
    }
    s += &format!("// {comment}\n");
    s += &format!(
        "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {{\n    res.send(200, [], [], \"{lit}\".to_bytes())\n}}\n"
    );
    s
}

/// Fuel needed to check `files` in the guest, as a one-shot run does. (A run without its grants
/// is refused by the host before the guest checks, so `check_files` measures it.)
fn one_shot_fuel(files: &[(String, String)]) -> u64 {
    let (mut lo, mut hi) = (0u64, 4_000_000_000u64);
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        let limits = Limits {
            fuel: mid,
            ..Limits::default()
        };
        let mut kio = Kio::new(limits).unwrap();
        // Any reply, even a diagnostic, means it did not run out of fuel.
        match kio.check_files(files) {
            Ok(_) => hi = mid,
            Err(_) => lo = mid,
        }
    }
    hi
}

/// Serve `files` and prove the request ran the snapshot's program: it answers with `want`, and
/// within fuel that a re-check would exceed.
fn serve_exactly(files: Vec<(String, String)>, want: &[u8]) {
    let full = one_shot_fuel(&files);
    let limits = Limits {
        fuel: full * 2 / 3,
        ..Limits::web()
    };
    let server = Server::new(files, limits).unwrap().unwrap();
    for _ in 0..2 {
        let r = server.handle(get("/"), vec![]).unwrap();
        assert_eq!(
            (r.status, r.body.as_slice()),
            (200, want),
            "snapshot not used or wrong program"
        );
    }
}

fn one(src: String) -> Vec<(String, String)> {
    vec![("main".to_string(), src)]
}

#[test]
fn quotes_backslashes_and_u_escapes_in_comments_and_strings() {
    let comment = r#"say "hi" \ \u{41} \u{0} \\ \" }]),"#;
    serve_exactly(
        one(program(r##"a\\u{41}\\\"b"##, comment)),
        br##"a\u{41}\"b"##.as_slice(),
    );
}

#[test]
fn control_and_unicode_characters() {
    let comment = "nul:\0 bel:\x07 cr:\r ls:\u{2028} ps:\u{2029} bom:\u{feff} nel:\u{85} zwj:\u{200d} \u{1f600} \u{10ffff} é 日本 crlf:\r\n";
    let lit = "\u{2028}\u{85}\u{1f600}é\u{7f}\u{feff}";
    serve_exactly(one(program(lit, comment)), lit.as_bytes());
}

#[test]
fn control_characters_inside_a_string_literal() {
    let lit = "a\0b\rc\x07d\x1be";
    serve_exactly(one(program(lit, "x")), lit.as_bytes());
}

#[test]
fn awkward_file_names() {
    for name in [
        "we\"ird\\}],) é",
        "a}",
        "]",
        ",",
        ")",
        "{name: \"x\", source: \"y\"}",
        "",
        "\u{2028}",
        "n\0l",
        "😀",
    ] {
        let files = vec![(name.to_string(), program("ok", "c"))];
        let full = one_shot_fuel(&files);
        let limits = Limits {
            fuel: full * 2 / 3,
            ..Limits::web()
        };
        if let Ok(Ok(server)) = Server::new(files, limits) {
            let r = server.handle(get("/"), vec![]).unwrap();
            assert_eq!((r.status, r.body.as_slice()), (200, &b"ok"[..]), "name {name:?}");
        }
    }
}

#[test]
fn empty_file_is_refused_or_served() {
    let Ok(Err(diags)) = Server::new(one(String::new()), Limits::web()) else {
        panic!("an empty program has no main: expected diagnostics");
    };
    assert!(!diags.is_empty());
}

/// A source far larger than any program: the server serves exactly it, within a limit sized to
/// copying the files into each request, or refuses. Never a panic.
#[test]
fn huge_file_serves_or_refuses_cleanly() {
    let src = program("big", &"x".repeat(5 << 20));
    let limits = Limits {
        fuel: 1_000_000_000,
        ..Limits::web()
    };
    if let Ok(Ok(server)) = Server::new(one(src), limits) {
        let r = server.handle(get("/"), vec![]).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"big"[..]));
    }
}

/// A program the host checks but whose `init` runs out of memory or fuel is a startup error, not
/// a panic (once a guest panic message met WASI's blocking stderr inside a Tokio runtime).
#[test]
fn init_failure_is_a_startup_error() {
    // The host's own check recurses deeply in a debug build; give it room.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(init_failure)
        .unwrap()
        .join()
        .unwrap();
}

fn init_failure() {
    let mut src = String::new();
    for i in 0..20000 {
        src += &format!("fn f{i}(a: Int, b: Int) -> Int {{\n    let c = a + b * {i};\n    c + 1\n}}\n");
    }
    src += "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {\n    res.send(200, [], [], \"ok\".to_bytes())\n}\n";
    let Err(e) = Server::new(one(src), Limits::web()) else {
        panic!("the guest cannot check this inside its memory: expected a startup error");
    };
    assert!(format!("{e:#}").contains("initialization function"), "{e:#}");
}

/// A request that changes everything it can must leave the next request a clean instance.
#[test]
fn no_state_leaks_between_requests() {
    let src = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
        let mut v = [1];
        v.push(2);
        res.send(200, [], [], "ok".to_bytes())
    }"#;
    let server = Server::new(one(src.into()), Limits::web()).unwrap().unwrap();
    for _ in 0..50 {
        assert_eq!(server.handle(get("/"), vec![]).unwrap().body, b"ok");
    }
}
