//! `kio review NEW --base OLD`: one page from the evidence `kio check` and `kio test` produce
//! (R1a). Every line carries its label; nothing on the page is a verdict.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const USAGE: &str =
    "usage: kio review <new.kio> --base <old.kio> [--grants file] [--test-grants file] [--requirements file] [--entry file [--entry-grants file]].. [--json]";

/// The largest requirements file diffed, and the most line pairs the diff compares.
const REQUIREMENTS_MAX: u64 = 1 << 20;
const DIFF_CELLS_MAX: usize = 4_000_000;

/// A test whose fuel or peak memory changed by more than 1/`RESOURCE_SHARE` (2%) has a row.
const RESOURCE_SHARE: u64 = 50;

/// A section's line: its label (empty under a labelled line) and text.
type Row = (&'static str, String);

/// What each capability method leaves for a person to check: the type, the methods (empty: all)
/// and what the host does not see.
const BLIND_SPOTS: [(&str, &[&str], &str); 8] = [
    (
        "Db",
        &["exec", "query", "transaction"],
        "the SQL text and what it changes",
    ),
    (
        "Http",
        &["send", "get", "send_all"],
        "the paths, headers and bodies sent",
    ),
    (
        "Dir",
        &["write", "create", "append", "remove"],
        "what is written or removed",
    ),
    ("Kv", &["set", "swap", "delete", "add"], "the values"),
    ("Outgoing", &["send"], "what the client receives"),
    ("WsSend", &["text", "binary", "close"], "what the client receives"),
    ("Hmac", &["sign"], "what is signed"),
    ("Log", &[], "what is logged"),
];

/// The capability methods that leave nothing for a person to check: they read, or the host
/// decides the result. Every caps.wit method is here or in `BLIND_SPOTS` (a unit test).
#[cfg(test)]
const NO_BLIND_SPOT: &[(&str, &[&str])] = &[
    ("Dir", &["read", "list", "exists"]),
    (
        "Incoming",
        &["method", "target", "header_names", "header_values", "body"],
    ),
    ("Kv", &["get", "keys", "scan"]),
    ("WsMessage", &["kind", "conn", "data", "target", "header"]),
    ("Rng", &["bytes", "int"]),
    ("Clock", &["now_ms"]),
    ("Hmac", &["verify"]),
    ("Secret", &["matches"]),
];

const SAME_AUTHOR: &str = "tests and properties were written by the same author as the code unless stated";

/// What the command line names, read: each side's files and path, the grants file for NEW's
/// `main`, the tests' grants file, and the requirements file on each side (`None` when absent).
struct Inputs {
    old: (String, Program),
    new: (String, Program),
    grants: Option<(String, Vec<u8>)>,
    /// The grants file beside OLD's program at G's path relative to NEW's (R1a4).
    old_grants: Beside,
    test_grants: Option<(String, Vec<u8>)>,
    /// Each `--http name=fake:FILE` of the test grants file, as `(name, FILE)`.
    fakes: Vec<(String, String)>,
    /// Each FILE of `fakes` read once, for both sides' tests and Identity.
    fake_files: Vec<(String, Result<Vec<u8>, String>)>,
    requirements: Option<[(String, Option<String>); 2]>,
    entries: Vec<Entry>,
}

/// Another entry of the change (`--entry`): the path as given, escaped; OLD's program (`None`:
/// the entry is new) and NEW's, each with its path; and its `--entry-grants` file.
struct Entry {
    name: String,
    old: Option<(String, Program)>,
    /// Where OLD's entry is or would be.
    old_path: String,
    new: (String, Program),
    grants: Option<(String, Vec<u8>)>,
    old_grants: Beside,
}

/// Where a grants file of NEW's sits beside OLD's program: G's path relative to NEW's directory,
/// escaped, and what is there (`Err` when G is outside NEW's directory).
type Beside = Result<(String, OldGrants), ()>;

/// What is beside OLD's program at a grants file's place. OLD's tree may be anyone's: only a
/// regular file inside its directory, reached through no symlink, is read, and why another is
/// not is never shown.
enum OldGrants {
    /// Nothing: the file is new.
    Absent,
    /// The file's path and contents.
    File(String, Vec<u8>),
    /// Not compared, and why: the end of the page's line.
    NotCompared(&'static str),
    /// Not compared: OLD's program is in NEW's directory, so the file there is NEW's.
    Shared,
}

impl OldGrants {
    fn file(&self) -> Option<(String, Vec<u8>)> {
        match self {
            OldGrants::File(p, b) => Some((p.clone(), b.clone())),
            _ => None,
        }
    }
}

/// The most bytes of OLD's grants file read; a longer one is not compared.
const OLD_GRANTS_MAX: u64 = 64 << 10;

/// The file at `g`'s path relative to `new`'s directory, beside the program `old`.
fn beside_old(old: &str, new: &str, g: &str) -> Beside {
    let dir = |p: &str| match Path::new(p).parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => ".".into(),
    };
    let (Ok(nd), Ok(gc)) = (dir(new).canonicalize(), Path::new(g).canonicalize()) else {
        return Err(());
    };
    let Ok(rel) = gc.strip_prefix(&nd) else {
        return Err(());
    };
    let shown = kio_host::escape_line(&rel.display().to_string());
    let not = |why| Ok((shown.clone(), OldGrants::NotCompared(why)));
    let refused = "is not a regular file inside its directory";
    let Ok(od) = dir(old).canonicalize() else {
        return not(refused);
    };
    if od == nd {
        return Ok((shown, OldGrants::Shared));
    }
    // Every component below OLD's directory a directory, the last a regular file, none a symlink
    // (`rel` has no `..`: it is below a canonical path).
    let mut path = dir(old);
    let parts: Vec<_> = rel.components().collect();
    for (i, c) in parts.iter().enumerate() {
        path.push(c);
        let last = i + 1 == parts.len();
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((shown, OldGrants::Absent)),
            Ok(m) if last && m.is_file() => {}
            Ok(m) if !last && m.is_dir() => {}
            _ => return not(refused),
        }
    }
    if !path.canonicalize().is_ok_and(|p| p.starts_with(&od)) {
        return not(refused);
    }
    // A hard link could be to any file (NEW's own included): `nofollow` refuses one too.
    match read_regular(&path, OLD_GRANTS_MAX, true) {
        Ok(bytes) => Ok((shown, OldGrants::File(path.display().to_string(), bytes))),
        Err(Unread::TooLarge) => not("is over 64 KiB"),
        Err(_) => not(refused),
    }
}

/// `kio review`: exit 1 if a side or entry does not check, a grants file does not bind, or a NEW test
/// fails; else 3 if authority was added, a dependency added or changed, or a test removed; else
/// 0. 2 for usage.
pub(crate) fn review(path: &str, flags: &[&str], json: bool) -> i32 {
    let (inputs, out) = captured(json, || inputs(path, flags));
    let inputs = match inputs {
        Ok(i) => i,
        Err(code) => {
            if json {
                println!("{}", json!({"ok": false, "errors": out.errors}));
            }
            return code;
        }
    };
    let mut page = Page::default();
    let requirements = inputs.requirements.as_ref().map(requirements);
    let (checks, binds) = authority(&inputs, &mut page);
    let (tests, removed, failed) = match checks {
        true => tests(&inputs, &mut page),
        false => {
            let line = "not run: a program does not check (Authority)";
            page.tests.push(("error", line.into()));
            (json!(null), false, false)
        }
    };
    not_checked(&inputs, &mut page);
    // Every file was read before the runs; one that changed since does not match its hash.
    let identity = identity(&inputs, &mut page);
    let code = if !checks || !binds || failed || !page.changed.is_empty() {
        1
    } else if page.added || removed {
        3
    } else {
        0
    };
    if json {
        let line = json!({
            "requirements": requirements.as_ref().map(|r| r.1.clone()),
            "authority": page.authority_json,
            "tests": tests,
            "resources": page.resources_json,
            "not_checked": page.not_checked_json,
            "identity": identity,
        });
        println!("{line}");
        return code;
    }
    if let Some((rows, _)) = &requirements {
        section("Requirements", rows);
    }
    section("Authority", &page.authority);
    section("Tests", &page.tests);
    if checks {
        section("Resources", &labelled("measured", &page.resources));
    }
    section("Not checked", &labelled("not checked", &page.not_checked));
    println!("Identity");
    page.identity.iter().for_each(|l| println!("  {l}"));
    for p in &page.changed {
        println!(
            "  {:<12} {} changed while the review ran",
            "error",
            kio_host::escape_line(p)
        );
    }
    code
}

