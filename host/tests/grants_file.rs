//! P4: `kio run app.kio --grants app.grants` reads grant flags from a file, and
//! `kio check app.kio --grants app.grants` binds them to `main` without running or reading a secret.
use std::path::Path;
use std::process::Command;

const APP: &str = r#"fn main(input: Dir, now: Clock) -> Str {
  match input.read("a.txt") { Ok(b) => b.to_str() + " " + now.now_ms().to_str(), Err(_) => "no file" }
}
"#;

/// `(exit code, stdout, stderr)` of `kio args..`.
fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(args)
        .env_remove("P4_UNSET")
        .output()
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

/// A directory with `app.kio`, and `in dir/a.txt` holding `hi`: its name has a space.
fn setup() -> (tempfile::TempDir, String, String) {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "app.kio", APP);
    let input = t.path().join("in dir");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("a.txt"), "hi").unwrap();
    (t, app, input.to_str().unwrap().to_string())
}

#[test]
fn comments_blank_lines_and_a_value_with_spaces() {
    let (t, app, input) = setup();
    let g = write(
        t.path(),
        "app.grants",
        &format!("# app's grants\n\n  --dir input={input}  \n\t# the clock\n--clock   now=fixed:5\r\n"),
    );
    let (code, out, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!((code, out.as_str()), (0, "hi 5\n"), "{err}");
}

#[test]
fn command_line_flags_are_added_after_the_file() {
    let (t, app, input) = setup();
    let g = write(t.path(), "app.grants", &format!("--dir input={input}\n"));
    let (code, out, err) = kio(&["run", &app, "--clock", "now=fixed:7", "--grants", &g]);
    assert_eq!((code, out.as_str()), (0, "hi 7\n"), "{err}");
}

#[test]
fn quotes_are_part_of_the_value() {
    let (t, app, input) = setup();
    let g = write(
        t.path(),
        "app.grants",
        &format!("--dir input=\"{input}\"\n--clock now\n"),
    );
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains(&format!("--dir input=\"{input}\"")), "{err}");
}

#[test]
fn unknown_flags_and_lines_that_are_not_flags() {
    let (t, app, _) = setup();
    let g = write(t.path(), "a.grants", "# ok\n--clock now\n--dirs input=x\n");
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains(&format!("{g}:3: unknown flag `--dirs`")), "{err}");
    assert!(err.contains("hint: a line is one of `kio run`'s flags: --dir"), "{err}");
    // A line that is not a flag is not echoed: it could be a key.
    let g = write(t.path(), "b.grants", "whsec_p4_secret_value\n");
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains(&format!("{g}:1: not a flag")), "{err}");
    assert!(err.contains("hint: each line is one flag"), "{err}");
    assert!(!err.contains("whsec_p4"), "{err}");
    let g = write(t.path(), "c.grants", "--clock\n");
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains(&format!("{g}:1: `--clock` has no value")), "{err}");
    let g = write(t.path(), "d.grants", "--grants other.grants\n");
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("a grants file cannot name another"), "{err}");
    let (code, _, err) = kio(&["run", &app, "--grants", &g, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("--grants is given twice"), "{err}");
    let (code, _, err) = kio(&["run", &app, "--grants", "/nonexistent/p4.grants"]);
    assert_eq!(code, 2);
    assert!(err.contains("--grants /nonexistent/p4.grants: cannot read"), "{err}");
}

#[test]
fn a_flag_given_once_in_both_places_is_refused() {
    let (t, app, input) = setup();
    let g = write(
        t.path(),
        "app.grants",
        &format!("--dir input={input}\n--clock now\n--timeout 5\n"),
    );
    let (code, _, err) = kio(&["run", &app, "--grants", &g, "--timeout", "6"]);
    assert_eq!(code, 2);
    assert!(
        err.contains(&format!("`--timeout` is given in {g}:3 and on the command line")),
        "{err}"
    );
    assert!(err.contains("hint: give it in one place"), "{err}");
    let g = write(t.path(), "twice.grants", "--fuel 100\n# x\n--fuel 200\n");
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(
        err.contains(&format!("`--fuel` is given twice in {g}, lines 1 and 3")),
        "{err}"
    );
    // A grant of one name in both places is two grants of it.
    let g = write(t.path(), "both.grants", &format!("--dir input={input}\n--clock now\n"));
    let (code, _, err) = kio(&["run", &app, "--grants", &g, "--clock", "now=fixed:1"]);
    assert_eq!(code, 1);
    assert!(err.contains("grant `now` is given more than once"), "{err}");
}

