//! H28: a writable `--dir` that contains the program's root file or a module file is refused by
//! `kio run`, `--listen` and `kio check --grants`: the program could rewrite itself for the next
//! run. A `:ro` dir cannot write and is granted.
use std::path::Path;
use std::process::Command;

/// `(exit code, stdout, stderr)` of `kio args..` run in `cwd`.
fn kio(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

fn refusal(dir: &str, file: &str) -> String {
    format!(
        "kio: `--dir {dir}` contains the program's own source (`{file}`); it could rewrite itself for the next run\n  hint: keep the program outside every granted directory\n"
    )
}

/// Rewrites `app.kio` in the dir it is granted.
const SELF_WRITER: &str =
    r#"fn main(d: Dir) -> Result<(), IoErr> { d.write("app.kio", "fn main() -> Int { 666 }".to_bytes()) }"#;

fn s(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

#[test]
fn a_dir_containing_the_root_file_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let t = &t.path().canonicalize().unwrap();
    let app = t.join("app");
    std::fs::create_dir_all(app.join("sub")).unwrap();
    std::fs::create_dir(t.join("other")).unwrap();
    std::fs::write(app.join("app.kio"), SELF_WRITER).unwrap();
    std::os::unix::fs::symlink(&app, t.join("link")).unwrap();
    let prog = s(&app.join("app.kio"));
    let cases = [
        (t.clone(), s(&app)),
        (t.clone(), s(t)),
        (t.clone(), s(&t.join("link"))),
        (t.clone(), s(&app.join("sub/.."))),
        (t.clone(), "app".into()),
        (app.clone(), ".".into()),
        (app.join("sub"), "..".into()),
        (t.join("other"), "../app".into()),
    ];
    for (cwd, dir) in cases {
        let d = format!("d={dir}");
        assert_eq!(
            kio(&cwd, &["run", &prog, "--dir", &d]),
            (2, String::new(), refusal(&d, &prog)),
            "{dir} from {cwd:?}"
        );
    }
    // The root as given, relative to the working directory.
    assert_eq!(
        kio(&app, &["run", "app.kio", "--dir", "d=."]),
        (2, String::new(), refusal("d=.", "app.kio"))
    );
    assert_eq!(std::fs::read_to_string(app.join("app.kio")).unwrap(), SELF_WRITER);
}

#[test]
fn a_dir_containing_only_a_module_file_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let t = &t.path().canonicalize().unwrap();
    let (app, lib) = (t.join("app"), t.join("lib"));
    std::fs::create_dir(&app).unwrap();
    std::fs::create_dir(&lib).unwrap();
    std::fs::write(
        app.join("main.kio"),
        "mod util;\nfn main(d: Dir) -> Int { util::one() }",
    )
    .unwrap();
    std::fs::write(lib.join("util.kio"), "pub fn one() -> Int { 1 }").unwrap();
    std::os::unix::fs::symlink(lib.join("util.kio"), app.join("util.kio")).unwrap();
    let prog = s(&app.join("main.kio"));
    let d = format!("d={}", s(&lib));
    assert_eq!(
        kio(t, &["run", &prog, "--dir", &d]),
        (2, String::new(), refusal(&d, &s(&app.join("util.kio"))))
    );
    // A dir beside both runs.
    std::fs::create_dir(t.join("data")).unwrap();
    let d = format!("d={}", s(&t.join("data")));
    let (code, out, err) = kio(t, &["run", &prog, "--dir", &d]);
    assert_eq!((code, out.as_str()), (0, "1\n"), "{err}");
}

#[test]
fn a_root_file_symlinked_into_a_dir_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let t = &t.path().canonicalize().unwrap();
    let (app, real) = (t.join("app"), t.join("real"));
    std::fs::create_dir(&app).unwrap();
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("app.kio"), SELF_WRITER).unwrap();
    std::os::unix::fs::symlink(real.join("app.kio"), app.join("app.kio")).unwrap();
    // The program could point the link at a file of its own.
    let prog = s(&app.join("app.kio"));
    let d = format!("d={}", s(&app));
    assert_eq!(
        kio(t, &["run", &prog, "--dir", &d]),
        (2, String::new(), refusal(&d, &prog))
    );
}

#[test]
fn a_dir_beside_the_program_and_a_read_only_one_containing_it_run() {
    let t = tempfile::tempdir().unwrap();
    let t = &t.path().canonicalize().unwrap();
    let (app, data) = (t.join("app"), t.join("data"));
    std::fs::create_dir(&app).unwrap();
    std::fs::create_dir(&data).unwrap();
    std::fs::write(app.join("app.kio"), SELF_WRITER).unwrap();
    let prog = s(&app.join("app.kio"));
    let d = format!("d={}", s(&data));
    let (code, _, err) = kio(t, &["run", &prog, "--dir", &d]);
    assert_eq!(code, 0, "{err}");
    assert!(data.join("app.kio").exists());
    // A `:ro` dir cannot write the source: granted to a program that only reads.
    let reader = app.join("r.kio");
    std::fs::write(
        &reader,
        r#"fn main(d: Dir) -> Result<Int, IoErr> { Ok(d.read("app.kio")?.len()) }"#,
    )
    .unwrap();
    let ro = format!("d={}:ro", s(&app));
    let (code, out, err) = kio(t, &["run", &s(&reader), "--dir", &ro]);
    assert_eq!((code, out), (0, format!("Ok({})\n", SELF_WRITER.len())), "{err}");
}

#[test]
fn grants_files_and_listen_are_refused_too() {
    let t = tempfile::tempdir().unwrap();
    let t = &t.path().canonicalize().unwrap();
    let app = t.join("app");
    std::fs::create_dir(&app).unwrap();
    std::fs::write(app.join("app.kio"), SELF_WRITER).unwrap();
    let prog = s(&app.join("app.kio"));
    let d = format!("d={}", s(&app));
    let grants = s(&t.join("app.grants"));
    std::fs::write(&grants, format!("--dir {d}\n")).unwrap();
    for cmd in ["check", "run"] {
        assert_eq!(
            kio(t, &[cmd, &prog, "--grants", &grants]),
            (2, String::new(), refusal(&d, &prog)),
            "{cmd}"
        );
    }
    let server = app.join("server.kio");
    std::fs::write(
        &server,
        r#"fn main(req: Incoming, res: Outgoing, d: Dir) -> Result<(), WebErr> { res.send(200, [], [], "ok".to_bytes()) }"#,
    )
    .unwrap();
    let (code, _, err) = kio(t, &["run", &s(&server), "--listen", "127.0.0.1:0", "--dir", &d]);
    assert_eq!((code, err), (2, refusal(&d, &s(&server))));
}