/// Each section's text lines and `--json` value as it is built.
#[derive(Default)]
struct Page {
    authority: Vec<Row>,
    authority_json: serde_json::Value,
    added: bool,
    tests: Vec<Row>,
    resources: Vec<String>,
    resources_json: serde_json::Value,
    not_checked: Vec<String>,
    not_checked_json: serde_json::Value,
    identity: Vec<String>,
    /// Each file shown in Identity whose contents on disk no longer match its hash.
    changed: Vec<String>,
}

/// `title`, then each line after its label.
fn section(title: &str, rows: &[Row]) {
    println!("{title}");
    for (label, l) in rows {
        println!("  {label:<12} {l}");
    }
}

/// Each of `lines` under `label`.
fn labelled(label: &'static str, lines: &[String]) -> Vec<Row> {
    lines.iter().map(|l| (label, l.clone())).collect()
}

/// The flags parsed and every file they name read, or the exit code once the error is reported.
fn inputs(path: &str, flags: &[&str]) -> Result<Inputs, i32> {
    let (mut base, mut grants, mut test_grants, mut requirements) = (None, None, None, None);
    // Each `--entry` and the `--entry-grants` that follows it.
    let mut entries: Vec<(&str, Option<&str>)> = Vec::new();
    if !flags.len().is_multiple_of(2) {
        return Err(fail(USAGE.into()));
    }
    for pair in flags.chunks(2) {
        match (pair[0], entries.last_mut()) {
            ("--entry", _) => {
                entries.push((pair[1], None));
                continue;
            }
            ("--entry-grants", Some((_, g @ None))) => {
                *g = Some(pair[1]);
                continue;
            }
            ("--entry-grants", _) => {
                return Err(fail(
                    "`--entry-grants` binds the `--entry` just before it, once\n  hint: `--entry worker.kio --entry-grants worker.grants`".into(),
                ))
            }
            _ => {}
        }
        let slot = match pair[0] {
            "--base" => &mut base,
            "--grants" => &mut grants,
            "--test-grants" => &mut test_grants,
            "--requirements" => &mut requirements,
            _ => return Err(fail(USAGE.into())),
        };
        if slot.replace(pair[1]).is_some() {
            return Err(fail(format!("`{}` is given twice\n  hint: give it once", pair[0])));
        }
    }
    let Some(base) = base else {
        return Err(fail(
            "kio review needs the old program\n  hint: add `--base old.kio`; for a git revision, `git show REV:app.kio > old/app.kio` (guide, Reviewing a change)".into(),
        ));
    };
    let old = (base.to_string(), read_program(base)?);
    let new = (path.to_string(), read_program(path)?);
    let read = |flag: &str, g: Option<&str>| match g {
        Some(g) => match read_regular(g.as_ref(), GRANTS_MAX, false) {
            Ok(bytes) => Ok(Some((g.to_string(), bytes))),
            Err(Unread::TooLarge) => Err(fail(format!(
                "{flag} {}: over 1 MiB\n  hint: a grants file is one flag a line",
                kio_host::escape_line(g)
            ))),
            Err(e) => Err(fail(format!("{flag} {}: {e}", kio_host::escape_line(g)))),
        },
        None => Ok(None),
    };
    let grants = read("--grants", grants)?;
    let old_grants = match &grants {
        Some((g, _)) => beside_old(base, path, g),
        None => Err(()),
    };
    let test_grants = read("--test-grants", test_grants)?;
    if let Some(t) = &test_grants {
        shared_state(&t.0)?;
    }
    let fakes = match &test_grants {
        Some((t, _)) => test_fakes(t)?,
        None => Vec::new(),
    };
    let mut fake_files: Vec<(String, Result<Vec<u8>, String>)> = Vec::new();
    for (_, f) in &fakes {
        if !fake_files.iter().any(|(p, _)| p == f) {
            fake_files.push((f.clone(), read_fake(f)));
        }
    }
    let requirements = match requirements {
        Some(file) if Path::new(file).is_absolute() => {
            return Err(fail(format!(
                "--requirements {file}: an absolute path names one file for both sides\n  hint: give a path relative to each program's directory, e.g. `--requirements README.md`"
            )))
        }
        Some(file) => Some([read_requirements(base, file)?, read_requirements(path, file)?]),
        None => None,
    };
    if let Some([(o, None), (n, None)]) = &requirements {
        return Err(fail(format!(
            "--requirements: neither {o} nor {n} exists\n  hint: the path is relative to each program's directory, e.g. `--requirements README.md`"
        )));
    }
    let mut read_entries: Vec<Entry> = Vec::new();
    for (rel, g) in entries {
        let mut e = entry(base, path, rel, read("--entry-grants", g)?)?;
        if let Some((g, _)) = &e.grants {
            e.old_grants = beside_old(&e.old_path, &e.new.0, g);
        }
        if e.new.0 == new.0 || read_entries.iter().any(|x| x.new.0 == e.new.0) {
            return Err(fail(format!(
                "--entry {}: given twice, or it is the root\n  hint: name each other entry once; the root is reviewed already",
                e.name
            )));
        }
        read_entries.push(e);
    }
    Ok(Inputs {
        old,
        new,
        grants,
        old_grants,
        test_grants,
        fakes,
        fake_files,
        requirements,
        entries: read_entries,
    })
}

/// The entry `rel` beside each side's root, `old` and `new`: NEW's must exist; OLD's may not
/// (the entry is new). `rel` is relative, without `..` or control characters, so it names the
/// same place on both sides and nothing outside them.
fn entry(old: &str, new: &str, rel: &str, grants: Option<(String, Vec<u8>)>) -> Result<Entry, i32> {
    use std::path::Component;
    let name = kio_host::escape_line(rel);
    let parts: Vec<Component> = Path::new(rel).components().collect();
    let inside = parts
        .iter()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && parts.iter().any(|c| matches!(c, Component::Normal(_)));
    if !inside || rel.chars().any(char::is_control) {
        return Err(fail(format!(
            "--entry {name}: not a path inside the program's directory\n  hint: give a relative path without `..`, resolved beside each side's root, e.g. `--entry worker.kio`"
        )));
    }
    let beside = |root: &str| Path::new(root).parent().unwrap_or(".".as_ref()).join(rel);
    let (old_path, new_path) = (beside(old), beside(new));
    if std::fs::symlink_metadata(&new_path).is_err() {
        return Err(fail(format!(
            "--entry {name}: {} does not exist\n  hint: the path is relative to the directory of {}",
            kio_host::escape_line(&new_path.display().to_string()),
            kio_host::escape_line(new)
        )));
    }
    let read = |p: &Path| {
        let p = p.display().to_string();
        read_program(&p).map(|prog| (p, prog))
    };
    let old = match std::fs::symlink_metadata(&old_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        _ => Some(read(&old_path)?),
    };
    Ok(Entry {
        name,
        old,
        old_path: old_path.display().to_string(),
        new: read(&new_path)?,
        grants,
        old_grants: Err(()),
    })
}

/// Refuses a tests' grants file that gives a writable `--dir` or a file-backed `--kv` or `--db`:
/// OLD's tests run first and would leave their writes for NEW's.
fn shared_state(t: &str) -> Result<(), i32> {
    let (flags, _) = with_grants(&["--grants", t])?;
    for pair in flags.chunks(2) {
        let Some((name, target)) = pair[1].split_once('=') else {
            continue;
        };
        if ["--dir", "--kv", "--db"].contains(&pair[0].as_str()) && writable_target(target) {
            return Err(fail(format!(
                "--test-grants {t}: `{} {name}={target}` would carry OLD's test writes into NEW's\n  hint: grant it `:ro`, use `:memory:` for a store, or leave it out for a fresh one per test",
                pair[0]
            )));
        }
    }
    Ok(())
}

