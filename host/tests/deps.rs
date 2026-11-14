//! PK1: `kio.deps` beside the root loads a module from a path and pins its sha256.
use std::path::{Path, PathBuf};
use std::process::Command;

/// `(exit code, stdout, stderr)` of `kio args..` run in `cwd`.
fn kio(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

fn sha(text: &str) -> String {
    lang::digest::to_hex(&lang::digest::sha256(text.as_bytes()))
}

const WEB: &str = "pub fn inc(n: Int) -> Int {\n    n + 1\n}\n";
const APP: &str = "mod web;\nfn main() -> Int {\n    web::inc(1)\n}\n";

/// `lib/web.kio` and `app/app.kio`, with `app/kio.deps` holding `deps`; returns the root dir
/// (canonical) and `app/`.
fn project(app: &str, deps: &str) -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("lib")).unwrap();
    std::fs::create_dir(root.join("app")).unwrap();
    std::fs::write(root.join("lib/web.kio"), WEB).unwrap();
    std::fs::write(root.join("app/app.kio"), app).unwrap();
    std::fs::write(root.join("app/kio.deps"), deps).unwrap();
    (t, root.join("app"))
}

fn pinned() -> String {
    format!("# kio.deps\nweb  ../lib/web.kio  sha256:{}\n", sha(WEB))
}

