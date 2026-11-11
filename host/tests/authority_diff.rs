//! `kio check NEW --base OLD`: the authority diff, its output and exit codes (P1).
use std::path::Path;
use std::process::Command;

/// Write `files` into a new directory under `root` named `side`; the first is the entry.
fn side(root: &Path, side: &str, files: &[(&str, &str)]) -> String {
    let dir = root.join(side);
    std::fs::create_dir(&dir).unwrap();
    for (name, src) in files {
        std::fs::write(dir.join(name), src).unwrap();
    }
    dir.join(files[0].0).to_str().unwrap().to_string()
}

/// `kio check new --base old`: exit code, stdout, stderr.
fn diff(old: &[(&str, &str)], new: &[(&str, &str)]) -> (i32, String, String) {
    diff_with(old, new, &[])
}

/// `kio check new --base old` followed by `flags`.
fn diff_with(old: &[(&str, &str)], new: &[(&str, &str)], flags: &[&str]) -> (i32, String, String) {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = (side(t.path(), "old", old), side(t.path(), "new", new));
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["check", &new, "--base", &old])
        .args(flags)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (out.status.code().unwrap(), s(out.stdout), s(out.stderr))
}

const OLD: &str = r#"
fn delete(links: Kv, log: Log) -> Result<(), KvErr> { links.delete("k")?; let _w = log.write("gone"); Ok(()) }
fn main(links: Kv, log: Log) -> Result<(), KvErr> { delete(links, log) }
"#;

#[test]
fn no_change_exits_0() {
    assert_eq!(
        diff(&[("app.kio", OLD)], &[("app.kio", OLD)]),
        (0, "no change in authority\n".into(), "".into())
    );
}

#[test]
fn added_method_exits_3() {
    let new = OLD.replace(
        r#"links.delete("k")?;"#,
        r#"links.delete("k")?; let _k = links.keys("")?;"#,
    );
    let want = "~ links: Kv (+keys)\nauthority added (exit 3); --detail lists each function\n";
    assert_eq!(
        diff(&[("app.kio", OLD)], &[("app.kio", &new)]),
        (3, want.into(), "".into())
    );
    let want = "~ links: Kv (+keys)\n\n+ delete: links keys\n+ main: links keys\nauthority added (exit 3)\n";
    assert_eq!(
        diff_with(&[("app.kio", OLD)], &[("app.kio", &new)], &["--detail"]),
        (3, want.into(), "".into())
    );
}