/// Each `--http name=fake:FILE` of the test grants file `t`, as `(name, FILE)`.
fn test_fakes(t: &str) -> Result<Vec<(String, String)>, i32> {
    let (flags, _) = with_grants(&["--grants", t])?;
    let fake = |p: &[String]| match p[1].split_once('=') {
        Some((name, o)) if p[0] == "--http" => Some((name.to_string(), o.strip_prefix("fake:")?.to_string())),
        _ => None,
    };
    Ok(flags.chunks(2).filter_map(fake).collect())
}

/// The requirements file `file` beside the program `program`: its path and text, `None` when it
/// does not exist.
fn read_requirements(program: &str, file: &str) -> Result<(String, Option<String>), i32> {
    let path = Path::new(program).parent().unwrap_or(".".as_ref()).join(file);
    let shown = path.display().to_string();
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((shown, None)),
        Ok(m) if m.len() > REQUIREMENTS_MAX => {
            return Err(fail(format!(
                "--requirements {shown}: {} bytes; kio diffs files up to 1 MB\n  hint: name a shorter file that states the requirements",
                m.len()
            )))
        }
        _ => {}
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((shown, Some(text))),
        Err(e) => Err(fail(format!("--requirements {shown}: cannot read: {e}"))),
    }
}

/// The Requirements section: the unified diff of the two files, as lines and as `--json` gives it.
/// The label is on the first line only, so the diff below it reads as a diff.
fn requirements(sides: &[(String, Option<String>); 2]) -> (Vec<Row>, serde_json::Value) {
    let [(old_path, old), (new_path, new)] = sides;
    let text = |t: &Option<String>| t.clone().unwrap_or_default();
    let mut lines: Vec<Row> = vec![("not checked", "what the change claims; kio does not check it".into())];
    let diff = match unified(&text(old), &text(new)) {
        Ok(d) if d.is_empty() => vec![],
        Ok(d) => {
            let absent = |t: &Option<String>| if t.is_none() { " (absent)" } else { "" };
            let mut all = vec![
                format!("--- {old_path}{}", absent(old)),
                format!("+++ {new_path}{}", absent(new)),
            ];
            all.extend(d);
            all
        }
        Err(e) => {
            lines.push(("", e.clone()));
            return (
                lines,
                json!({"old": old_path, "new": new_path, "diff": null, "error": e}),
            );
        }
    };
    if diff.is_empty() {
        lines.push(("", "no change".into()));
    }
    lines.extend(diff.iter().map(|l| ("", shown_line(l))));
    (
        lines,
        json!({"old": old_path, "new": new_path, "diff": diff, "error": null}),
    )
}

/// `l` with control characters (but tab) and bidirectional overrides escaped, so a file shown
/// verbatim cannot move the cursor or reorder the page.
fn shown_line(l: &str) -> String {
    let mut o = String::with_capacity(l.len());
    for c in l.chars() {
        match c {
            '\t' => o.push(c),
            c if c.is_control() || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}') => {
                o.push_str(&format!("\\u{{{:02x}}}", c as u32))
            }
            c => o.push(c),
        }
    }
    o
}

/// The hunks of a unified diff of `old` and `new` by line, 3 lines of context (empty when they
/// are equal), from a longest common subsequence. Err when too many lines differ to compare.
fn unified(old: &str, new: &str) -> Result<Vec<String>, String> {
    const CONTEXT: usize = 3;
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (x, y) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if x.len().saturating_mul(y.len()) > DIFF_CELLS_MAX {
        return Err(format!(
            "{} and {} changed lines are too many to diff here; compare the files with `diff -u`",
            x.len(),
            y.len()
        ));
    }
    // lcs[i][j]: the longest common subsequence of x[i..] and y[j..].
    let w = y.len() + 1;
    let mut lcs = vec![0u32; (x.len() + 1) * w];
    for i in (0..x.len()).rev() {
        for j in (0..y.len()).rev() {
            lcs[i * w + j] = match x[i] == y[j] {
                true => lcs[(i + 1) * w + j + 1] + 1,
                false => lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]),
            };
        }
    }
    // Every line in order: ' ' kept, '-' only in old, '+' only in new.
    let mut ops: Vec<(char, &str)> = a[..pre].iter().map(|l| (' ', *l)).collect();
    let (mut i, mut j) = (0, 0);
    while i < x.len() || j < y.len() {
        if i < x.len() && j < y.len() && x[i] == y[j] {
            ops.push((' ', x[i]));
            (i, j) = (i + 1, j + 1);
        } else if j == y.len() || (i < x.len() && lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
            ops.push(('-', x[i]));
            i += 1;
        } else {
            ops.push(('+', y[j]));
            j += 1;
        }
    }
    ops.extend(a[a.len() - suf..].iter().map(|l| (' ', *l)));
    // Hunks: changes closer than twice the context share one.
    let changes: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != ' ').collect();
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &k in &changes {
        let (start, end) = (k.saturating_sub(CONTEXT), (k + CONTEXT + 1).min(ops.len()));
        match hunks.last_mut() {
            Some(h) if start <= h.1 => h.1 = end,
            _ => hunks.push((start, end)),
        }
    }
    let mut out = Vec::new();
    for (start, end) in hunks {
        let before = |c: char| ops[..start].iter().filter(|(o, _)| *o != c).count();
        let count = |c: char| ops[start..end].iter().filter(|(o, _)| *o != c).count();
        let range = |c: char| {
            let n = count(c);
            // As `diff -u`: an empty range names the line before it; a count of 1 is left out.
            match n {
                0 => format!("{},0", before(c)),
                1 => format!("{}", before(c) + 1),
                _ => format!("{},{n}", before(c) + 1),
            }
        };
        out.push(format!("@@ -{} +{} @@", range('+'), range('-')));
        out.extend(ops[start..end].iter().map(|(o, l)| format!("{o}{l}")));
    }
    Ok(out)
}

/// The Authority section: the root's lines, then each `--entry`'s after its name. Whether the
/// root's two sides check (its tests run), and whether every entry checks and every grants file
/// binds.
fn authority(inputs: &Inputs, page: &mut Page) -> (bool, bool) {
    let grants = inputs.grants.as_ref();
    let (mut value, checks, binds) = entry_authority(
        &inputs.old.1,
        &inputs.new,
        (grants, &inputs.old_grants),
        "",
        "--grants",
        page,
    );
    let mut ok = binds;
    let mut entries = Vec::new();
    let none = no_program();
    for e in &inputs.entries {
        let prefix = format!("{}: ", e.name);
        let old = e.old.as_ref().map_or(&none, |o| &o.1);
        let grants = e.grants.as_ref();
        let grants = (grants, &e.old_grants);
        let (mut v, checks, binds) = entry_authority(old, &e.new, grants, &prefix, "--entry-grants", page);
        ok &= checks && binds;
        v["entry"] = json!(e.name);
        v["new_entry"] = json!(e.old.is_none());
        entries.push(v);
    }
    value["entries"] = json!(entries);
    page.authority_json = value;
    (checks, ok)
}

/// What a new entry is diffed against: a `main` that reaches nothing, so all its authority is
/// `+` and every function is changed.
fn no_program() -> Program {
    Program {
        files: vec![("(none)".into(), "fn main() {}\n".into())],
        paths: vec![],
        ids: vec![],
        deps: vec![],
    }
}

