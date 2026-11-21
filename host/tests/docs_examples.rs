//! D7, D8, D10: guide.md's Db migration example, its outbox example and its "Fast forms" programs run as written, and
//! the fuel each fast-forms program states (and primer.md's table repeats) is what it uses.
use std::path::Path;
use std::process::Command;

fn doc(file: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs").join(file)).unwrap()
}

/// The text of `text` from `heading` to the next heading of its level or higher.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let from = &text[text.find(heading).unwrap_or_else(|| panic!("{heading}"))..];
    let level = heading.split(' ').next().unwrap();
    let end = from[1..]
        .match_indices("\n#")
        .find(|(i, _)| {
            let h = from[i + 2..].split(' ').next().unwrap();
            h.len() <= level.len() && h.chars().all(|c| c == '#')
        })
        .map_or(from.len(), |(i, _)| i + 1);
    &from[..end]
}

/// The fenced blocks of `text`, in order.
fn blocks(text: &str) -> Vec<&str> {
    text.split("```\n").skip(1).step_by(2).collect()
}

/// `(exit code, stdout, stderr)` of `kio args..`.
fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code().unwrap(), text(&out.stdout), text(&out.stderr))
}

#[test]
fn db_migration_runs_as_shown() {
    let guide = doc("guide.md");
    let db = section(&guide, "### Db");
    let start = db.find("**Schema changes.**").unwrap();
    let b = blocks(&db[start..]);
    let (program, session) = (b[0], b[1]);
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("bookings.kio");
    std::fs::write(&app, program).unwrap();
    let grant = format!("db={}", dir.path().join("bookings.db").display());
    let runs: Vec<&str> = session
        .split("$ kio run bookings.kio --db db=/tmp/bookings.db\n")
        .skip(1)
        .collect();
    assert_eq!(runs.len(), 2, "{session}");
    for want in runs {
        let (code, out, err) = kio(&["run", app.to_str().unwrap(), "--db", &grant]);
        assert_eq!((code, err + &out), (0, want.to_string()));
    }
}

/// `1234567` as `"1.2M"`, as primer.md writes fuel.
fn millions(n: u64) -> String {
    format!("{:.1}M", n as f64 / 1e6)
}

/// The access log and orders the fast-forms programs read, as guide.md describes them.
fn inputs(dir: &Path) {
    let mut log = String::new();
    for i in 0..2000 {
        let status = [200, 200, 200, 404, 500][i % 5];
        log += &format!(
            "2026-11-20T10:{:02}:{:02} GET /api/items/{} status={status} ms={}\n",
            i / 60 % 60,
            i % 60,
            i % 500,
            i * 7 % 300
        );
    }
    let orders: Vec<String> = (0..500)
        .map(|i| format!("{{\"id\": {i}, \"qty\": {}, \"price\": {}}}", i % 9 + 1, i * 37 % 1000))
        .collect();
    std::fs::write(dir.join("access.log"), log).unwrap();
    std::fs::write(dir.join("orders.json"), format!("[\n  {}\n]\n", orders.join(",\n  "))).unwrap();
}

/// Each program's first line, `// .. Fuel: slow 17,918,946; fast 4,601,583.`, names its
/// functions and their fuel; `main` calls `fast`, and each other function is run by calling it
/// there instead. All agree on the value; each uses its stated fuel within 5% (the bench gate
/// catches smaller drift; this catches the docs going stale), and the first is the slowest and
/// `fast` the fastest. primer.md's table repeats every number in order, in millions.
#[test]
fn fast_forms_use_the_fuel_shown() {
    let guide = doc("guide.md");
    let primer = doc("primer.md");
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in");
    std::fs::create_dir(&input).unwrap();
    inputs(&input);
    let grant = format!("input={}:ro", input.display());
    let exact = std::env::var_os("KIO_TEST_HEAP").is_none();
    let mut stated: Vec<u64> = vec![];
    for program in blocks(section(&guide, "### Fast forms")) {
        let line = program.lines().next().unwrap();
        let fuel = line.split("Fuel: ").nth(1).unwrap_or_else(|| panic!("{line}"));
        let main = program.rfind("fast(").unwrap();
        let mut value = None;
        let mut used = vec![];
        for entry in fuel.trim_end_matches('.').split("; ") {
            let (name, n) = entry.split_once(' ').unwrap();
            let want: u64 = n.replace(',', "").parse().unwrap();
            let src = format!("{}{name}({}", &program[..main], &program[main + 5..]);
            let app = dir.path().join(format!("{name}.kio"));
            std::fs::write(&app, src).unwrap();
            let (_, out, _) = kio(&["run", app.to_str().unwrap(), "--dir", &grant, "--json"]);
            let t: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(t["ok"], true, "{name}: {t}");
            assert_eq!(value.get_or_insert(t["value"].clone()), &t["value"], "{name}");
            let got = t["used"]["fuel"].as_u64().unwrap();
            eprintln!("{name} {got}");
            let off = got.abs_diff(want) as f64 / want as f64;
            assert!(!exact || off <= 0.05, "{line}\n  {name} used {got}");
            stated.push(want);
            used.push((name, got));
        }
        assert!(used.windows(2).all(|w| w[0].1 > w[1].1), "{used:?}");
        assert_eq!(used.last().unwrap().0, "fast");
    }
    let table: Vec<&str> = section(&primer, "## Fast forms")
        .split([' ', '|'])
        .filter(|w| w.ends_with('M') && w[..w.len() - 1].parse::<f64>().is_ok())
        .collect();
    let want: Vec<String> = stated.iter().map(|&n| millions(n)).collect();
    assert_eq!(table, want);
}

/// D10: guide.md's outbox program, its grants and fake host files, run as the session shows.
#[test]
fn outbox_runs_as_shown() {
    let guide = doc("guide.md");
    let b = blocks(section(&guide, "## The outbox pattern"));
    assert_eq!(b.len(), 4, "program, t.grants, hook.http, session");
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [("outbox.kio", b[0]), ("t.grants", b[1]), ("hook.http", b[2])] {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let (cmd, want) = b[3].split_once('\n').unwrap();
    assert_eq!(cmd, "$ kio test outbox.kio --grants t.grants");
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["test", "outbox.kio", "--grants", "t.grants"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!((out.status.code(), text.as_str()), (Some(0), want));
}