#[test]
fn removed_method_exits_0() {
    let new = OLD.replace(r#"let _w = log.write("gone"); "#, "");
    let want = "~ log: Log (-write)\nauthority removed only\n";
    assert_eq!(
        diff(&[("app.kio", OLD)], &[("app.kio", &new)]),
        (0, want.into(), "".into())
    );
}

#[test]
fn function_lines_shown_when_no_grant_line_shows_the_addition() {
    // `main` never calls `audit`: no grant changes, yet authority is added (exit 3 as before).
    let new = format!("{OLD}fn audit(links: Kv) -> Int {{ let _k = links.keys(\"\"); 0 }}");
    let want = "+ audit: links keys\nauthority added (exit 3)\n";
    assert_eq!(
        diff(&[("app.kio", OLD)], &[("app.kio", &new)]),
        (3, want.into(), "".into())
    );
}

#[test]
fn detail_on_no_change_is_no_change() {
    assert_eq!(
        diff_with(&[("app.kio", OLD)], &[("app.kio", OLD)], &["--detail"]),
        (0, "no change in authority\n".into(), "".into())
    );
}

#[test]
fn base_that_fails_to_check_exits_1_and_names_the_base() {
    let (code, out, err) = diff(&[("app.kio", "fn main() -> Int { nope }")], &[("app.kio", OLD)]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(
        err.starts_with("kio: the base program ") && err.contains("old/app.kio does not check"),
        "{err}"
    );
    assert!(err.contains("app.kio:1: error:"), "{err}");
}

#[test]
fn new_that_fails_to_check_exits_1_and_names_the_new_side() {
    let (code, _, err) = diff(&[("app.kio", OLD)], &[("app.kio", "fn main() -> Int { nope }")]);
    assert_eq!(code, 1);
    assert!(
        err.starts_with("kio: the new program ") && err.contains("new/app.kio does not check"),
        "{err}"
    );
}

#[test]
fn a_real_call_after_a_shadowing_baseline_exits_3() {
    // The baseline's `api.get` is a local `Map`'s, so the real `Http.get` is added authority.
    let shadow = r#"{ let api: Map<Str, Int> = Map::new(); let _v = api.get("x"); }"#;
    let old = format!("fn main(api: Http) {shadow}");
    let new = format!(r#"fn main(api: Http) {{ {shadow} let _r = api.get("/x"); }}"#);
    let want = "~ api: Http (+get)\nauthority added (exit 3); --detail lists each function\n";
    assert_eq!(
        diff(&[("app.kio", &old)], &[("app.kio", &new)]),
        (3, want.into(), "".into())
    );
}

/// The shape of kio-web's links app: a handler, a module with the HTTP helper, and a webhook
/// added to `main` as `hook: Http`. Modules resolve beside each side's entry.
const LINKS_OLD: &str = r#"mod http;
fn create(links: Kv, log: Log) -> Result<Str, KvErr> { links.set("link/a", "x".to_bytes())?; let _w = log.write("made a"); Ok("a") }
fn main(req: Incoming, res: Outgoing, links: Kv, log: Log) -> Result<(), WebErr> {
    let _b = req.body();
    let made = create(links, log);
    res.send(201, [], [], "".to_bytes())?;
    if let Ok(_slug) = made {}
    Ok(())
}
"#;
const HTTP: &str = r#"pub fn post(api: Http, path: Str, body: Str) -> Result<(), Str> {
    match api.send("POST", path, ["content-type: application/json"], body.to_bytes(), 0) { Ok(_r) => Ok(()), Err(_e) => Err("failed") }
}
"#;

#[test]
fn links_webhook_adds_exactly_hook_send() {
    let new = LINKS_OLD
        .replace(
            "log: Log) -> Result<(), WebErr>",
            "log: Log, hook: Http) -> Result<(), WebErr>",
        )
        .replace(
            "if let Ok(_slug) = made {}",
            r#"if let Ok(slug) = made { let _r = http::post(hook, "/", slug); }"#,
        );
    let (code, out, err) = diff(
        &[("app.kio", LINKS_OLD), ("http.kio", HTTP)],
        &[("app.kio", &new), ("http.kio", HTTP)],
    );
    let want = "+ hook: Http (send)\nauthority added (exit 3); --detail lists each function\n";
    assert_eq!((code, out.as_str(), err.as_str()), (3, want, ""));
}

/// kio-web payhook's step 2: a new `Hmac` grant reaching `verify` through `receive`, `handle`
/// and `check_signature`, and a `Clock`. The reviewer reads two lines; `--detail` adds the four
/// functions.
const PAY_OLD: &str = r#"fn handle(req: Incoming) -> Int { req.body().len() }
fn receive(req: Incoming) -> Int { handle(req) }
fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> { let _n = receive(req); res.send(200, [], [], "ok".to_bytes()) }
"#;
const PAY_NEW: &str = r#"fn check_signature(stripe: Hmac, body: Bytes) -> Bool { stripe.verify(body, "".to_bytes()) }
fn handle(req: Incoming, stripe: Hmac, now: Clock) -> Int {
    if now.now_ms() < 0 || !check_signature(stripe, req.body()) { return 0; }
    req.body().len()
}
fn receive(req: Incoming, stripe: Hmac, now: Clock) -> Int { handle(req, stripe, now) }
fn main(req: Incoming, res: Outgoing, stripe: Hmac, now: Clock) -> Result<(), WebErr> { let _n = receive(req, stripe, now); res.send(200, [], [], "ok".to_bytes()) }
"#;

#[test]
fn payhook_step_2_is_two_lines() {
    let summary = "+ stripe: Hmac (verify)\n+ now: Clock (now_ms)\n";
    let want = format!("{summary}authority added (exit 3); --detail lists each function\n");
    let (old, new) = ([("app.kio", PAY_OLD)], [("app.kio", PAY_NEW)]);
    assert_eq!(diff(&old, &new), (3, want, "".into()));
    let funcs = "+ check_signature: stripe verify\n+ handle: stripe verify, now now_ms\n+ main: stripe verify, now now_ms\n+ receive: stripe verify, now now_ms\n";
    let want = format!("{summary}\n{funcs}authority added (exit 3)\n");
    assert_eq!(diff_with(&old, &new, &["--detail"]), (3, want, "".into()));
}