/// One entry's Authority lines, each text after `prefix`: `kio check NEW --base OLD`'s lines,
/// under each grant line where each gained method is used; with a grants file (`flag`) the
/// errors of binding it to NEW's `main`, labelled `error`, and its change from OLD's (R1a4).
/// Its `--json` value, whether both sides check, and whether the grants bind.
fn entry_authority(
    old: &Program,
    (new_path, new): &(String, Program),
    (grants, old_grants): (Option<&(String, Vec<u8>)>, &Beside),
    prefix: &str,
    flag: &str,
    page: &mut Page,
) -> (serde_json::Value, bool, bool) {
    let rows = &mut page.authority;
    let mut push = |label, line: &str| rows.push((label, format!("{prefix}{line}")));
    let mut errors = Vec::new();
    let (mut value, checks) = match kio_host::diff_files(&old.files, &new.files) {
        Ok(diff) => {
            page.added |= diff.added;
            // Grant lines are what the host enforces; function lines the checker's attribution.
            let (lines, funcs) = diff.lines(false);
            if lines.is_empty() {
                push("enforced", "no change in authority");
            }
            let gained = gained(old, new);
            for l in &diff.grants {
                push("enforced", l);
                let grant = (l.strip_prefix("+ ").or(l.strip_prefix("~ "))).and_then(|g| g.split_once(": "));
                for (_, m, fs) in gained.iter().filter(|g| Some(g.0.as_str()) == grant.map(|g| g.0)) {
                    push("derived", &format!("  {m} in {}", fs.join(", ")));
                }
            }
            if funcs && diff.added && !diff.grant_added {
                // Exit 3 with no grant added: name the functions whose lines add.
                let names: Vec<&str> = (diff.funcs.iter())
                    .filter_map(|l| Some(l.strip_prefix("+ ")?.split_once(": ")?.0))
                    .collect();
                let grants = if diff.grants.is_empty() {
                    "grants unchanged"
                } else {
                    "no grant added"
                };
                push(
                    "derived",
                    &format!("{grants}; new functions reach: {}", names.join(", ")),
                );
            }
            if funcs {
                diff.funcs.iter().for_each(|l| push("derived", l));
            }
            let gained: Vec<_> = (gained.iter())
                .map(|(g, m, fs)| json!({"grant": g, "method": m, "functions": fs}))
                .collect();
            let v = json!({"grants": diff.grants, "funcs": diff.funcs, "added": diff.added, "gained": gained});
            (v, true)
        }
        Err((is_base, diags)) => {
            let (side, prog) = if is_base { ("old", old) } else { ("new", new) };
            let shown = kio_host::escape_line(&prog.files[0].0);
            push("error", &format!("the {side} program {shown} does not check"));
            let (_, out) = captured(true, || print_diags(prog, diags, "check"));
            push_errors(&mut page.authority, &out.errors);
            errors = out.errors;
            (json!({"grants": [], "funcs": [], "added": false, "gained": []}), false)
        }
    };
    let mut grant_errors = Vec::new();
    if let Some((g, bytes)) = grants {
        let (_, out) = captured(true, || bind(new_path, g));
        let (shown_g, shown) = (kio_host::escape_line(g), kio_host::escape_line(new_path));
        // The file's change from OLD's, then whether it binds.
        if let Ok((rel, old_g)) = old_grants {
            let not = match old_g {
                OldGrants::NotCompared(why) => Some(format!("the base's {rel} {why}; not compared")),
                OldGrants::Shared => Some("the base shares this directory; not compared".to_string()),
                _ => None,
            };
            if let Some(line) = &not {
                page.authority.push(("not checked", format!("{prefix}{rel}: {line}")));
                value["grants_file"] = json!({"path": rel, "old": null, "diff": null, "not_compared": line});
            } else {
                let old_g = old_g.file();
                let (lines, v) = grants_diff((g, bytes), old_g.as_ref());
                lines
                    .iter()
                    .for_each(|l| page.authority.push(("enforced", format!("{prefix}{rel}: {l}"))));
                value["grants_file"] =
                    json!({"path": rel, "old": old_g.as_ref().map(|o| &o.0), "diff": v, "not_compared": null});
            }
        } else {
            let line = format!("{prefix}{shown_g}: outside the program's directory; not compared with the base");
            page.authority.push(("not checked", line));
            value["grants_file"] = json!(null);
        }
        if out.errors.is_empty() {
            let items = bound(g, bytes);
            let what = match items.is_empty() {
                true => String::new(),
                false => format!(": {}", items.join(", ")),
            };
            let msg = format!("{prefix}{flag} {shown_g} binds {shown}'s main{what}");
            page.authority.push(("enforced", msg));
            value["bound"] = json!(items);
        } else {
            let msg = format!("{prefix}{flag} {shown_g} does not bind to {shown}'s main:");
            page.authority.push(("error", msg));
            push_errors(&mut page.authority, &out.errors);
        }
        grant_errors = out.errors;
    }
    let binds = grant_errors.is_empty();
    value["errors"] = json!(errors);
    value["grant_errors"] = json!(grant_errors);
    (value, checks, binds)
}