#[test]
fn a_listed_module_loads_from_its_path_and_others_from_beside_the_root() {
    let app = "mod web;\nmod util;\nfn main() -> Int {\n    web::inc(util::two())\n}\n";
    // A listed name no `mod` declares is no error, and its file is not read.
    let deps = pinned() + "other  ../lib/missing.kio\n";
    let (_t, dir) = project(app, &deps);
    std::fs::write(dir.join("util.kio"), "pub fn two() -> Int {\n    2\n}\n").unwrap();
    assert_eq!(kio(&dir, &["run", "app.kio"]), (0, "3\n".into(), "".into()));
    let (code, out, err) = kio(&dir, &["check", "app.kio"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(
        out.ends_with(&format!("\n\ndep web ../lib/web.kio sha256:{}\n", sha(WEB))),
        "{out}"
    );
    let (code, out, _) = kio(&dir, &["check", "app.kio", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["deps"],
        serde_json::json!([{"name": "web", "path": "../lib/web.kio", "sha256": sha(WEB)}])
    );
    // No kio.deps is an empty one.
    std::fs::remove_file(dir.join("kio.deps")).unwrap();
    std::fs::copy(dir.join("../lib/web.kio"), dir.join("web.kio")).unwrap();
    assert_eq!(kio(&dir, &["run", "app.kio"]), (0, "3\n".into(), "".into()));
}

/// Each error, at the `mod` line where there is one, with its hint; exit 2, nothing run.
#[test]
fn errors_and_hints() {
    let hash = sha(WEB);
    let cases: Vec<(String, Option<&str>, String)> = vec![
        (
            "web  ../lib/web.kio\n".into(),
            None,
            "app.kio:1: error: `web` has no hash in kio.deps:1\n  hint: review it, then `kio deps pin app.kio`\n".into(),
        ),
        (
            "# c\n\nweb ../lib/web.kio sha256:12ab\n".into(),
            None,
            "kio: kio.deps:3: `web ../lib/web.kio sha256:12ab` is not a dependency line\n  hint: write `NAME  PATH  sha256:HEX`: NAME a module name, PATH relative to kio.deps; `kio deps pin` writes the hash\n".into(),
        ),
        (
            "web-x  ../lib/web.kio\n".into(),
            None,
            "kio: kio.deps:1: `web-x  ../lib/web.kio` is not a dependency line\n  hint: write `NAME  PATH  sha256:HEX`: NAME a module name, PATH relative to kio.deps; `kio deps pin` writes the hash\n".into(),
        ),
        (
            format!("web ../lib/web.kio sha256:{hash}\nweb\t../lib/x.kio\n"),
            None,
            "kio: kio.deps:2: `web` is listed twice (also line 1)\n  hint: keep one line per module\n".into(),
        ),
        (
            format!("web  ../lib/nope.kio  sha256:{hash}\n"),
            None,
            "app.kio:1: error: kio.deps:1: ../lib/nope.kio does not exist\n  hint: a path in kio.deps is relative to the directory kio.deps is in\n".into(),
        ),
        (
            format!("web  ../lib/web.kio  sha256:{}\n", sha("old")),
            None,
            format!(
                "app.kio:1: error: ../lib/web.kio does not match kio.deps:1 (sha256:{}.. pinned, sha256:{}.. found)\n  hint: review the change, then `kio deps pin app.kio`\n",
                &sha("old")[..12],
                &hash[..12]
            ),
        ),
        (
            format!("web  ../lib/web.kio  sha256:{hash}\n"),
            Some("web.kio"),
            "app.kio:1: error: `web` is in kio.deps:1 and beside app.kio\n  hint: delete one\n".into(),
        ),
    ];
    for (deps, beside, want) in cases {
        let (_t, dir) = project(APP, &deps);
        if let Some(b) = beside {
            std::fs::write(dir.join(b), WEB).unwrap();
        }
        for cmd in ["check", "run"] {
            assert_eq!(
                kio(&dir, &[cmd, "app.kio"]),
                (2, "".into(), want.clone()),
                "{cmd} {deps}"
            );
        }
    }
}

#[test]
fn json_names_the_mod_line() {
    let (_t, dir) = project(APP, &format!("web  ../lib/web.kio  sha256:{}\n", sha("old")));
    let (code, out, _) = kio(&dir, &["check", "app.kio", "--json"]);
    assert_eq!(code, 2);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let e = &v["errors"][0];
    assert_eq!(
        (
            e["kind"].as_str(),
            e["file"].as_str(),
            e["line"].as_u64(),
            e["col"].as_u64()
        ),
        (Some("deps"), Some("app.kio"), Some(1), Some(1))
    );
    assert_eq!(e["hint"], "review the change, then `kio deps pin app.kio`");
}

/// A dependency's own error is shown at its path.
#[test]
fn an_error_in_a_dependency_names_its_path() {
    let bad = "pub fn inc(n: Int) -> Int {\n    nope\n}\n";
    let (_t, dir) = project(APP, &format!("web  ../lib/web.kio  sha256:{}\n", sha(bad)));
    std::fs::write(dir.join("../lib/web.kio"), bad).unwrap();
    let (code, _, err) = kio(&dir, &["check", "app.kio"]);
    assert_eq!(code, 1);
    assert!(err.starts_with("../lib/web.kio:2: error: unknown name `nope`"), "{err}");
}

/// `run`, `run --listen` and `test` stop on a mismatch before anything runs.
#[test]
fn a_mismatch_stops_run_listen_and_test() {
    let app = "mod web;\nfn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {\n    res.send(200, [], [], \"x\".to_bytes())\n}\n#[test]\nfn t() {\n    assert_eq(web::inc(1), 2);\n}\n";
    let (_t, dir) = project(app, &pinned());
    assert_eq!(kio(&dir, &["test", "app.kio"]).0, 0);
    std::fs::write(dir.join("../lib/web.kio"), WEB.replace("+ 1", "+ 2")).unwrap();
    for args in [
        &["run", "app.kio", "--listen", "127.0.0.1:0"][..],
        &["test", "app.kio"],
        &["run", "app.kio"],
    ] {
        let (code, _, err) = kio(&dir, args);
        assert_eq!(code, 2, "{args:?}");
        assert!(
            err.contains("app.kio:1: error: ../lib/web.kio does not match kio.deps:2"),
            "{args:?}: {err}"
        );
    }
}

#[test]
fn pin_writes_hashes_keeps_comments_and_is_idempotent() {
    let deps = "# where web comes from\n\nweb\t../lib/web.kio\n";
    let (_t, dir) = project(APP, deps);
    let h = sha(WEB);
    let want = format!("web  none -> sha256:{}..\n", &h[..12]);
    assert_eq!(kio(&dir, &["deps", "pin"]), (0, want, "".into()));
    let text = std::fs::read_to_string(dir.join("kio.deps")).unwrap();
    assert_eq!(
        text,
        format!("# where web comes from\n\nweb\t../lib/web.kio  sha256:{h}\n")
    );
    assert_eq!(
        kio(&dir, &["deps", "pin", "app.kio"]),
        (0, "nothing changed\n".into(), "".into())
    );
    assert_eq!(kio(&dir, &["run", "app.kio"]).0, 0);
    let web2 = WEB.replace("+ 1", "+ 2");
    std::fs::write(dir.join("../lib/web.kio"), &web2).unwrap();
    let parent = dir.parent().unwrap();
    let want = format!("web  sha256:{}.. -> sha256:{}..\n", &h[..12], &sha(&web2)[..12]);
    assert_eq!(kio(parent, &["deps", "pin", "app/app.kio"]), (0, want, "".into()));
    let text = std::fs::read_to_string(dir.join("kio.deps")).unwrap();
    assert_eq!(
        text,
        format!("# where web comes from\n\nweb\t../lib/web.kio  sha256:{}\n", sha(&web2))
    );
    assert_eq!(kio(&dir, &["run", "app.kio"]), (0, "3\n".into(), "".into()));
    // A missing file or kio.deps is exit 2, and nothing is written.
    std::fs::write(dir.join("kio.deps"), "web ../lib/nope.kio\n").unwrap();
    let (code, _, err) = kio(&dir, &["deps", "pin"]);
    assert_eq!((code, err.as_str()), (2, "kio: kio.deps:1: ../lib/nope.kio does not exist\n  hint: a path in kio.deps is relative to the directory kio.deps is in\n"));
    assert_eq!(
        std::fs::read_to_string(dir.join("kio.deps")).unwrap(),
        "web ../lib/nope.kio\n"
    );
    let (code, _, err) = kio(parent, &["deps", "pin"]);
    assert_eq!(code, 2);
    assert!(err.starts_with("kio: kio.deps: cannot read: "), "{err}");
    assert!(
        err.ends_with(
            "hint: create it beside the root, a line `NAME  PATH` per module; `kio deps pin` adds the hashes\n"
        ),
        "{err}"
    );
}

/// `--base` reads each side's own kio.deps: a dependency that starts writing shows as authority
/// added.
#[test]
fn base_uses_each_sides_kio_deps() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let look = "pub fn look(d: Dir) -> Result<Int, IoErr> {\n    Ok(d.list(\"\")?.len())\n}\n";
    let touch = "pub fn look(d: Dir) -> Result<Int, IoErr> {\n    d.write(\"x\", \"y\".to_bytes())?;\n    Ok(0)\n}\n";
    let app = "mod web;\nfn main(d: Dir) -> Result<Int, IoErr> {\n    web::look(d)\n}\n";
    for (side, lib) in [("old", look), ("new", touch)] {
        std::fs::create_dir_all(root.join(side).join("lib")).unwrap();
        std::fs::write(root.join(side).join("lib/web.kio"), lib).unwrap();
        std::fs::write(root.join(side).join("app.kio"), app).unwrap();
        let deps = format!("web lib/web.kio sha256:{}\n", sha(lib));
        std::fs::write(root.join(side).join("kio.deps"), deps).unwrap();
    }
    let (code, out, err) = kio(&root, &["check", "new/app.kio", "--base", "old/app.kio"]);
    assert_eq!(code, 3, "{err}");
    assert!(out.starts_with("~ d: Dir (+write -list)\n"), "{out}");

    let (code, out, err) = kio(&root, &["review", "new/app.kio", "--base", "old/app.kio"]);
    assert_eq!(code, 3, "{err}");
    let (o, n) = (sha(look), sha(touch));
    let changed = format!(
        "  not checked  dependency web changed (sha256:{}.. -> sha256:{}..): its code is not shown; its authority is above",
        &o[..12],
        &n[..12]
    );
    assert!(out.contains(&changed), "{out}");
    assert!(
        out.contains(&format!("  old dep web lib/web.kio sha256:{o}\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("  new dep web lib/web.kio sha256:{n}\n")),
        "{out}"
    );
    assert!(!out.contains("changed while the review ran"), "{out}");
    // Added and removed.
    std::fs::write(root.join("old/web.kio"), look).unwrap();
    std::fs::remove_file(root.join("old/kio.deps")).unwrap();
    let (_, out, _) = kio(&root, &["review", "new/app.kio", "--base", "old/app.kio"]);
    assert!(
        out.contains(&format!("dependency web added (sha256:{}..)", &n[..12])),
        "{out}"
    );
    let (_, out, _) = kio(&root, &["review", "old/app.kio", "--base", "new/app.kio", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["not_checked"]["deps"],
        serde_json::json!([{"name": "web", "old": n, "new": null}])
    );
    assert_eq!(v["identity"]["deps"]["old"][0]["sha256"], n);
}

/// A dependency and kio.deps are the program's own source: a writable `--dir` holding either is
/// refused; `:ro` runs.
#[test]
fn a_writable_dir_holding_a_dependency_or_kio_deps_is_refused() {
    let app = "mod web;\nfn main(d: Dir) -> Result<Int, IoErr> {\n    Ok(web::inc(d.list(\"\")?.len()))\n}\n";
    let (_t, dir) = project(app, &pinned());
    let refusal = |grant: &str, file: &str| {
        format!("kio: `--dir {grant}` contains the program's own source (`{file}`); it could rewrite itself for the next run\n  hint: keep the program outside every granted directory\n")
    };
    assert_eq!(
        kio(&dir, &["run", "app.kio", "--dir", "d=../lib"]),
        (2, "".into(), refusal("d=../lib", "../lib/web.kio"))
    );
    assert_eq!(kio(&dir, &["run", "app.kio", "--dir", "d=../lib:ro"]).0, 0);
    // kio.deps reached through a symlink into another directory.
    let pins = dir.join("../pins");
    std::fs::create_dir(&pins).unwrap();
    std::fs::rename(dir.join("kio.deps"), pins.join("kio.deps")).unwrap();
    std::os::unix::fs::symlink(pins.join("kio.deps"), dir.join("kio.deps")).unwrap();
    assert_eq!(
        kio(&dir, &["run", "app.kio", "--dir", "d=../pins"]),
        (2, "".into(), refusal("d=../pins", "kio.deps"))
    );
}

#[test]
fn the_audit_log_names_each_dependency() {
    let (_t, dir) = project(APP, &pinned());
    assert_eq!(kio(&dir, &["run", "app.kio", "--audit", "../audit.log"]).0, 0);
    let log = std::fs::read_to_string(dir.join("../audit.log")).unwrap();
    let want = format!(
        r#""deps":[{{"name":"web","path":"../lib/web.kio","sha256":"{}"}}]"#,
        sha(WEB)
    );
    assert!(log.lines().next().unwrap().contains(&want), "{log}");
}

/// Anything but a regular file is refused before it is read (a FIFO or device would block or
/// never end), by `check`, `deps pin` and for the root; nothing is written.
#[test]
fn a_dependency_that_is_not_a_regular_file_is_refused() {
    let (_t, dir) = project(APP, &format!("web  ../lib/fifo.kio  sha256:{}\n", sha(WEB)));
    let fifo = dir.join("../lib/fifo.kio");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let want = "app.kio:1: error: kio.deps:1: ../lib/fifo.kio is not a regular file\n";
    assert_eq!(kio(&dir, &["check", "app.kio"]), (2, "".into(), want.into()));
    std::os::unix::fs::symlink("/dev/zero", dir.join("../lib/zero.kio")).unwrap();
    std::fs::write(dir.join("kio.deps"), "web  ../lib/zero.kio\n").unwrap();
    let want = "kio: kio.deps:1: ../lib/zero.kio is not a regular file\n";
    assert_eq!(kio(&dir, &["deps", "pin"]), (2, "".into(), want.into()));
    assert_eq!(
        std::fs::read_to_string(dir.join("kio.deps")).unwrap(),
        "web  ../lib/zero.kio\n"
    );
    let want = "kio: ../lib/fifo.kio is not a regular file\n";
    assert_eq!(kio(&dir, &["check", "../lib/fifo.kio"]), (2, "".into(), want.into()));
}

/// An absolute path, or a control character in a field, is not a dependency line; the line is
/// echoed escaped. A leading byte-order mark is skipped.
#[test]
fn absolute_paths_and_control_characters_are_refused() {
    let form = "\n  hint: write `NAME  PATH  sha256:HEX`: NAME a module name, PATH relative to kio.deps; `kio deps pin` writes the hash\n";
    let (_t, dir) = project(APP, "web  /etc/web.kio\n");
    let want = format!("kio: kio.deps:1: `web  /etc/web.kio` is not a dependency line{form}");
    assert_eq!(kio(&dir, &["check", "app.kio"]), (2, "".into(), want));
    std::fs::write(dir.join("kio.deps"), "web  ../lib/\x1b[2Jweb.kio\n").unwrap();
    let want = format!("kio: kio.deps:1: `web  ../lib/\\u{{1b}}[2Jweb.kio` is not a dependency line{form}");
    assert_eq!(kio(&dir, &["check", "app.kio"]), (2, "".into(), want));
    std::fs::write(dir.join("kio.deps"), format!("\u{feff}{}", pinned())).unwrap();
    assert_eq!(kio(&dir, &["run", "app.kio"]), (0, "2\n".into(), "".into()));
    let (code, out, _) = kio(&dir, &["run", "app.kio", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["deps"][0]["sha256"], sha(WEB));
}

/// `--base` names each dependency whose pin changed, though its authority did not; exit codes
/// follow authority alone.
#[test]
fn base_names_a_dependency_whose_pin_changed() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let web2 = format!("{WEB}// a comment\n");
    for (side, lib) in [("old", WEB), ("new", web2.as_str())] {
        std::fs::create_dir_all(root.join(side).join("lib")).unwrap();
        std::fs::write(root.join(side).join("lib/web.kio"), lib).unwrap();
        std::fs::write(root.join(side).join("app.kio"), APP).unwrap();
        let deps = format!("web lib/web.kio sha256:{}\n", sha(lib));
        std::fs::write(root.join(side).join("kio.deps"), deps).unwrap();
    }
    let base = ["check", "new/app.kio", "--base", "old/app.kio"];
    let want = format!(
        "no change in authority\n\ndep web changed (sha256:{}.. -> sha256:{}..)\n",
        &sha(WEB)[..12],
        &sha(&web2)[..12]
    );
    assert_eq!(kio(&root, &base), (0, want, "".into()));
    let (code, out, _) = kio(&root, &[&base[..], &["--json"]].concat());
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let want = serde_json::json!([{"name": "web", "old": sha(WEB), "new": sha(&web2)}]);
    assert_eq!(v["diff"]["deps"], want);
    std::fs::remove_file(root.join("old/kio.deps")).unwrap();
    std::fs::write(root.join("old/web.kio"), WEB).unwrap();
    let added = "no change in authority\n\ndep web added\n";
    assert_eq!(kio(&root, &base), (0, added.into(), "".into()));
    let back = ["check", "old/app.kio", "--base", "new/app.kio"];
    let removed = "no change in authority\n\ndep web removed\n";
    assert_eq!(kio(&root, &back), (0, removed.into(), "".into()));
}
