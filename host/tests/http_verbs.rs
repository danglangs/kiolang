//! P7: `kio run`, `--listen` and `kio check --grants` refuse a program whose authority report
//! shows a literal verb `--http-methods` does not grant; `kio check --base` sees the verb.
use std::path::Path;
use std::process::Command;

/// `(exit code, stdout, stderr)` of `kio args..`.
fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

const API: &str = "api=http://127.0.0.1:9";

fn send(verb: &str) -> String {
    format!(r#"match api.send({verb}, "/", [], "x".to_bytes(), 0) {{ Ok(r) => r.status, Err(_) => 0 }}"#)
}

/// Each way of starting `src`'s call: `kio run`, `kio check --grants` and `--listen`, with
/// `flags` after the `--http` grant; each refuses with `refusal` (or, `None`, does not refuse).
fn starts(t: &Path, call: &str, flags: &[&str], refusal: Option<&str>) {
    let prog = write(t, "p.kio", &format!("fn main(api: Http) -> Int {{ {call} }}"));
    let mut args = vec!["--http", API];
    args.extend(flags);
    let (code, out, err) = kio(&[&["run", &prog][..], &args].concat());
    match refusal {
        Some(r) => assert_eq!((code, out.as_str(), err.as_str()), (2, "", r)),
        // Not refused at start: the request reaches the closed port and fails.
        None => assert_eq!((code, out.as_str()), (0, "0\n"), "{err}"),
    }
    let lines: Vec<String> = args.chunks(2).map(|f| f.join(" ") + "\n").collect();
    let g = write(t, "p.grants", &lines.concat());
    let (code, _, err) = kio(&["check", &prog, "--grants", &g]);
    match refusal {
        Some(r) => assert_eq!((code, err.as_str()), (2, r)),
        None => assert_eq!(code, 0, "{err}"),
    }
    if let Some(r) = refusal {
        let app = write(
            t,
            "app.kio",
            &format!(
                "fn main(req: Incoming, res: Outgoing, api: Http) -> Result<(), WebErr> {{ let _s = {call}; res.send(200, [], [], \"ok\".to_bytes()) }}"
            ),
        );
        let listen = [&["run", &app, "--listen", "127.0.0.1:0"][..], &args].concat();
        let (code, _, err) = kio(&listen);
        assert_eq!((code, err.as_str()), (2, r));
    }
}

#[test]
fn a_literal_verb_not_granted_is_refused_at_start() {
    let t = tempfile::tempdir().unwrap();
    starts(
        t.path(),
        &send(r#""PUT""#),
        &["--http-methods", "api=POST"],
        Some("kio: `api` sends PUT but `--http-methods api=POST` grants only POST\n  hint: add PUT to --http-methods, or remove the call\n"),
    );
    starts(
        t.path(),
        &send(r#""DELETE""#),
        &[],
        Some("kio: `api` sends DELETE but `--http api` grants only GET\n  hint: add `--http-methods api=DELETE`, or remove the call\n"),
    );
    // `send_all`'s requests count; the message names the flag as given.
    let all = r#"match api.send_all([HttpRequest { method: "PATCH", path: "/", headers: [], body: "".to_bytes(), timeout_ms: 0 }])[0] { Ok(r) => r.status, Err(_) => 0 }"#;
    starts(
        t.path(),
        all,
        &["--http-methods", "api=post,put"],
        Some("kio: `api` sends PATCH but `--http-methods api=post,put` grants only POST,PUT\n  hint: add PATCH to --http-methods, or remove the call\n"),
    );
}

#[test]
fn granted_get_and_unseen_verbs_start() {
    let t = tempfile::tempdir().unwrap();
    starts(t.path(), &send(r#""PUT""#), &["--http-methods", "api=PUT"], None);
    starts(t.path(), &send(r#""GET""#), &[], None);
    // `*` is checked per request at run time, as before.
    starts(
        t.path(),
        &format!(r#"{{ let m = "PU" + "T"; {} }}"#, send("m")),
        &[],
        None,
    );
}

#[test]
fn the_35d_shape_exits_3_under_check_base() {
    let t = tempfile::tempdir().unwrap();
    let all = r#"let _a = api.send_all([HttpRequest { method: "POST", path: "/r", headers: [], body: "".to_bytes(), timeout_ms: 0 }]);"#;
    let old = write(t.path(), "old.kio", &format!("fn main(api: Http) {{ {all} }}"));
    let new = write(
        t.path(),
        "new.kio",
        &format!(r#"fn main(api: Http) {{ {all} let _s = api.send("PUT", "/x", [], "".to_bytes(), 0); }}"#),
    );
    let want = "~ api: Http (+send PUT)\nauthority added (exit 3); --detail lists each function\n";
    assert_eq!(kio(&["check", &new, "--base", &old]), (3, want.into(), String::new()));
    let (_, out, _) = kio(&["check", &new, "--base", &old, "--detail"]);
    assert_eq!(
        out,
        "~ api: Http (+send PUT)\n\n+ main: api send PUT\nauthority added (exit 3)\n"
    );
    let (_, out, _) = kio(&["check", &new]);
    assert_eq!(out, "main : api send PUT send_all POST\n");
}

const REQ: &str = r#"HttpRequest { method: "M", path: "/", headers: [], body: "x".to_bytes(), timeout_ms: 0 }"#;

/// `g.send_all([HttpRequest { method: "M", .. }])` for grant `g` and verb `m`.
fn all(g: &str, m: &str) -> String {
    format!(
        "let _{g} = {g}.send_all([{}]);",
        REQ.replace("\"M\"", &format!("\"{m}\""))
    )
}

#[test]
fn each_send_all_has_its_own_verbs() {
    // The 35d attack on a second grant: `a` PUTs, as only `b` did before.
    let t = tempfile::tempdir().unwrap();
    let body = format!("{} {}", all("a", "POST"), all("b", "PUT"));
    let old = write(t.path(), "old.kio", &format!("fn main(a: Http, b: Http) {{ {body} }}"));
    let attack = r#"let _x = a.send("PUT", "/exfil", [], "s".to_bytes(), 0);"#;
    let new = write(
        t.path(),
        "new.kio",
        &format!("fn main(a: Http, b: Http) {{ {body} {attack} }}"),
    );
    let want = "~ a: Http (+send PUT)\nauthority added (exit 3); --detail lists each function\n";
    assert_eq!(kio(&["check", &new, "--base", &old]), (3, want.into(), String::new()));
    // Each grant needs only its own method to start.
    let b = format!(
        "fn main(a: Http, b: Http) -> Int {{ {} {} 0 }}",
        all("a", "POST"),
        all("b", "DELETE")
    );
    let prog = write(t.path(), "b.kio", &b);
    let (a_, b_) = ("a=http://127.0.0.1:9", "b=http://127.0.0.1:9");
    let flags = [
        "--http",
        a_,
        "--http",
        b_,
        "--http-methods",
        "a=POST",
        "--http-methods",
        "b=DELETE",
    ];
    let (code, out, err) = kio(&[&["run", &prog][..], &flags].concat());
    assert_eq!((code, out.as_str()), (0, "0\n"), "{err}");
    let (code, _, err) = kio(&[&["run", &prog][..], &flags[..6]].concat());
    assert_eq!(
        (code, err.as_str()),
        (2, "kio: `b` sends DELETE but `--http b` grants only GET\n  hint: add `--http-methods b=DELETE`, or remove the call\n")
    );
}

#[test]
fn a_capital_literal_that_is_not_a_method_is_refused() {
    let t = tempfile::tempdir().unwrap();
    starts(
        t.path(),
        &send(r#""PUTX""#),
        &["--http-methods", "api=PUT"],
        Some("kio: `api` sends PUTX, not an HTTP method\n  hint: a method is one of GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS\n"),
    );
}