/// Per capability method a grant of NEW's `main` gained (on a `+` or `~` line), the functions
/// `main` reaches in NEW that use it (their report line has it) and did not in OLD; `main` only
/// when no other function does. Empty when a side does not check.
fn gained(old: &Program, new: &Program) -> Vec<(String, String, Vec<String>)> {
    let (Ok(o), Ok(n)) = (kio_host::check_program(&old.files), kio_host::check_program(&new.files)) else {
        return vec![];
    };
    let (oa, na) = (lang::authority(&o), lang::authority(&n));
    let reached = |a: &lang::Authority, d: &str| -> BTreeSet<String> {
        let main = a.funcs.get("main").map_or(&[][..], Vec::as_slice);
        main.iter()
            .find(|(e, _)| e == d)
            .map(|(_, ms)| ms.clone())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for g @ (d, _) in &na.main {
        let was = if oa.main.contains(g) {
            reached(&oa, d)
        } else {
            BTreeSet::new()
        };
        for m in reached(&na, d).difference(&was) {
            let before = lang::users(&o, d, &[m]);
            let mut fs: Vec<String> = (lang::users(&n, d, &[m]).into_iter())
                .filter(|f| !before.contains(f))
                .collect();
            if fs.len() > 1 {
                fs.retain(|f| f != "main");
            }
            out.push((d.clone(), m.clone(), fs));
        }
    }
    out
}

/// The flag-level change from OLD's grants file `old` (`None`: there is none) to `new`, each a
/// path and the bytes read once: lines (`+ --dir x=y`, `- ..`, `unchanged`, `new file`) and
/// `--json`'s value. A file that does not parse is named; its binding error is above (NEW) or it
/// is the base's (OLD).
fn grants_diff(new: (&String, &Vec<u8>), old: Option<&(String, Vec<u8>)>) -> (Vec<String>, serde_json::Value) {
    let parse = |(g, bytes): (&String, &Vec<u8>)| {
        let text = std::str::from_utf8(bytes).ok()?;
        let (flags, _) = captured(true, || grants_flags(text, g, &[]));
        Some(flags.ok()?.chunks(2).map(|p| p.join(" ")).collect::<Vec<String>>())
    };
    let old = old.map(|(g, b)| (g, b));
    let Some(old) = old else {
        return (vec!["new file".into()], json!("new file"));
    };
    let (Some(n), Some(mut o)) = (parse(new), parse(old)) else {
        let line = "a side's file does not parse; not compared";
        return (vec![line.into()], json!(null));
    };
    let mut lines = Vec::new();
    for f in n {
        match o.iter().position(|x| *x == f) {
            Some(i) => _ = o.remove(i),
            None => lines.push(format!("+ {f}")),
        }
    }
    lines.extend(o.iter().map(|f| format!("- {f}")));
    let v = json!(lines);
    if lines.is_empty() {
        lines.push("unchanged".into());
    }
    (lines.iter().map(|l| kio_host::escape_line(l)).collect(), v)
}

/// What the grants file `g`, read once as `bytes`, binds, for a file that bound (R1a5): one item
/// a grant in file order, `name=value` as written (a secret's value is its source, `env:VAR` or
/// `file:PATH`: binding refuses any other form), an `Http` grant's `--http-methods` and
/// `--http-header`s after it in parentheses, and `--listen`, `--host` and `--ws-origin` (who may
/// reach the program). Limits and `--audit` are left out (#290). Each part ASCII, every other
/// character and `\` escaped, and quoted when it holds a character that separates parts here.
fn bound(g: &str, bytes: &[u8]) -> Vec<String> {
    let text = std::str::from_utf8(bytes).unwrap_or_default();
    let Ok(flags) = captured(true, || grants_flags(text, g, &[])).0 else {
        return vec![];
    };
    // ASCII only, `\` doubled: no character can pass for another (`о` for `o`, `，` for `,`).
    let shown = |s: &str| {
        let s: String = (s.chars())
            .map(|c| match c {
                '\\' => "\\\\".to_string(),
                ' '..='~' => c.to_string(),
                c => format!("\\u{{{:02x}}}", c as u32),
            })
            .collect();
        match s.contains([',', ';', '(', ')', '"', ' ']) {
            true => format!("\"{}\"", s.replace('"', "\\\"")),
            false => s,
        }
    };
    let pairs: Vec<(&str, &str)> = flags.chunks(2).map(|p| (p[0].as_str(), p[1].as_str())).collect();
    let mut items = Vec::new();
    for &(flag, spec) in &pairs {
        let item = match flag {
            "--dir" | "--kv" | "--db" | "--rng" | "--clock" | "--log" | "--hmac" | "--secret" => shown(spec),
            "--listen" | "--host" | "--ws-origin" => format!("{flag} {}", shown(spec)),
            "--http" => {
                let name = spec.split_once('=').map_or(spec, |(n, _)| n);
                let extras: Vec<String> = (pairs.iter())
                    .filter(|(f, _)| *f == "--http-methods" || *f == "--http-header")
                    .filter_map(|(_, s)| s.split_once('=').filter(|(n, _)| *n == name))
                    .map(|(_, v)| shown(v))
                    .collect();
                match extras.is_empty() {
                    true => shown(spec),
                    false => format!("{} ({})", shown(spec), extras.join("; ")),
                }
            }
            _ => continue,
        };
        items.push(item);
    }
    items
}

/// `kio check path --grants g`'s binding, its errors in `--json`'s object; dry, so a server's
/// grants bind here too.
fn bind(path: &str, g: &str) -> i32 {
    let (flags, grants_file) = match with_grants(&["--grants", g]) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    match read_program(path) {
        Ok(prog) => run_program(&prog, &flags, grants_file.as_deref(), true, None),
        Err(code) => code,
    }
}

/// Each error's lines, indented and unlabelled, after the line that introduces them.
fn push_errors(rows: &mut Vec<Row>, errors: &[serde_json::Value]) {
    for e in errors {
        rows.extend(error_lines(e).into_iter().map(|l| ("", format!("  {l}"))));
    }
}

/// The grants line of the Tests section when there is no `--test-grants`.
const DEFAULT_TEST_GRANTS: &str = "defaults (temporary Dir, :memory: stores, fixed clock)";

/// The Tests section, and the Resources section from the same runs: `--json`'s value, whether a
/// test was removed or tests less, and whether a side's tests do not run or a NEW test failed.
/// The tests run with `--test-grants` only, else the defaults: never `main`'s grants.
fn tests(inputs: &Inputs, page: &mut Page) -> (serde_json::Value, bool, bool) {
    let (flags, shown): (Vec<&str>, &str) = match &inputs.test_grants {
        Some((t, _)) => (vec!["--grants", t], t),
        None => (vec![], DEFAULT_TEST_GRANTS),
    };
    // A secret T names that cannot be read fails every test granted it, on both sides: one error.
    if let Some(e) = unreadable_secret(&flags) {
        page.tests.push((
            "error",
            format!("--test-grants {shown} cannot be applied; no test ran:"),
        ));
        page.tests.extend(
            e.lines()
                .map(|l| ("", format!("  {}", kio_host::escape_line(l.trim_start())))),
        );
        resources(&[], &[], page);
        return (json!({"grants": shown, "setup_error": e}), false, true);
    }
    let answered: Vec<String> = (inputs.fakes.iter())
        .map(|(n, f)| {
            format!(
                "{} answered from {}",
                kio_host::escape_line(n),
                kio_host::escape_line(f)
            )
        })
        .collect();
    let line = match answered.is_empty() {
        true => format!("grants: {shown}"),
        false => format!("grants: {shown} ({})", answered.join(", ")),
    };
    page.tests.push(("tested", line));
    let side = |path: &str| {
        let (tests, out) = captured(true, || test_setup(path, &flags, &inputs.fake_files));
        tests.map(|t| (run_tests(&t), t)).map_err(|_| out.errors)
    };
    let (old, new) = (side(&inputs.old.0), side(&inputs.new.0));
    let mut sides = json!({});
    for (name, s) in [("old", &old), ("new", &new)] {
        sides[name] = match s {
            // A side that predates tests has none: 0 tests, not a failure.
            Ok((results, _)) if results.is_empty() => {
                page.tests.push(("tested", format!("{name}: 0 tests")));
                json!({"passed": 0, "failed": 0, "errors": []})
            }
            Ok((results, _)) => {
                let failed = results.iter().filter(|r| r.error.is_some()).count();
                let mut line = format!("{name}: {} passed, {failed} failed", results.len() - failed);
                // OLD's failures back nothing; NEW's are the `error` lines below.
                let label = match (name, failed) {
                    ("old", 1..) => {
                        line += "; the base fails its own tests";
                        "not checked"
                    }
                    _ => "tested",
                };
                page.tests.push((label, line));
                json!({"passed": results.len() - failed, "failed": failed, "errors": []})
            }
            Err(errors) => {
                page.tests.push(("error", format!("{name}: the tests do not run:")));
                push_errors(&mut page.tests, errors);
                json!({"passed": 0, "failed": 0, "errors": errors})
            }
        };
    }
    // Compared only when both sides ran: a side that did not run has no tests to compare.
    let (old_r, new_r): (&[TestResult], &[TestResult]) = match (&old, &new) {
        (Ok((o, _)), Ok((n, _))) => (o, n),
        _ => (&[], &[]),
    };
    let find = |rs: &'_ [TestResult], name: &str| rs.iter().position(|r| r.name == name);
    let added: Vec<&str> = (new_r.iter().map(|r| r.name.as_str()))
        .filter(|n| find(old_r, n).is_none())
        .collect();
    let removed: Vec<&str> = (old_r.iter().map(|r| r.name.as_str()))
        .filter(|n| find(new_r, n).is_none())
        .collect();
    // An added test that fails is an `error`, not evidence.
    let fails = |n: &str| find(new_r, n).is_some_and(|i| new_r[i].error.is_some());
    page.tests.extend(
        added
            .iter()
            .map(|n| (if fails(n) { "error" } else { "tested" }, format!("added: {n}"))),
    );
    page.tests
        .extend(removed.iter().map(|n| ("not checked", format!("removed: {n}"))));
    // A test on both sides that changed, or reaches a function that did, tests something else
    // now: read it like an added one.
    let changes = Changes::of(&inputs.old.1, &inputs.new.1);
    let mut changed: Vec<&str> = Vec::new();
    for n in (new_r.iter().map(|r| r.name.as_str())).filter(|n| find(old_r, n).is_some()) {
        let mut fs = changes.reached(&[n]);
        if fs.is_empty() {
            continue;
        }
        changed.push(n);
        fs.retain(|f| f != n);
        let line = match fs.is_empty() {
            true => format!("changed: {n}"),
            false => format!("changed: {n} (reaches changed {})", fs.join(", ")),
        };
        page.tests.push(("not checked", line));
    }
    // A changed function no test reaches ran in no test: per program, one line naming them.
    let unreached = unreached_changed(inputs, &changes);
    for (entry, fs) in &unreached {
        let prefix = entry.map_or(String::new(), |e| format!("{e}: "));
        let line = format!("changed, reached by no test: {prefix}{}", fs.join(", "));
        page.tests.push(("not checked", line));
    }
    let unreached: Vec<_> = (unreached.iter())
        .flat_map(|(e, fs)| fs.iter().map(move |f| json!({"entry": e, "function": f})))
        .collect();
    // A property that ran fewer cases than in OLD, or became an example test, tests less. One
    // that failed stopped at the failing case: its count says nothing.
    let mut fewer = Vec::new();
    for o in old_r {
        let (Some(op), Some(n)) = (&o.prop, find(new_r, &o.name).map(|i| &new_r[i])) else {
            continue;
        };
        if o.error.is_some() || n.error.is_some() {
            continue;
        }
        let line = match &n.prop {
            Some(np) if np.compiled < op.compiled => {
                format!("removed: {} ({} cases, {} in old)", o.name, np.compiled, op.compiled)
            }
            Some(_) => continue,
            None => format!(
                "removed: {} (now an example test; {} cases in old)",
                o.name, op.compiled
            ),
        };
        page.tests.push(("not checked", line));
        let cases = n.prop.as_ref().map_or(1, |p| p.compiled);
        fewer.push(json!({"name": o.name, "cases": cases, "old_cases": op.compiled, "property": n.prop.is_some()}));
    }
    let mut properties = Vec::new();
    for r in new.as_ref().map_or(&[][..], |(r, _)| r) {
        if let Some(p) = &r.prop {
            // A failed property's cases are its FAIL line below, not evidence.
            let label = if r.error.is_some() { "error" } else { "tested" };
            page.tests.push((label, format!("{}: {}", r.name, p.summary())));
            properties.push(json!({"name": r.name, "cases": p.compiled, "seed": p.seed, "discarded": p.discarded}));
        }
    }
    let mut failures = Vec::new();
    if let Ok((results, t)) = &new {
        for r in results.iter().filter(|r| r.error.is_some()) {
            // The FAIL line is labelled; the lines under it share its label.
            let lines = result_lines(t, r);
            page.tests.extend(
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, l)| (if i == 0 { "error" } else { "" }, l)),
            );
            failures.push(result_json(r));
        }
    }
    let old_failed: Vec<&str> = (old.as_ref().map_or(&[][..], |(r, _)| r).iter())
        .filter(|r| r.error.is_some())
        .map(|r| r.name.as_str())
        .collect();
    page.tests.extend(
        old_failed
            .iter()
            .map(|n| ("not checked", format!("failed in old: {n}"))),
    );
    resources(old_r, new_r, page);
    let failed = new.is_err() || old.is_err() || !failures.is_empty();
    let value = json!({
        "grants": inputs.test_grants.as_ref().map(|(t, _)| t), "fakes": fakes_json(inputs),
        "old": sides["old"], "new": sides["new"], "added": added, "removed": removed, "changed": changed,
        "unreached_changed": unreached, "fewer_cases": fewer, "properties": properties, "failures": failures, "old_failed": old_failed,
        "setup_error": null,
    });
    (value, !removed.is_empty() || !fewer.is_empty(), failed)
}