#[test]
fn check_grants_binds_without_running() {
    let (t, app, input) = setup();
    let g = write(t.path(), "app.grants", &format!("--dir input={input}\n--clock now\n"));
    let (code, out, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!((code, out.as_str()), (0, "main : input read, now now_ms\n"), "{err}");
    // A path is not opened: CI need not have it.
    let g = write(t.path(), "away.grants", "--dir input=/nonexistent/p4\n--clock now\n");
    assert_eq!(kio(&["check", &app, "--grants", &g]).0, 0);
    // Missing, extra and wrong-kind grants: the usual errors and hints.
    let g = write(t.path(), "missing.grants", "--dir input=x\n");
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 1);
    assert!(err.contains("no grant for `now: Clock`"), "{err}");
    assert!(err.contains("hint: grant it: `--clock now`"), "{err}");
    let g = write(t.path(), "extra.grants", "--dir input=x\n--clock now\n--rng r\n");
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 1);
    assert!(err.contains("grant `r` is not a parameter of `main`"), "{err}");
    assert!(
        err.contains("hint: grant only `main`'s parameters: input, now"),
        "{err}"
    );
    let g = write(t.path(), "kind.grants", "--dir input=x\n--rng now\n");
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 1);
    assert!(err.contains("no grant for `now: Clock`"), "{err}");
    // Syntax is checked as `kio run` checks it.
    let g = write(t.path(), "bad.grants", "--dir input=x\n--clock now=fixed:soon\n");
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("--clock now=fixed:soon"), "{err}");
    // Extra flags on the command line, as with `run`.
    let g = write(t.path(), "dir.grants", "--dir input=x\n");
    assert_eq!(kio(&["check", &app, "--grants", &g, "--clock", "now"]).0, 0);
}

const HMAC_APP: &str = r#"fn main(api: Http, k: Hmac) -> Bool { k.verify("x".to_bytes(), "y".to_bytes()) }
"#;

#[test]
fn check_grants_reads_no_secret() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "app.kio", HMAC_APP);
    let g = write(
        t.path(),
        "app.grants",
        "--http api=api.example\n--http-header api=Authorization:env:P4_UNSET\n--hmac k=file:/nonexistent/p4.key:verify\n",
    );
    let (code, out, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!((code, out.as_str()), (0, "main : k verify\n"), "{err}");
    // `kio run` reads them, and refuses.
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("`P4_UNSET` is not set"), "{err}");
    // The form is still checked, and a header name, and `:verify` against a signing program.
    let g = write(t.path(), "form.grants", "--http api=api.example\n--hmac k=P4_UNSET\n");
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("--hmac k: expected name=env:VAR"), "{err}");
    let g = write(
        t.path(),
        "header.grants",
        "--http api=api.example\n--http-header api=Host:env:P4_UNSET\n--hmac k=env:P4_UNSET\n",
    );
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("`Host` is set by the host"), "{err}");
    let signs = write(
        t.path(),
        "signs.kio",
        "fn main(k: Hmac) -> Bool { k.sign(\"x\".to_bytes()).is_ok() }\n",
    );
    let g = write(t.path(), "signs.grants", "--hmac k=env:P4_UNSET:verify\n");
    let (code, _, err) = kio(&["check", &signs, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(
        err.contains("`k` is granted `:verify` but the program can sign with it"),
        "{err}"
    );
}

#[test]
fn check_grants_binds_a_server() {
    let t = tempfile::tempdir().unwrap();
    let app = write(
        t.path(),
        "app.kio",
        "fn main(req: Incoming, res: Outgoing, now: Clock) -> Result<(), Str> { Ok(()) }\n",
    );
    let g = write(t.path(), "app.grants", "--clock now\n--listen 127.0.0.1:8080\n");
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 0, "{err}");
    let g = write(t.path(), "nolisten.grants", "--clock now\n");
    let (code, _, err) = kio(&["check", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("hint: add `--listen 127.0.0.1:8080`"), "{err}");
}

#[test]
fn a_grants_file_the_program_could_change_is_refused() {
    let (t, app, input) = setup();
    // Inside a granted `--dir`: the program could write its own grants for the next run.
    let g = write(
        Path::new(&input),
        "app.grants",
        &format!("--dir input={input}\n--clock now\n"),
    );
    for cmd in ["run", "check"] {
        let (code, _, err) = kio(&[cmd, &app, "--grants", &g]);
        assert_eq!(code, 2, "{cmd}");
        assert!(err.contains("is inside `--dir input="), "{err}");
        assert!(
            err.contains("hint: keep the grants file outside every granted directory"),
            "{err}"
        );
    }
    // A symlink inside it to a file outside: the program could point it elsewhere.
    let outside = write(
        t.path(),
        "outside.grants",
        &format!("--dir input={input}\n--clock now\n"),
    );
    let link = Path::new(&input).join("link.grants");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let (code, _, err) = kio(&["run", &app, "--grants", link.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(err.contains("is inside `--dir input="), "{err}");
    assert_eq!(kio(&["run", &app, "--grants", &outside]).0, 0);
    // The `--log` file, or a store's.
    let g = t.path().join("app.grants").to_str().unwrap().to_string();
    for flag in ["--log log=", "--kv kv=", "--audit "] {
        std::fs::write(&g, format!("--dir input={input}\n--clock now\n{flag}{g}\n")).unwrap();
        let (code, _, err) = kio(&["run", &app, "--grants", &g]);
        assert_eq!(code, 2, "{flag}");
        assert!(err.contains("could change its own grants"), "{err}");
    }
    // Another hard link could be inside a granted directory.
    std::fs::write(&g, format!("--dir input={input}\n--clock now\n")).unwrap();
    let other = t.path().join("link.grants");
    std::fs::hard_link(&g, &other).unwrap();
    let (code, _, err) = kio(&["run", &app, "--grants", &g]);
    assert_eq!(code, 2);
    assert!(err.contains("has other hard links"), "{err}");
    std::fs::remove_file(&other).unwrap();
    assert_eq!(kio(&["run", &app, "--grants", &g]).0, 0);
}
