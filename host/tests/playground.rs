//! SITE B: the playground (tools/playground, built natively here) says what `kio check` says, for
//! every example on the page and for programs with errors.
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// Programs that fail to check, beyond the page's `error.kio`.
const ERRORS: [&str; 5] = [
    // A column counts characters, not bytes.
    "fn main() -> Str {\n  let s = \"héllo ✓\"; s + missing\n}\n",
    "fn main( -> Str { \"x\" }\n",
    "fn main() -> Int {\n  \"x\"\n}\n",
    "fn helper() -> Str {\n  input.read(\"a\").unwrap_or([].to_bytes()).to_str()\n}\n\nfn other() -> Int { true }\n\nfn main() -> Str { helper() }\n",
    "",
];

/// `(exit code, stdout)` of `kio args..`.
fn kio(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("{e}: {s}"))
}

/// The page's examples, by file name.
fn examples() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../site/playground");
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "kio"))
        .map(|p| {
            (
                p.file_name().unwrap().to_str().unwrap().into(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    assert!(out.len() >= 6, "{out:?}");
    out
}

/// Each error's message, hint, line and column: the fields both give.
fn errors(v: &Value) -> Vec<Value> {
    let keep = |e: &Value| serde_json::json!([e["message"], e["hint"], e["line"], e["col"]]);
    v["errors"].as_array().unwrap().iter().map(keep).collect()
}

/// `src` as `name` in `dir`.
fn write(dir: &Path, name: &str, src: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    path.to_str().unwrap().into()
}

#[test]
fn check_matches_the_cli() {
    let t = tempfile::tempdir().unwrap();
    let mut programs = examples();
    programs.extend(
        ERRORS
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("error {i}"), s.to_string())),
    );
    let mut failing = 0;
    for (name, src) in &programs {
        let path = write(t.path(), "playground.kio", src);
        let page = json(&playground::check_json(src));
        let (code, out) = kio(&["check", &path, "--json"]);
        let cli = json(&out);
        assert_eq!(page["ok"], cli["ok"], "{name}");
        assert_eq!(errors(&page), errors(&cli), "{name}");
        if page["ok"] == false {
            failing += 1;
            assert_eq!(code, 1, "{name}");
            for e in page["errors"].as_array().unwrap() {
                let at = |l: &str, c: &str| (e[l].as_u64().unwrap(), e[c].as_u64().unwrap());
                assert!(at("end_line", "end_col") >= at("line", "col"), "{name}: {e}");
            }
            continue;
        }
        assert_eq!(page["report"], cli["report"], "{name}");
        let (code, text) = kio(&["check", &path]);
        assert_eq!(code, 0, "{name}");
        assert_eq!(page["text"].as_str().unwrap(), text.trim_end_matches('\n'), "{name}");
    }
    assert_eq!(failing, 6, "error.kio and ERRORS");
}

#[test]
fn diff_matches_the_cli() {
    let t = tempfile::tempdir().unwrap();
    let ex: std::collections::HashMap<String, String> = examples().into_iter().collect();
    let pairs = [
        ("sync.kio", "archive.kio"),
        ("archive.kio", "sync.kio"),
        ("hello.kio", "sync.kio"),
        ("sync.kio", "sync.kio"),
        ("count.kio", "tests.kio"),
        ("error.kio", "sync.kio"),
        ("sync.kio", "error.kio"),
    ];
    for (old, new) in pairs {
        let (o, n) = (
            write(t.path(), "base.kio", &ex[old]),
            write(t.path(), "playground.kio", &ex[new]),
        );
        let page = json(&playground::diff_json(&ex[old], &ex[new]));
        let (code, out) = kio(&["check", &n, "--base", &o, "--json"]);
        let cli = json(&out);
        assert_eq!(page["ok"], cli["ok"], "{old} -> {new}");
        assert_eq!(errors(&page), errors(&cli), "{old} -> {new}");
        if page["ok"] == false {
            let side = if old == "error.kio" { "base" } else { "new" };
            assert!(page["errors"].as_array().unwrap().iter().all(|e| e["side"] == side));
            continue;
        }
        for k in ["grants", "funcs", "added"] {
            assert_eq!(page[k], cli["diff"][k], "{old} -> {new}: {k}");
        }
        let (code2, text) = kio(&["check", &n, "--base", &o]);
        assert_eq!(
            (code2, page["text"].as_str().unwrap()),
            (code, text.trim_end_matches('\n')),
            "{old} -> {new}"
        );
    }
    let page = json(&playground::diff_json(&ex["sync.kio"], &ex["archive.kio"]));
    assert_eq!(page["grants"], serde_json::json!(["~ orders: Dir (+remove)"]));
    assert_eq!(page["added"], true);
}

#[test]
fn one_file_and_64_kb() {
    let page = json(&playground::check_json("  mod web;\nfn main() {}\n"));
    let e = &page["errors"][0];
    assert_eq!(e["message"], "the playground checks one file");
    assert_eq!(e["hint"], "use `kio check` for modules");
    assert_eq!(
        (&e["line"], &e["col"], &e["end_col"]),
        (&1.into(), &3.into(), &11.into())
    );
    let big = format!("fn main() {{}}\n{}", " ".repeat(playground::MAX_INPUT));
    let page = json(&playground::check_json(&big));
    assert_eq!(page["errors"][0]["message"], "the playground checks up to 64 KB");
    assert_eq!(page["errors"][0]["line"], Value::Null);
    let ok = json(&playground::check_json(&ex_sync()));
    assert_eq!(ok["grants"][1]["flag"], "--http crm=HOST --http-methods crm=POST");
}

fn ex_sync() -> String {
    examples().into_iter().find(|(n, _)| n == "sync.kio").unwrap().1
}