/// Each function's tokens by key (tests included), as text: whitespace and comments aside,
/// string literals exact. None if `prog` does not check.
fn fn_texts(prog: &Program) -> BTreeMap<String, String> {
    let Ok(p) = kio_host::check_program(&prog.files) else {
        return BTreeMap::new();
    };
    (p.fn_spans().into_iter())
        .filter_map(|(k, s)| {
            let text = prog.files.get(s.file as usize)?.1.get(s.start..s.end)?;
            let tokens = match lang::lex::lex(text) {
                Ok(ts) => format!("{:?}", ts.into_iter().map(|t| t.kind).collect::<Vec<_>>()),
                Err(_) => text.to_string(),
            };
            Some((k, tokens))
        })
        .collect()
}

/// NEW's functions against OLD's: NEW checked, and the functions whose tokens are not OLD's (a
/// new function counts).
struct Changes {
    new: Option<lang::Program>,
    changed: BTreeSet<String>,
    /// NEW's `fn_texts`.
    texts: BTreeMap<String, String>,
}

impl Changes {
    fn of(old: &Program, new: &Program) -> Changes {
        let old_texts = fn_texts(old);
        let texts = fn_texts(new);
        let changed = (texts.iter())
            .filter(|(k, t)| old_texts.get(*k) != Some(t))
            .map(|(k, _)| k.clone())
            .collect();
        Changes {
            new: kio_host::check_program(&new.files).ok(),
            changed,
            texts,
        }
    }

    /// The changed functions a call of any of `roots` may run, `roots` included, in key order.
    fn reached(&self, roots: &[&str]) -> Vec<String> {
        let Some(p) = &self.new else { return vec![] };
        let reach = lang::reachable(p, roots.iter().copied());
        reach.intersection(&self.changed).cloned().collect()
    }
}

/// Per program, the root (`changes` is its) then each `--entry`, its changed functions `main`
/// reaches that no test of the root's reaches (`lang::reachable` from every test, as the report's
/// `reached only from tests:`), in key order; a program with none is left out. An entry's
/// function counts as reached when the root's tests reach one of its key and every function it
/// may run in the entry has the same tokens as the root's of that key (#290).
fn unreached_changed<'a>(inputs: &'a Inputs, changes: &Changes) -> Vec<(Option<&'a str>, Vec<String>)> {
    let Some(root) = &changes.new else { return vec![] };
    let tests: Vec<String> = root.tests().into_iter().map(|t| t.name).collect();
    let reach = lang::reachable(root, tests.iter().map(String::as_str));
    let unreached = |c: &Changes| -> Vec<String> {
        let Some(p) = &c.new else { return vec![] };
        let same = |k: &String| changes.texts.get(k) == c.texts.get(k);
        (c.reached(&["main"]).into_iter())
            .filter(|k| !reach.contains(k) || !lang::reachable(p, [k.as_str()]).iter().all(same))
            .collect()
    };
    let mut out = vec![(None, unreached(changes))];
    let none = no_program();
    for e in &inputs.entries {
        let c = Changes::of(e.old.as_ref().map_or(&none, |o| &o.1), &e.new.1);
        out.push((Some(e.name.as_str()), unreached(&c)));
    }
    out.retain(|(_, fs)| !fs.is_empty());
    out
}

/// The first `--hmac`, `--secret` or `--http-header` secret the tests' grants `flags` name that
/// cannot be read, as `kio run` refuses it. Granted dirs are not known here: a key file inside
/// one is refused per test, as before.
fn unreadable_secret(flags: &[&str]) -> Option<String> {
    let (flags, _) = with_grants(flags).ok()?;
    for pair in flags.chunks(2) {
        let (flag, spec) = (pair[0].as_str(), pair[1].as_str());
        let e = match flag {
            "--hmac" => hmac(spec, &[], &[], false).err(),
            "--secret" => secret_grant(spec, &[], &[], false).err(),
            "--http-header" => {
                let Some((_, source)) = spec.split_once('=').and_then(|(_, r)| r.split_once(':')) else {
                    continue;
                };
                let usage = "expected name=Header:env:VAR or name=Header:file:PATH";
                secret(source, usage, "the header's value", &[], &[], false).err()
            }
            _ => None,
        };
        if let Some(e) = e {
            let label = match flag {
                "--hmac" | "--secret" => spec.split_once('=').map_or("", |(n, _)| n),
                _ => header_label(spec),
            };
            return Some(flag_error(flag, label, &e));
        }
    }
    None
}

