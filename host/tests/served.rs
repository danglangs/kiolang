//! W13c, kept for compiled servers: a `Server` runs exactly the program it checked and compiled,
//! whatever the characters in its sources and file names, and refuses cleanly what it cannot run.
use kio_host::{Limits, Request, Server};

fn get(target: &str) -> Request {
    Request {
        method: "GET".into(),
        target: target.into(),
        header_names: vec!["host".into()],
        header_values: vec!["localhost".into()],
        body: vec![],
    }
}

/// A program of filler functions, then `main` sending `lit` (an already escaped kio string
/// literal body) and carrying `comment` text.
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

/// Serve `files` and check two requests answer with `want`.
fn serve_exactly(files: Vec<(String, String)>, want: &[u8]) {
    let server = Server::new(files, Limits::web()).unwrap().unwrap();
    for _ in 0..2 {
        let r = server.handle(get("/"), vec![]).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, want), "wrong program");
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
        serve_exactly(vec![(name.to_string(), program("ok", "c"))], b"ok");
    }
}

#[test]
fn empty_file_is_refused_or_served() {
    let Ok(Err(diags)) = Server::new(one(String::new()), Limits::web()) else {
        panic!("an empty program has no main: expected diagnostics");
    };
    assert!(!diags.is_empty());
}

/// A source far larger than any program serves exactly it: nothing is copied per request.
#[test]
fn huge_file_is_served() {
    serve_exactly(one(program("big", &"x".repeat(5 << 20))), b"big");
}

/// A program with many functions compiles once and serves (W13c: 20,000 once exhausted the
/// guest interpreter's memory while checking inside the sandbox).
#[test]
fn many_functions_are_served() {
    // The host's check recurses deeply in a debug build; give it room.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(many_functions)
        .unwrap()
        .join()
        .unwrap();
}

fn many_functions() {
    let mut src = String::new();
    for i in 0..5000 {
        src += &format!("fn f{i}(a: Int, b: Int) -> Int {{\n    let c = a + b * {i};\n    c + 1\n}}\n");
    }
    src += "fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {\n    res.send(200, [], [], \"ok\".to_bytes())\n}\n";
    serve_exactly(one(src), b"ok");
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
