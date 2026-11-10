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
    let t = tempfile::tempdir().unwrap();
    let (old, new) = (side(t.path(), "old", old), side(t.path(), "new", new));
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["check", &new, "--base", &old])
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
    let want = "+ delete: links keys\n+ main: links keys\nauthority added: 2 lines (exit 3)\n";
    assert_eq!(
        diff(&[("app.kio", OLD)], &[("app.kio", &new)]),
        (3, want.into(), "".into())
    );
}

#[test]
fn removed_method_exits_0() {
    let new = OLD.replace(r#"let _w = log.write("gone"); "#, "");
    let want = "- delete: log write\n- main: log write\nauthority removed only\n";
    assert_eq!(
        diff(&[("app.kio", OLD)], &[("app.kio", &new)]),
        (0, want.into(), "".into())
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
    match api.send("POST", path, ["content-type: application/json"], body.to_bytes()) { Ok(_r) => Ok(()), Err(_e) => Err("failed") }
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
    let want = "+ main: grant hook: Http\n+ main: hook send\nauthority added: 2 lines (exit 3)\n";
    assert_eq!((code, out.as_str(), err.as_str()), (3, want, ""));
}