/// The Resources section: per test on both sides, fuel and peak memory old -> new when either
/// changed by more than 2%; then over every such test the total fuel and the largest peak. A test
/// that failed on either side stopped early: it is left out, and counted.
fn resources(old: &[TestResult], new: &[TestResult], page: &mut Page) {
    // Every test that failed in NEW, and those on both sides that failed in OLD only.
    let failed = new.iter().filter(|n| n.error.is_some()).count();
    let mut failed_old = 0;
    let pairs: Vec<(&str, Used, Used)> = (old.iter())
        .filter_map(|o| {
            let n = new.iter().find(|n| n.name == o.name)?;
            if n.error.is_some() {
                return None;
            }
            if o.error.is_some() {
                failed_old += 1;
                return None;
            }
            Some((o.name.as_str(), o.used?, n.used?))
        })
        .collect();
    let tests = |n: usize| if n == 1 { "test" } else { "tests" };
    if failed > 0 {
        page.resources.push(format!(
            "{failed} {} failed in new; left out of Resources",
            tests(failed)
        ));
    }
    if failed_old > 0 {
        let n = failed_old;
        page.resources
            .push(format!("{n} {} failed in old only; left out of Resources", tests(n)));
    }
    let changed = |o: u64, n: u64| o.abs_diff(n) * RESOURCE_SHARE > o;
    let row = |label: &str, peak: &str, o: &Used, n: &Used| {
        let (om, nm) = (o.peak_memory as u64, n.peak_memory as u64);
        format!(
            "{label}: fuel {} -> {} ({}), {peak} {} KB -> {} KB ({})",
            o.fuel,
            n.fuel,
            percent(o.fuel, n.fuel),
            om >> 10,
            nm >> 10,
            percent(om, nm)
        )
    };
    let as_json = |name: &str, o: &Used, n: &Used| json!({"name": name, "fuel": [o.fuel, n.fuel], "peak_memory": [o.peak_memory, n.peak_memory]});
    let mut rows = Vec::new();
    for (name, o, n) in &pairs {
        if changed(o.fuel, n.fuel) || changed(o.peak_memory as u64, n.peak_memory as u64) {
            page.resources.push(row(name, "peak memory", o, n));
            rows.push(as_json(name, o, n));
        }
    }
    let total = |pick: fn(&(&str, Used, Used)) -> Used| {
        pairs
            .iter()
            .map(pick)
            .fold((0, 0), |(f, m), u| (f + u.fuel, m.max(u.peak_memory)))
    };
    let ((of, om), (nf, nm)) = (total(|p| p.1), total(|p| p.2));
    let sum = |fuel, peak_memory| Used {
        fuel,
        calls: 0,
        io: 0,
        peak_memory,
        ms: 0.0,
    };
    let (o, n) = (sum(of, om), sum(nf, nm));
    // Each test only in NEW that passed (R1a5): its own numbers, no old to compare.
    let added: Vec<(&str, Used)> = (new.iter())
        .filter(|n| n.error.is_none() && !old.iter().any(|o| o.name == n.name))
        .filter_map(|n| Some((n.name.as_str(), n.used?)))
        .collect();
    let added_lines = added
        .iter()
        .map(|(name, u)| format!("added: {name}: fuel {}, peak memory {} KB", u.fuel, u.peak_memory >> 10));
    let added: Vec<_> = (added.iter())
        .map(|(name, u)| json!({"name": name, "fuel": u.fuel, "peak_memory": u.peak_memory}))
        .collect();
    if pairs.is_empty() {
        page.resources.extend(added_lines);
        let ran = if failed + failed_old == 0 { "ran" } else { "passed" };
        page.resources.push(format!("no test {ran} on both sides"));
        page.resources_json = json!({"rows": rows, "added": added, "total": null, "tests": 0, "failed": failed, "failed_old": failed_old});
        return;
    }
    if rows.is_empty() {
        page.resources.push("no test changed by more than 2%".into());
    }
    page.resources.extend(added_lines);
    let label = format!("total over {} tests on both sides", pairs.len());
    page.resources.push(row(&label, "largest peak memory", &o, &n));
    page.resources_json = json!({"rows": rows, "added": added, "total": as_json("total", &o, &n), "tests": pairs.len(), "failed": failed, "failed_old": failed_old});
}

/// The change from `old` to `new` in percent, two decimals, rounded away from zero so a change
/// over 2% never reads `2.00%`: `+12.50%`.
fn percent(old: u64, new: u64) -> String {
    if old == 0 {
        return if new == 0 { "+0.00%".into() } else { "new".into() };
    }
    let (d, o) = (old.abs_diff(new) as u128, old as u128);
    let bp = (d * 10_000).div_ceil(o);
    let sign = if new < old { '-' } else { '+' };
    format!("{sign}{}.{:02}%", bp / 100, bp % 100)
}

/// The Not checked section: for each capability method NEW's `main` reaches, what the host does
/// not look at; each dependency added, removed or changed (PK1); then the line on who wrote the
/// tests.
fn not_checked(inputs: &Inputs, page: &mut Page) {
    let mut rows = Vec::new();
    let none = no_program();
    let entries =
        (inputs.entries.iter()).map(|e| (Some(e.name.as_str()), e.old.as_ref().map_or(&none, |o| &o.1), &e.new.1));
    for (entry, old, prog) in [(None, &inputs.old.1, &inputs.new.1)].into_iter().chain(entries) {
        let Ok(p) = kio_host::check_program(&prog.files) else {
            continue;
        };
        let changes = Changes::of(old, prog);
        let prefix = entry.map_or(String::new(), |e| format!("{e}: "));
        let a = lang::authority(&p);
        for (param, methods) in a.funcs.get("main").into_iter().flatten() {
            let Some((_, ty)) = a.main.iter().find(|(n, _)| n == param) else {
                continue;
            };
            let Some((_, of, what)) = BLIND_SPOTS.iter().find(|(t, _, _)| t == ty) else {
                continue;
            };
            let reached: Vec<&str> = (methods.iter().map(String::as_str))
                .filter(|m| of.is_empty() || of.contains(&m.split(' ').next().unwrap_or_default()))
                .collect();
            if reached.is_empty() {
                continue;
            }
            // The functions that use these methods (`main` only when no other does): the changed
            // functions they reach, themselves included.
            let mut users = lang::users(&p, param, &reached);
            if users.len() > 1 {
                users.retain(|f| f != "main");
            }
            let users = changes.reached(&users.iter().map(String::as_str).collect::<Vec<_>>());
            let mark = match users.is_empty() {
                true => String::new(),
                false => format!(" (changed: {})", users.join(", ")),
            };
            page.not_checked
                .push(format!("{prefix}{param}: {ty} {}: {what}{mark}", reached.join(", ")));
            rows.push(
                json!({"entry": entry, "grant": param, "type": ty, "methods": reached, "what": what, "changed": users}),
            );
        }
    }
    // Per program, each its own `kio.deps`: two programs may pin one name to different files. A
    // change two programs share is shown once, for the first.
    let mut dep_rows = Vec::new();
    let mut seen = Vec::new();
    let programs = [(None, &inputs.old.1.deps, &inputs.new.1.deps)]
        .into_iter()
        .chain((inputs.entries.iter()).map(|e| {
            (
                Some(e.name.as_str()),
                e.old.as_ref().map_or(&none.deps, |o| &o.1.deps),
                &e.new.1.deps,
            )
        }));
    for (entry, old_deps, new_deps) in programs {
        for change in deps::changes(old_deps, new_deps) {
            if seen.contains(&change) {
                continue;
            }
            let (name, o, n) = &change;
            // Code added or changed that the page does not show: exit 3, as authority added (#280).
            page.added |= n.is_some();
            let what = match (o, n) {
                (Some(o), Some(n)) => format!("changed ({} -> {})", deps::short(o), deps::short(n)),
                (Some(o), None) => format!("removed ({})", deps::short(o)),
                (_, n) => format!("added ({})", deps::short(n.as_deref().unwrap_or_default())),
            };
            let prefix = entry.map_or(String::new(), |e| format!("{e}: "));
            page.not_checked.push(format!(
                "{prefix}dependency {name} {what}: its code is not shown; its authority is above"
            ));
            dep_rows.push(json!({"entry": entry, "name": name, "old": o, "new": n}));
            seen.push(change);
        }
    }
    for (n, f) in &inputs.fakes {
        let (n, f) = (kio_host::escape_line(n), kio_host::escape_line(f));
        page.not_checked.push(format!(
            "{n}: Http: tests ran against {f}, not a server; the real endpoint's answers are untested"
        ));
    }
    page.not_checked.push(SAME_AUTHOR.into());
    page.not_checked_json =
        json!({"methods": rows, "deps": dep_rows, "fakes": fakes_json(inputs), "author": SAME_AUTHOR});
}

/// Each fake of the test grants file, for `--json`.
fn fakes_json(inputs: &Inputs) -> serde_json::Value {
    json!(inputs
        .fakes
        .iter()
        .map(|(n, f)| json!({"grant": n, "path": f}))
        .collect::<Vec<_>>())
}

/// Each program one side loads: its root, then each `--entry` it has.
fn side_programs(inputs: &Inputs, new: bool) -> Vec<&Program> {
    let root = if new { &inputs.new.1 } else { &inputs.old.1 };
    let entries = (inputs.entries.iter()).filter_map(|e| match new {
        true => Some(&e.new.1),
        false => e.old.as_ref().map(|o| &o.1),
    });
    [root].into_iter().chain(entries).collect()
}

/// The dependencies one side's programs load, each once.
fn side_deps(inputs: &Inputs, new: bool) -> Vec<kio_host::Dep> {
    let mut deps: Vec<kio_host::Dep> = Vec::new();
    for d in side_programs(inputs, new).into_iter().flat_map(|p| &p.deps) {
        if !deps.iter().any(|x| x.name == d.name && x.path == d.path) {
            deps.push(d.clone());
        }
    }
    deps
}

/// The Identity section: kio's version and commit, the sha256 of each file read, and the seed.
/// Each file is read again: one whose contents differ from what was hashed goes in `changed`.
fn identity(inputs: &Inputs, page: &mut Page) -> serde_json::Value {
    let hash = |b: &[u8]| lang::digest::to_hex(&lang::digest::sha256(b));
    // Each file shown and its hash (`None`: absent).
    let mut shown: Vec<(String, Option<String>)> = Vec::new();
    let (version, commit) = (env!("CARGO_PKG_VERSION"), env!("KIO_COMMIT"));
    page.identity.push(format!("kio {version} ({commit})"));
    let mut sides = json!({});
    let mut deps = json!({});
    for (side, new) in [("old", false), ("new", true)] {
        // Every file of the root and each entry; one they share (a module) once.
        let mut each: Vec<(String, String)> = (side_programs(inputs, new).into_iter())
            .flat_map(|prog| (0..prog.files.len()).map(move |i| (json_path(prog, i), hash(prog.files[i].1.as_bytes()))))
            .collect();
        each.sort();
        each.dedup();
        shown.extend(each.iter().map(|(p, h)| (p.clone(), Some(h.clone()))));
        (page.identity).extend(
            each.iter()
                .map(|(p, h)| format!("{side} {} sha256:{h}", kio_host::escape_line(p))),
        );
        sides[side] = json!(each
            .iter()
            .map(|(p, h)| json!({"path": p, "sha256": h}))
            .collect::<Vec<_>>());
        let side_deps = side_deps(inputs, new);
        for d in &side_deps {
            let path = kio_host::escape_line(&d.path);
            page.identity
                .push(format!("{side} dep {} {path} sha256:{}", d.name, d.sha256));
        }
        let rows: Vec<_> = (side_deps.iter())
            .map(|d| json!({"name": d.name, "path": d.path, "sha256": d.sha256}))
            .collect();
        deps[side] = json!(rows);
    }
    let mut file = |what: &str, f: &Option<(String, Vec<u8>)>| {
        f.as_ref().map(|(g, bytes)| {
            let path = kio_host::escape_line(g);
            page.identity.push(format!("{what} {path} sha256:{}", hash(bytes)));
            shown.push((g.clone(), Some(hash(bytes))));
            json!({"path": g, "sha256": hash(bytes)})
        })
    };
    let old_of = |b: &Beside| b.as_ref().ok().and_then(|(_, o)| o.file());
    let grants = file("grants", &inputs.grants);
    let old_grants = file("old grants", &old_of(&inputs.old_grants));
    let test_grants = file("test grants", &inputs.test_grants);
    // A FILE that could not be read was not: its error is the tests' on both sides.
    let test_fakes: Vec<_> = (inputs.fake_files.iter())
        .filter_map(|(p, r)| file("test fake", &r.as_ref().ok().map(|b| (p.clone(), b.clone()))))
        .collect();
    let entry_grants: Vec<_> = (inputs.entries.iter())
        .filter_map(|e| file(&format!("{}: entry grants", e.name), &e.grants))
        .collect();
    let old_entry_grants: Vec<_> = (inputs.entries.iter())
        .filter_map(|e| file(&format!("{}: old entry grants", e.name), &old_of(&e.old_grants)))
        .collect();
    let mut files = Vec::new();
    let requirements = inputs.requirements.as_ref().map(|sides| {
        let rows: Vec<_> = (sides.iter().zip(["old", "new"]))
            .map(|((path, text), side)| {
                let h = text.as_ref().map(|t| hash(t.as_bytes()));
                let shown = h.as_ref().map_or("absent".into(), |h| format!("sha256:{h}"));
                page.identity.push(format!("{side} requirements {path} {shown}"));
                files.push((path.clone(), h.clone()));
                json!({"path": path, "sha256": h})
            })
            .collect();
        rows
    });
    page.identity.push(format!("test seed {DEFAULT_SEED}"));
    shown.extend(files);
    page.changed = changed(&shown);
    json!({
        "kio": version, "commit": commit, "old": sides["old"], "new": sides["new"], "deps": deps,
        "grants": grants, "test_grants": test_grants, "test_fakes": test_fakes, "entry_grants": entry_grants, "old_grants": old_grants, "old_entry_grants": old_entry_grants, "requirements": requirements, "seed": DEFAULT_SEED,
        "changed": page.changed,
    })
}

/// The most bytes of a file read again for `changed`: one longer, or not a regular file now
/// (a FIFO would block), has changed.
const CHANGED_READ_MAX: u64 = 64 << 20;

/// Each of `files` (a path and its sha256, `None` for absent) whose contents on disk differ now.
fn changed(files: &[(String, Option<String>)]) -> Vec<String> {
    let hash = |b: &[u8]| lang::digest::to_hex(&lang::digest::sha256(b));
    let mut out: Vec<String> = Vec::new();
    for (path, h) in files {
        if read_regular(path.as_ref(), CHANGED_READ_MAX, false)
            .ok()
            .map(|b| hash(&b))
            != *h
            && !out.contains(path)
        {
            out.push(path.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{changed, percent, unified, BLIND_SPOTS, NO_BLIND_SPOT};

    #[test]
    fn a_file_changed_after_it_was_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.kio"), dir.path().join("b.md"));
        std::fs::write(&a, "fn main() {}\n").unwrap();
        let hash = |t: &str| Some(lang::digest::to_hex(&lang::digest::sha256(t.as_bytes())));
        let a = a.display().to_string();
        let b = b.display().to_string();
        let files = [(a.clone(), hash("fn main() {}\n")), (b.clone(), None)];
        assert_eq!(changed(&files), Vec::<String>::new());
        std::fs::write(&a, "fn main() { }\n").unwrap();
        std::fs::write(&b, "now here\n").unwrap();
        assert_eq!(changed(&files), [a, b]);
    }

    /// A new caps.wit method fails here until it is given a blind spot or none.
    #[test]
    fn every_capability_method_is_classified() {
        for cap in lang::CapTable::v0().caps {
            for (m, _, _) in &cap.methods {
                let blind = BLIND_SPOTS
                    .iter()
                    .any(|(t, ms, _)| *t == cap.name && (ms.is_empty() || ms.contains(&m.as_str())));
                let none = NO_BLIND_SPOT
                    .iter()
                    .any(|(t, ms)| *t == cap.name && ms.contains(&m.as_str()));
                assert!(
                    blind != none,
                    "{}.{m}: in BLIND_SPOTS {blind}, in NO_BLIND_SPOT {none}",
                    cap.name
                );
            }
        }
    }

    #[test]
    fn percent_rounds_away_from_zero() {
        assert_eq!(percent(168702, 172079), "+2.01%");
        assert_eq!(percent(100, 102), "+2.00%");
        assert_eq!(percent(100, 50), "-50.00%");
        assert_eq!(percent(7, 7), "+0.00%");
        assert_eq!(percent(0, 3), "new");
    }

    #[test]
    fn unified_diff_ranges() {
        assert_eq!(unified("a\n", "a\n"), Ok(vec![]));
        assert_eq!(unified("", "a\n").unwrap(), ["@@ -0,0 +1 @@", "+a"]);
        assert_eq!(unified("a\nb\n", "b\n").unwrap(), ["@@ -1,2 +1 @@", "-a", " b"]);
        assert_eq!(
            unified("a\nb\nc\n", "a\nc\n").unwrap(),
            ["@@ -1,3 +1,2 @@", " a", "-b", " c"]
        );
        let err = unified(&"x\n".repeat(3000), &"y\n".repeat(3000)).unwrap_err();
        assert!(err.starts_with("3000 and 3000 changed lines are too many"), "{err}");
    }
}
