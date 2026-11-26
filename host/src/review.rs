//! `kio review NEW --base OLD`: one page from the evidence `kio check` and `kio test` produce
//! (R1a). Every line carries its label; nothing on the page is a verdict.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod mutants;

const USAGE: &str =
    "usage: kio review <new.kio> --base <old.kio> [--grants file [grant flags]] [--test-grants file] [--requirements file] [--entry file [--entry-grants file [grant flags]] [--entry-test-grants file]].. [--seed s] [--mutants n] [--mutations file] [--json]";

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
        &["exec", "query", "transaction", "exec_rows", "transaction_rows"],
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
    ("Clock", &["now_ms", "sleep_ms"]),
    ("Hmac", &["verify"]),
    ("Secret", &["matches"]),
    ("Value", &["get"]),
];

const SAME_AUTHOR: &str = "tests and properties were written by the same author as the code unless stated";

/// What the command line names, read: each side's files and path, the grants file for NEW's
/// `main`, the tests' grants file, and the requirements file on each side (`None` when absent).
struct Inputs {
    old: (String, Program),
    /// OLD's file does not exist but its directory does: `old` is that directory's modules
    /// under an empty `main` (`read_old`, R1d1).
    old_absent: bool,
    new: (String, Program),
    grants: Option<(String, Vec<u8>)>,
    /// The grant flags given after `--grants` (R1d4): bound with it, the same on both sides.
    extra: Vec<String>,
    /// The grants file beside OLD's program at G's path relative to NEW's (R1a4).
    old_grants: Beside,
    test: TestGrants,
    /// `--requirements`' path as given, and the file on each side.
    requirements: Option<Requirements>,
    entries: Vec<Entry>,
    /// The tests' seed (`--seed`), which also draws the mutants.
    seed: u64,
    /// How many generated mutants to run (`--mutants`).
    mutants: usize,
    /// The mutations file (`--mutations`), read once and parsed (R1b2).
    mutations: Option<mutants::File>,
}

/// `--requirements`' path as given, and on each side the file's path and text (`None`: absent).
type Requirements = (String, [(String, Option<String>); 2]);

/// A grants file of NEW's and the grant flags given after it, and what is at its place beside
/// OLD's program.
type GrantsGiven<'a> = (Option<&'a (String, Vec<u8>)>, &'a [String], &'a Beside);

/// Another entry of the change (`--entry`): the path as given, escaped; OLD's program (`None`:
/// neither the file nor its directory is there) and NEW's, each with its path; its
/// `--entry-grants` file and the grant flags after it; its `--entry-test-grants`.
struct Entry {
    name: String,
    old: Option<(String, Program)>,
    /// OLD has no file here: the entry is new (`old` is its directory's modules, `read_old`).
    old_absent: bool,
    /// Where OLD's entry is or would be.
    old_path: String,
    new: (String, Program),
    grants: Option<(String, Vec<u8>)>,
    extra: Vec<String>,
    old_grants: Beside,
    /// `None`: the entry's tests run under the root's (`--test-grants`, R1d3).
    test: Option<TestGrants>,
}

/// A tests' grants file (`--test-grants`, `--entry-test-grants`) and its fakes.
#[derive(Default)]
struct TestGrants {
    file: Option<(String, Vec<u8>)>,
    /// Each `--http name=fake:FILE` of the file, as `(name, FILE)`.
    fakes: Vec<(String, String)>,
    /// Each FILE of `fakes` read once, for both sides' tests and Identity.
    fake_files: Vec<(String, Result<Vec<u8>, String>)>,
    /// Each FILE of `fake_files` that `kio test`'s rules accept (`fake_file`): only these are hashed.
    fakes_ok: Vec<String>,
    /// Each FILE of `fakes_ok` and what is at its place beside OLD's program (R1d7).
    old_fakes: Vec<(String, Beside)>,
}

impl TestGrants {
    /// The flags that apply the file, none for the defaults.
    fn flags(&self) -> Vec<&str> {
        self.file.as_ref().map_or(vec![], |(t, _)| vec!["--grants", t.as_str()])
    }

    /// The file as the Tests section names it.
    fn shown(&self) -> &str {
        self.file.as_ref().map_or(DEFAULT_TEST_GRANTS, |(t, _)| t.as_str())
    }
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

/// `kio review`: exit 1 if a side or entry does not check, a grants file does not bind, a NEW test
/// fails, the mutant runs do not finish or an entry of the mutations file cannot run; else 3 if
/// authority was added, a dependency added or changed, a test removed, or a mutant of the
/// mutations file survived; else 0. 2 for usage.
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
        // A fake's name as its Identity line shows it.
        let mutations = inputs.mutations.as_ref().is_some_and(|m| m.path == *p);
        let fake = test_grants(&inputs).any(|(_, t)| t.fakes_ok.contains(p));
        let p = match fake || mutations {
            true => ascii(p),
            false => kio_host::escape_line(p),
        };
        println!("  {:<12} {} changed while the review ran", "error", p);
    }
    code
}

/// Each tests' grants file with how the page prefixes its lines: the root's, then each entry's
/// own (`--entry-test-grants`).
fn test_grants(inputs: &Inputs) -> impl Iterator<Item = (String, &TestGrants)> {
    let entries = (inputs.entries.iter()).filter_map(|e| Some((format!("{}: ", e.name), e.test.as_ref()?)));
    std::iter::once((String::new(), &inputs.test)).chain(entries)
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
    let (mut seed, mut mutants, mut mutations) = (None, None, None);
    let mut extra: Vec<String> = Vec::new();
    let mut entries: Vec<Given> = Vec::new();
    // Where a grant flag goes while grant flags follow `--grants` (false) or an
    // `--entry-grants` (true).
    let mut grant_flags: Option<bool> = None;
    if !flags.len().is_multiple_of(2) {
        return Err(fail(USAGE.into()));
    }
    for pair in flags.chunks(2) {
        let after = grant_flags.take();
        if RUN_FLAGS.contains(&pair[0]) {
            let to = match (after, entries.last_mut()) {
                (Some(false), _) => &mut extra,
                (Some(true), Some(e)) => &mut e.extra,
                _ => {
                    return Err(fail(format!(
                        "`{} {}` is a grant: give it right after the grants file it adds to\n  hint: `--grants app.grants {} ..`, or after an `--entry-grants FILE`",
                        pair[0],
                        kio_host::escape_line(pair[1]),
                        pair[0]
                    )))
                }
            };
            to.extend([pair[0].to_string(), pair[1].to_string()]);
            grant_flags = after;
            continue;
        }
        match (pair[0], entries.last_mut()) {
            ("--entry", _) => {
                entries.push(Given {
                    path: pair[1],
                    grants: None,
                    extra: Vec::new(),
                    test: None,
                });
                continue;
            }
            ("--entry-grants", Some(e)) if e.grants.is_none() => {
                e.grants = Some(pair[1]);
                grant_flags = Some(true);
                continue;
            }
            ("--entry-test-grants", Some(e)) if e.test.is_none() => {
                e.test = Some(pair[1]);
                continue;
            }
            (flag @ ("--entry-grants" | "--entry-test-grants"), _) => {
                return Err(fail(format!(
                "`{flag}` binds the `--entry` just before it, once\n  hint: `--entry worker.kio {flag} worker.grants`"
            )))
            }
            ("--grants", _) => grant_flags = Some(false),
            _ => {}
        }
        let slot = match pair[0] {
            "--base" => &mut base,
            "--grants" => &mut grants,
            "--test-grants" => &mut test_grants,
            "--requirements" => &mut requirements,
            "--seed" => &mut seed,
            "--mutants" => &mut mutants,
            "--mutations" => &mut mutations,
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
    let seed = match seed.map(str::parse::<u64>) {
        None => DEFAULT_SEED,
        Some(Ok(s)) => s,
        Some(Err(_)) => {
            return Err(fail(format!(
                "--seed {}: expected a non-negative integer\n  hint: `--seed 1`, the default",
                kio_host::escape_line(seed.unwrap_or_default())
            )))
        }
    };
    let max = mutants::MAX;
    let mutants = match mutants.map(str::parse::<usize>) {
        None => mutants::DEFAULT,
        Some(Ok(n)) if n <= max => n,
        Some(_) => {
            return Err(fail(format!(
                "--mutants {}: expected 0 to {max}\n  hint: `--mutants 0` runs none",
                kio_host::escape_line(mutants.unwrap_or_default())
            )))
        }
    };
    let new = (path.to_string(), read_program(path)?);
    let (old_prog, old_absent) = read_old(base, &new.1)?;
    let old = (base.to_string(), old_prog);
    let mutations = mutations.map(|m| mutants::read(m, path)).transpose()?;
    let read = |flag: &str, g: Option<&str>| g.map(|g| read_grants(flag, g)).transpose();
    let grants = read("--grants", grants)?;
    let old_grants = match &grants {
        Some((g, _)) => beside_old(base, path, g),
        None => Err(()),
    };
    let requirements = match requirements {
        Some(file) if Path::new(file).is_absolute() => {
            return Err(fail(format!(
                "--requirements {file}: an absolute path names one file for both sides\n  hint: give a path relative to each program's directory, e.g. `--requirements README.md`"
            )))
        }
        Some(file) => Some((
            file.to_string(),
            [read_requirements(base, file)?, read_requirements(path, file)?],
        )),
        None => None,
    };
    let mut read_entries: Vec<Entry> = Vec::new();
    for g in &entries {
        let mut e = entry(base, path, g.path, read("--entry-grants", g.grants)?)?;
        e.extra = g.extra.clone();
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
    // No tests' grants file may hold a NEW program's source.
    let progs = std::iter::once(&new.1).chain(read_entries.iter().map(|e| &e.new.1));
    let sources: Vec<_> = progs.flat_map(|p| p.paths.iter()).collect();
    let test = read_test_grants("--test-grants", test_grants, (base, path), &sources)?;
    let mut tests = Vec::new();
    for (e, g) in read_entries.iter().zip(&entries) {
        let sides = (e.old_path.as_str(), e.new.0.as_str());
        tests.push(match g.test {
            Some(_) => Some(read_test_grants("--entry-test-grants", g.test, sides, &sources)?),
            None => None,
        });
    }
    for (e, t) in read_entries.iter_mut().zip(tests) {
        e.test = t;
    }
    Ok(Inputs {
        old,
        old_absent,
        new,
        grants,
        extra,
        old_grants,
        test,
        requirements,
        entries: read_entries,
        seed,
        mutants,
        mutations,
    })
}

/// An `--entry` as the command line gives it: its path, `--entry-grants` and the grant flags
/// after it, and `--entry-test-grants`.
struct Given<'a> {
    path: &'a str,
    grants: Option<&'a str>,
    extra: Vec<String>,
    test: Option<&'a str>,
}

/// The grants file `g` of `flag`, read.
fn read_grants(flag: &str, g: &str) -> Result<(String, Vec<u8>), i32> {
    match read_regular(g.as_ref(), GRANTS_MAX, false) {
        Ok(bytes) => Ok((g.to_string(), bytes)),
        Err(Unread::TooLarge) => Err(fail(format!(
            "{flag} {}: over 1 MiB\n  hint: a grants file is one flag a line",
            kio_host::escape_line(g)
        ))),
        Err(e) => Err(fail(format!("{flag} {}: {e}", kio_host::escape_line(g)))),
    }
}

/// The tests' grants file `t` of `flag`, for the program at `new` (OLD's at `old`), read and
/// checked: no state carried from OLD's tests to NEW's, fakes inside NEW's directory, no `--dir`
/// over one of NEW's `sources`.
fn read_test_grants(
    flag: &str,
    t: Option<&str>,
    (old, new): (&str, &str),
    sources: &[&std::path::PathBuf],
) -> Result<TestGrants, i32> {
    let Some(t) = t else {
        return Ok(TestGrants::default());
    };
    let (t, bytes) = read_grants(flag, t)?;
    shared_state(flag, &t)?;
    let (fakes, fake_files, fakes_ok) = test_fakes(flag, &t, &bytes, new)?;
    source_dirs(flag, &t, sources)?;
    let old_fakes = (fakes_ok.iter())
        .map(|f| (f.clone(), beside_old(old, new, f)))
        .collect();
    Ok(TestGrants {
        file: Some((t, bytes)),
        fakes,
        fake_files,
        fakes_ok,
        old_fakes,
    })
}

/// OLD's program at `path`, and `false`; when no file is there but its directory is, the entry
/// is new (R1d1): that directory's modules that NEW's root `new` names (beside it or in its
/// `kio.deps`) under a root with an empty `main`, and `true`. So a module function is compared
/// with its old text by its qualified name, and the old modules' tests run as the base's.
fn read_old(path: &str, new: &Program) -> Result<(Program, bool), i32> {
    let dir = Path::new(path).parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or(".".as_ref());
    let missing = matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
    if !missing || !dir.is_dir() {
        return Ok((read_program(path)?, false));
    }
    let (listed, _) = deps::read(dir)?;
    let mods: String = (mod_lines(&new.files[0].1).into_iter())
        .filter(|(m, _)| listed.iter().any(|l| l.name == *m) || dir.join(format!("{m}.kio")).is_file())
        .map(|(m, _)| format!("mod {m};\n"))
        .collect();
    // The root is no file: it stands at its directory, whose identity each test run checks.
    let id = kio_host::path_id(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let mut prog = with_modules(path, format!("{mods}fn main() {{}}\n"), id)?;
    prog.paths[0] = dir.to_path_buf();
    Ok((prog, true))
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
    let new_path = new_path.display().to_string();
    let new = (new_path.clone(), read_program(&new_path)?);
    // Neither the file nor its directory on OLD's side: no OLD program at all.
    let gone = std::fs::symlink_metadata(&old_path).is_err()
        && old_path
            .parent()
            .is_some_and(|d| !d.as_os_str().is_empty() && !d.is_dir());
    let old_path = old_path.display().to_string();
    let (old, old_absent) = match gone {
        true => (None, true),
        false => {
            let (prog, absent) = read_old(&old_path, &new.1)?;
            (Some((old_path.clone(), prog)), absent)
        }
    };
    Ok(Entry {
        name,
        old,
        old_absent,
        old_path,
        new,
        grants,
        extra: Vec::new(),
        old_grants: Err(()),
        test: None,
    })
}

/// Refuses a tests' grants file that gives a writable `--dir` or a file-backed `--kv` or `--db`:
/// OLD's tests run first and would leave their writes for NEW's.
fn shared_state(flag: &str, t: &str) -> Result<(), i32> {
    let (flags, _) = with_grants(&["--grants", t])?;
    for pair in flags.chunks(2) {
        let Some((name, target)) = pair[1].split_once('=') else {
            continue;
        };
        if ["--dir", "--kv", "--db"].contains(&pair[0].as_str()) && writable_target(target) {
            return Err(fail(format!(
                "{flag} {t}: `{} {name}={target}` would carry OLD's test writes into NEW's\n  hint: grant it `:ro`, use `:memory:` for a store, or leave it out for a fresh one per test",
                pair[0]
            )));
        }
    }
    Ok(())
}

/// Refuses a tests' grants file whose `--dir`, `:ro` or not, holds one of NEW's source files
/// (`sources`: its programs' files and `kio.deps`): a test could read the code under review
/// instead of running it (R1b3). Inside is decided by identity, as H28 decides it, so a symlink
/// to the directory or a symlinked directory on the way to a file counts. A source file with
/// other hard links is refused while any `--dir` is granted: one could be inside it.
fn source_dirs(flag: &str, t: &str, sources: &[&std::path::PathBuf]) -> Result<(), i32> {
    let (flags, _) = with_grants(&["--grants", t])?;
    let at = format!("{flag} {}", kio_host::escape_line(t));
    for pair in flags.chunks(2).filter(|p| p[0] == "--dir") {
        let Some((name, spec)) = pair[1].split_once('=') else {
            continue;
        };
        let grant = kio_host::escape_line(&format!("--dir {name}={spec}"));
        let (target, _) = kio_host::kv::split_spec(spec);
        let Ok(id) = kio_host::path_id(target.as_ref()) else {
            continue;
        };
        for file in sources {
            let shown = kio_host::escape_line(&file.display().to_string());
            if kio_host::within(file, id) {
                return Err(fail(format!(
                    "{at}: `{grant}` holds NEW's source `{shown}`; a test could read the code under review instead of running it\n  hint: grant a directory that holds no program file, e.g. `--dir data=testdata:ro` beside the program"
                )));
            }
            if std::fs::metadata(file).is_ok_and(|m| std::os::unix::fs::MetadataExt::nlink(&m) > 1) {
                return Err(fail(format!(
                    "{at}: NEW's source `{shown}` has other hard links; one could be inside `{grant}`\n  hint: give the source file one link (copy it), or leave the `--dir` out"
                )));
            }
        }
    }
    Ok(())
}

/// A test grants file's fakes: each `--http name=fake:FILE` as `(name, FILE)`, each FILE read
/// once (its bytes or why not), and the FILEs `kio test`'s rules accept.
type Fakes = (
    Vec<(String, String)>,
    Vec<(String, Result<Vec<u8>, String>)>,
    Vec<String>,
);

/// The fakes of the test grants file `t`, parsed from the `bytes` read and hashed. A grant name
/// that is not an identifier, or a FILE outside NEW's directory (the change does not show it),
/// is refused.
fn test_fakes(flag: &str, t: &str, bytes: &[u8], new: &str) -> Result<Fakes, i32> {
    let at = format!("{flag} {}", kio_host::escape_line(t));
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Err(fail(format!("{at}: not UTF-8")));
    };
    let flags = grants_flags(text, t, &[])?;
    http_names(&flags).map_err(|e| fail(format!("{at}: {e}")))?;
    let fake = |p: &[String]| match p[1].split_once('=') {
        Some((name, o)) if p[0] == "--http" => Some((name.to_string(), o.strip_prefix("fake:")?.to_string())),
        _ => None,
    };
    let fakes: Vec<(String, String)> = flags.chunks(2).filter_map(fake).collect();
    // As `--grants G` is compared only inside NEW's directory: canonical, so no symlink leads out.
    let dir = Path::new(new).parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or(".".as_ref()).canonicalize();
    let (mut files, mut ok) = (Vec::new(), Vec::<String>::new());
    for (name, f) in &fakes {
        if let (Ok(d), Ok(c)) = (&dir, Path::new(f).canonicalize()) {
            if !c.starts_with(d) {
                return Err(fail(format!(
                    "{at}: --http {name}=fake:{}: outside {}'s directory, so the change does not show it\n  hint: keep the fake file beside the program it tests",
                    kio_host::escape_line(f),
                    kio_host::escape_line(new)
                )));
            }
        }
        if !files.iter().any(|(p, _)| p == f) {
            files.push((f.clone(), read_fake(f)));
        }
        if fake_file(name, f, &flags, &files).is_ok() && !ok.contains(f) {
            ok.push(f.clone());
        }
    }
    Ok((fakes, files, ok))
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
/// The label is on the first line only, so the diff below it reads as a diff. `file` is the path
/// as given; on neither side, one line says so (R1d5).
fn requirements((file, sides): &Requirements) -> (Vec<Row>, serde_json::Value) {
    let [(old_path, old), (new_path, new)] = sides;
    if old.is_none() && new.is_none() {
        let line = format!("no requirements file ({} on neither side)", kio_host::escape_line(file));
        let v = json!({"old": old_path, "new": new_path, "diff": null, "error": null, "neither": true});
        return (vec![("not checked", line)], v);
    }
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

/// A new entry's base `old` in NEW's own directory (`new`'s) is compared with NEW's modules:
/// said, as a line, after `prefix` (F6, #372).
fn shares_dir(old: &str, new: &str, prefix: &str, page: &mut Page) {
    let dir = |p: &str| {
        let d = Path::new(p).parent().filter(|d| !d.as_os_str().is_empty());
        d.unwrap_or(".".as_ref()).canonicalize().ok()
    };
    if dir(old).is_some_and(|d| Some(d) == dir(new)) {
        let base = kio_host::escape_line(old);
        let line = format!("{prefix}new entry: {base} shares NEW's directory; its modules are NEW's own");
        page.authority.push(("not checked", line));
    }
}

/// The Authority section: the root's lines, then each `--entry`'s after its name. Whether the
/// root's two sides check (its tests run), and whether every entry checks and every grants file
/// binds.
fn authority(inputs: &Inputs, page: &mut Page) -> (bool, bool) {
    if inputs.old_absent {
        let line = format!("new entry: {} has no file", kio_host::escape_line(&inputs.old.0));
        page.authority.push(("not checked", line));
        shares_dir(&inputs.old.0, &inputs.new.0, "", page);
    }
    let grants = (inputs.grants.as_ref(), &inputs.extra[..], &inputs.old_grants);
    let (mut value, checks, binds) = entry_authority(&inputs.old.1, &inputs.new, grants, "", "--grants", page);
    value["new_entry"] = json!(inputs.old_absent);
    let mut ok = binds;
    let mut entries = Vec::new();
    let none = no_program();
    for e in &inputs.entries {
        let prefix = format!("{}: ", e.name);
        if let (true, Some((o, _))) = (e.old_absent, &e.old) {
            shares_dir(o, &e.new.0, &prefix, page);
        }
        let old = e.old.as_ref().map_or(&none, |o| &o.1);
        let grants = (e.grants.as_ref(), &e.extra[..], &e.old_grants);
        let (mut v, checks, binds) = entry_authority(old, &e.new, grants, &prefix, "--entry-grants", page);
        ok &= checks && binds;
        v["entry"] = json!(e.name);
        v["new_entry"] = json!(e.old_absent);
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
    (grants, extra, old_grants): GrantsGiven,
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
            // A `main` grant renamed moves every binding it reaches: one line, not one per leaf.
            if let Some(l) = diff.renamed() {
                push("derived", &l);
            }
            // Exit 3 with no grant added: name the functions whose lines add.
            let names: Vec<&str> = (diff.funcs.iter())
                .filter_map(|l| Some(l.strip_prefix("+ ")?.split_once(": ")?.0))
                .collect();
            if funcs && diff.added && !diff.grant_added && !names.is_empty() {
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
            // Binding lines (L32): the grants each leaf is bound to, where that changed.
            if let Some(h) = diff.binding_header() {
                push("derived", &h);
            }
            diff.binding_lines(false).iter().for_each(|l| push("derived", l));
            let gained: Vec<_> = (gained.iter())
                .map(|(g, m, fs)| json!({"grant": g, "method": m, "functions": fs}))
                .collect();
            let bindings = kio_host::bindings_json(&diff);
            let v = json!({"grants": diff.grants, "funcs": diff.funcs, "bindings": bindings, "added": diff.added, "gained": gained});
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
        let (_, out) = captured(true, || bind(new_path, g, extra));
        value["command_line"] = json!(extra);
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
            let items = bound(g, bytes, extra);
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

/// The flags of a grants file that grant a capability; the others set limits or serve.
const GRANT_FLAGS: [&str; 12] = [
    "--dir",
    "--http",
    "--http-methods",
    "--http-header",
    "--kv",
    "--db",
    "--rng",
    "--clock",
    "--log",
    "--hmac",
    "--secret",
    "--value",
];

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
        // The binds line shows the grants; what else the file sets (limits, `--audit`, `--listen`)
        // shows here (R1d7).
        let mut lines = vec!["new file".to_string()];
        let other = |f: &String| !GRANT_FLAGS.iter().any(|g| f.split(' ').next() == Some(g));
        lines.extend(
            parse(new)
                .unwrap_or_default()
                .iter()
                .filter(|f| other(f))
                .map(|f| format!("+ {f}")),
        );
        return (
            lines.iter().map(|l| kio_host::escape_line(l)).collect(),
            json!("new file"),
        );
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
/// Then each grant of `extra` (the flags after `--grants`, R1d4), marked `(command line)`.
fn bound(g: &str, bytes: &[u8], extra: &[String]) -> Vec<String> {
    let text = std::str::from_utf8(bytes).unwrap_or_default();
    let Ok(mut flags) = captured(true, || grants_flags(text, g, &[])).0 else {
        return vec![];
    };
    let in_file = flags.len() / 2;
    flags.extend(extra.iter().cloned());
    let shown = ascii;
    let pairs: Vec<(&str, &str)> = flags.chunks(2).map(|p| (p[0].as_str(), p[1].as_str())).collect();
    let mut items = Vec::new();
    for (i, &(flag, spec)) in pairs.iter().enumerate() {
        let given = if i < in_file { "" } else { " (command line)" };
        let item = match flag {
            "--dir" | "--kv" | "--db" | "--rng" | "--clock" | "--log" | "--hmac" | "--secret" | "--value" => {
                shown(spec)
            }
            "--listen" | "--host" | "--ws-origin" => format!("{flag} {}", shown(spec)),
            "--http" => {
                let name = spec.split_once('=').map_or(spec, |(n, _)| n);
                let extras: Vec<String> = (pairs.iter().enumerate())
                    .filter(|(_, (f, _))| *f == "--http-methods" || *f == "--http-header")
                    .filter_map(|(j, (_, s))| Some((j, s.split_once('=').filter(|(n, _)| *n == name)?.1)))
                    .map(|(j, v)| {
                        format!(
                            "{}{}",
                            shown(v),
                            if j < in_file || i >= in_file {
                                ""
                            } else {
                                " (command line)"
                            }
                        )
                    })
                    .collect();
                match extras.is_empty() {
                    true => shown(spec),
                    false => format!("{} ({})", shown(spec), extras.join("; ")),
                }
            }
            _ => continue,
        };
        items.push(item + given);
    }
    items
}

/// `s` as the page shows a part of a grant: ASCII only, `\` doubled, so no character can pass for
/// another (`о` for `o`, `，` for `,`), and quoted when it holds a character that separates parts.
fn ascii(s: &str) -> String {
    let s = ascii_chars(s);
    match s.contains([',', ';', '(', ')', '"', ' ']) {
        true => format!("\"{}\"", s.replace('"', "\\\"")),
        false => s,
    }
}

/// `s` ASCII only, `\` doubled and every other character escaped: `ascii` without the quotes.
fn ascii_chars(s: &str) -> String {
    (s.chars())
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            ' '..='~' => c.to_string(),
            c => format!("\\u{{{:02x}}}", c as u32),
        })
        .collect()
}

/// `kio check path --grants g`'s binding, its errors in `--json`'s object; dry, so a server's
/// grants bind here too. `extra`: grant flags given after the file, as `kio run` takes them.
fn bind(path: &str, g: &str, extra: &[String]) -> i32 {
    let given: Vec<&str> = ["--grants", g]
        .into_iter()
        .chain(extra.iter().map(String::as_str))
        .collect();
    let (flags, grants_file) = match with_grants(&given) {
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

/// The tests a review runs (R1d3): the root's under T, then each entry's under its
/// `--entry-test-grants`, else T.
struct Suite<'a> {
    /// `None` for the root, else the entry's name: its tests are `NAME: test` on the page.
    entry: Option<&'a str>,
    test: &'a TestGrants,
    /// The program on each side: OLD's (`None`: there is none) and NEW's.
    old: Option<&'a (String, Program)>,
    new: &'a (String, Program),
}

impl Suite<'_> {
    /// What the page puts before each of its tests' names.
    fn prefix(&self) -> String {
        self.entry.map_or(String::new(), |e| format!("{e}: "))
    }
}

/// The root's suite, then each entry's.
fn suites(inputs: &Inputs) -> Vec<Suite<'_>> {
    let mut out = vec![Suite {
        entry: None,
        test: &inputs.test,
        old: Some(&inputs.old),
        new: &inputs.new,
    }];
    for e in &inputs.entries {
        out.push(Suite {
            entry: Some(&e.name),
            test: e.test.as_ref().unwrap_or(&inputs.test),
            old: e.old.as_ref(),
            new: &e.new,
        });
    }
    out
}

/// Per suite on one side (`progs`, `None` without a program), the tests it leaves to an earlier
/// suite whose tests set up there (`up`): a shared module's run once, as the earliest such
/// program's. One whose tests do not set up leaves its shared tests to the next (F4).
fn skips(progs: &[Option<&Program>], up: &[bool]) -> Vec<Vec<String>> {
    (0..progs.len())
        .map(|k| {
            let Some(p) = progs[k] else { return Vec::new() };
            (0..k)
                .filter(|&j| up[j])
                .filter_map(|j| progs[j])
                .flat_map(|q| shared_tests(q, p))
                .collect()
        })
        .collect()
}

/// Each `--http-record G=D` of tests' grants `t` (as `G=D`) that no test of `prog` can take
/// (none takes `G: Http` and `D: Dir`): a record serves only a test that reads the Dir, so
/// OLD's tests run without one only NEW's tests take (F1, #372).
fn unused_records(t: &TestGrants, prog: &Program) -> Vec<String> {
    let (Ok(p), Some((file, _))) = (kio_host::check_program(&prog.files), &t.file) else {
        return Vec::new();
    };
    let (flags, _) = captured(true, || with_grants(&["--grants", file]));
    let Ok((flags, _)) = flags else { return Vec::new() };
    let tests = p.tests();
    let takes = |t: &lang::TestFn, n: &str, ty: &str| t.params.iter().any(|(m, k)| m == n && k == ty);
    (flags.chunks(2))
        .filter(|p| p[0] == "--http-record")
        .filter_map(|p| p.get(1))
        .filter(|v| {
            v.split_once('=')
                .is_some_and(|(g, d)| !tests.iter().any(|t| takes(t, g, "Http") && takes(t, d, "Dir")))
        })
        .cloned()
        .collect()
}

/// The tests of `p` that `q` has too, with the same tokens in every function they may run and
/// every constant and type those use: a shared module's, run once, as `q`'s.
fn shared_tests(q: &Program, p: &Program) -> Vec<String> {
    let none = no_program();
    let (cq, cp) = (Changes::of(&none, q), Changes::of(&none, p));
    let (Some(tq), Some(tp)) = (&cq.new, &cp.new) else {
        return vec![];
    };
    let theirs: BTreeSet<String> = tq.tests().into_iter().map(|t| t.name).collect();
    (tp.tests().into_iter().map(|t| t.name))
        .filter(|n| theirs.contains(n) && same_as_root(&cq, &cp, n))
        .collect()
}

/// A test as the page names it (`FILE: name` for an entry's), its name in its program, its
/// suite and its result.
struct Shown<'a> {
    name: String,
    bare: String,
    suite: usize,
    r: &'a TestResult,
}

/// Whether `s` failed.
fn failed(s: &Shown) -> bool {
    s.r.error.is_some()
}

/// One suite's tests on one side: `None` without a program there, else the results or why they
/// did not run.
type Run<'a> = Option<Result<&'a [TestResult], &'a Vec<serde_json::Value>>>;

/// Each test of `runs` (one side's, per suite) as the page names it, of the suites whose NEW
/// tests ran (`new_runs`): a suite that did not run has none to compare.
fn shown<'a>(runs: &[Run<'a>], new_runs: &[Run], suites: &[Suite]) -> Vec<Shown<'a>> {
    let mut out = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        if let (Some(Ok(rs)), Some(Ok(_))) = (r, &new_runs[i]) {
            let prefix = suites[i].prefix();
            out.extend(rs.iter().map(|r| Shown {
                name: format!("{prefix}{}", r.name),
                bare: r.name.clone(),
                suite: i,
                r,
            }));
        }
    }
    out
}

/// The Tests grants line of `t`:the file (or the defaults) and each fake it answers from.
fn grants_line(t: &TestGrants) -> String {
    let answered: Vec<String> = (t.fakes.iter())
        .map(|(n, f)| format!("{} answered from {}", ascii(n), ascii(f)))
        .collect();
    match answered.is_empty() {
        true => format!("grants: {}", t.shown()),
        false => format!("grants: {} ({})", t.shown(), answered.join(", ")),
    }
}

/// The Tests section, and the Resources section from the same runs: `--json`'s value, whether a
/// test was removed, tests less or a mutant of the mutations file survived, and whether NEW's
/// tests do not run, a NEW test failed or the mutants did not all run. Each suite's tests run
/// with its tests' grants file only, else the defaults: never `main`'s grants. OLD's tests that
/// do not run are `not checked`, as OLD's failures are (R1d2).
fn tests(inputs: &Inputs, page: &mut Page) -> (serde_json::Value, bool, bool) {
    let suites = suites(inputs);
    // A secret a tests' grants file names that cannot be read fails every test granted it, on
    // both sides: one error.
    for (prefix, t) in test_grants(inputs) {
        let Some(e) = unreadable_secret(&t.flags()) else {
            continue;
        };
        let flag = if prefix.is_empty() {
            "--test-grants"
        } else {
            "--entry-test-grants"
        };
        let line = format!("{prefix}{flag} {} cannot be applied; no test ran:", t.shown());
        page.tests.push(("error", line));
        page.tests.extend(
            e.lines()
                .map(|l| ("", format!("  {}", kio_host::escape_line(l.trim_start())))),
        );
        resources(&[], &[], &[], &[], page);
        return (
            json!({"grants": t.shown(), "fakes": fakes_json(t), "setup_error": e, "mutants": null}),
            false,
            true,
        );
    }
    for (prefix, t) in test_grants(inputs) {
        page.tests.push(("tested", format!("{prefix}{}", grants_line(t))));
    }
    let seed = inputs.seed.to_string();
    let set_up = |s: &Suite, (path, prog): &(String, Program), unrecorded: &[String]| {
        let flags: Vec<&str> = s.test.flags().into_iter().chain(["--seed", &seed]).collect();
        let (t, out) = captured(true, || {
            test_setup_of(path, Some(prog.clone()), &flags, &s.test.fake_files, unrecorded)
        });
        t.map_err(|_| out.errors)
    };
    // Each side's suites set up, then each told the shared tests an earlier one that set up
    // runs (`skips`).
    let with_skips = |sets: &mut [Option<Result<Tests, Vec<serde_json::Value>>>], progs: &[Option<&Program>]| {
        let up: Vec<bool> = sets.iter().map(|t| matches!(t, Some(Ok(_)))).collect();
        for (t, skip) in sets.iter_mut().zip(skips(progs, &up)) {
            if let Some(Ok(t)) = t {
                t.skip = skip;
            }
        }
    };
    // Threads for the runs (`jobs`, T7), at most that many runs at once on both sides together.
    let mut new_sets: Vec<_> = (suites.iter()).map(|s| Some(set_up(s, s.new, &[]))).collect();
    let new_progs: Vec<Option<&Program>> = suites.iter().map(|s| Some(&s.new.1)).collect();
    with_skips(&mut new_sets, &new_progs);
    let new_sets: Vec<Result<Tests, _>> = new_sets.into_iter().flatten().collect();
    let opens = |p: &Program| {
        let pairs: Vec<(&str, &str)> = p.files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        lang::check_files_for_tests(&pairs, &lang::CapTable::v0()).is_ok_and(|p| opens_db(&p))
    };
    let db = suites.iter().filter_map(|s| s.old).any(|o| opens(&o.1))
        || new_sets.iter().any(|t| t.as_ref().is_ok_and(|t| opens_db(&t.program)));
    let n = (new_sets.iter())
        .map(|t| t.as_ref().map_or(1, |t| jobs(&t.flags, db)))
        .min()
        .unwrap_or(1);
    RUN_LIMIT.store(n, std::sync::atomic::Ordering::Relaxed);
    let threaded = |mut t: Tests| -> Result<Tests, Vec<serde_json::Value>> {
        t.jobs = n;
        // Runs on more than half the cores compile each on its own thread: Wasmtime's threads
        // would come on top of them, and a run near its --timeout could cross it.
        let cores = std::thread::available_parallelism().map_or(1, |c| c.get());
        if n > 1 && n * 2 > cores {
            t.engine = t.engine.one_thread().map_err(|e| {
                vec![json!({"kind": "internal", "message": format!("{e:#}"), "hint": null, "file": null, "line": null, "col": null})]
            })?;
        }
        Ok(t)
    };
    // OLD's tests set up under T; when they do not, again without the records no OLD test can
    // take (`unused_records`), each dropped line kept for the page.
    let old_progs: Vec<Option<&Program>> = suites.iter().map(|s| s.old.map(|o| &o.1)).collect();
    type OldSide = (
        Vec<Option<Result<Vec<TestResult>, Vec<serde_json::Value>>>>,
        Vec<Vec<String>>,
    );
    let old_side = || -> OldSide {
        let mut dropped = vec![Vec::new(); suites.len()];
        let mut sets: Vec<_> = (suites.iter().enumerate())
            .map(|(i, s)| {
                let o = s.old?;
                let first = set_up(s, o, &[]);
                let gone = match first.is_ok() {
                    true => Vec::new(),
                    false => unused_records(s.test, &o.1),
                };
                if gone.is_empty() {
                    return Some(first);
                }
                // A retry that does not set up either keeps the first error.
                match set_up(s, o, &gone) {
                    Ok(again) => {
                        dropped[i] = gone.iter().map(|g| format!("--http-record {g}")).collect();
                        Some(Ok(again))
                    }
                    _ => Some(first),
                }
            })
            .collect();
        with_skips(&mut sets, &old_progs);
        let runs = (sets.into_iter())
            .map(|t| t.map(|t| t.and_then(threaded).map(|t| run_tests(&t))))
            .collect();
        (runs, dropped)
    };
    let new_sets: Vec<_> = new_sets.into_iter().map(|t| t.and_then(threaded)).collect();
    let run_new = |sets: Vec<Result<Tests, _>>| -> Vec<Result<(Vec<TestResult>, Tests), _>> {
        sets.into_iter().map(|t| t.map(|t| (run_tests(&t), t))).collect()
    };
    // Both sides' tests at once when no test can see another's effects; OLD's set up on a
    // thread of its own, as a checked program stays on its thread.
    let ((old, dropped), new) = match n > 1 {
        true => std::thread::scope(|s| {
            let thread = std::thread::Builder::new().stack_size(kio_host::RUN_STACK);
            let old = thread.spawn_scoped(s, old_side).expect("cannot start a thread");
            let new = run_new(new_sets);
            (old.join().unwrap_or_else(|p| std::panic::resume_unwind(p)), new)
        }),
        false => {
            let old = old_side();
            (old, run_new(new_sets))
        }
    };
    let old_runs: Vec<Run> = (old.iter())
        .map(|r| r.as_ref().map(|r| r.as_ref().map(Vec::as_slice)))
        .collect();
    let new_runs: Vec<Run> = (new.iter())
        .map(|r| Some(r.as_ref().map(|(r, _)| r.as_slice())))
        .collect();
    let mut sides = json!({});
    for (name, runs) in [("old", &old_runs), ("new", &new_runs)] {
        let results: Vec<&TestResult> = (runs.iter().flatten().flatten()).flat_map(|r| r.iter()).collect();
        let failed = results.iter().filter(|r| r.error.is_some()).count();
        // A side that predates tests has none: 0 tests, not a failure.
        if results.is_empty() && runs.iter().flatten().any(|r| r.is_ok()) {
            page.tests.push(("tested", format!("{name}: 0 tests")));
        } else if !results.is_empty() {
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
        }
        let mut errors = Vec::new();
        for (i, r) in runs.iter().enumerate() {
            let Some(Err(e)) = r else { continue };
            // OLD's tests not running backs nothing either.
            let label = if name == "old" { "not checked" } else { "error" };
            let prefix = suites[i].prefix();
            page.tests
                .push((label, format!("{name}: {prefix}the tests do not run:")));
            push_errors(&mut page.tests, e);
            errors.extend(e.iter().cloned());
        }
        sides[name] = json!({"passed": results.len() - failed, "failed": failed, "errors": errors});
    }
    for (i, d) in dropped.iter().enumerate().filter(|(_, d)| !d.is_empty()) {
        let line = format!(
            "old: {}tests run without {} (no old test takes its Dir)",
            suites[i].prefix(),
            d.join(", ")
        );
        page.tests.push(("tested", line));
    }
    let none = no_program();
    let changes: Vec<Changes> = (suites.iter())
        .map(|s| Changes::of(s.old.map_or(&none, |o| &o.1), &s.new.1))
        .collect();
    // Compared per suite, only where NEW's tests ran: a suite that did not run has none to
    // compare. Where OLD's did not (only an entry's OLD that does not check, after the retry
    // above), every NEW test of that suite is added.
    let (mut old_r, new_r) = (
        shown(&old_runs, &new_runs, &suites),
        shown(&new_runs, &new_runs, &suites),
    );
    let old_down: Vec<usize> = (0..suites.len())
        .filter(|&i| matches!(old_runs[i], Some(Err(_))))
        .collect();
    let find = |rs: &[Shown], name: &str| rs.iter().position(|r| r.name == name);
    // A module's test that runs in another suite on the other side (another program loads its
    // module first there) is the same test when its text is: matched by its name in its
    // program (F3).
    let mut old_texts: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    for o in 0..old_r.len() {
        let (bare, at) = (old_r[o].bare.clone(), old_r[o].suite);
        if !bare.contains("::") || find(&new_r, &old_r[o].name).is_some() {
            continue;
        }
        let texts =
            (old_texts.entry(at)).or_insert_with(|| suites[at].old.map_or_else(BTreeMap::new, |p| fn_texts(&p.1)));
        let same = |n: &&Shown| {
            n.bare == bare
                && n.suite != at
                && find(&old_r, &n.name).is_none()
                && texts
                    .get(&bare)
                    .is_some_and(|t| changes[n.suite].texts.get(&bare) == Some(t))
        };
        if let Some(n) = new_r.iter().find(same) {
            old_r[o].name = n.name.clone();
        }
    }
    let added: Vec<&Shown> = new_r.iter().filter(|n| find(&old_r, &n.name).is_none()).collect();
    let removed: Vec<&str> = (old_r.iter().map(|r| r.name.as_str()))
        .filter(|n| find(&new_r, n).is_none())
        .collect();
    // An added test that fails is an `error`, not evidence.
    for a in &added {
        let label = if failed(a) { "error" } else { "tested" };
        let why = match old_down.contains(&a.suite) {
            true => " (old's tests do not run)",
            false => "",
        };
        page.tests.push((label, format!("added: {}{why}", a.name)));
    }
    page.tests
        .extend(removed.iter().map(|n| ("not checked", format!("removed: {n}"))));
    // A test on both sides that changed, or reaches a function that did, tests something else
    // now: read it like an added one. One matched in another suite is compared with that
    // suite's OLD program.
    let mut changed: Vec<&str> = Vec::new();
    for (n, o) in new_r.iter().filter_map(|n| Some((n, &old_r[find(&old_r, &n.name)?]))) {
        let mut fs = match (o.suite == n.suite, suites[o.suite].old) {
            (false, Some(op)) => Changes::of(&op.1, &suites[n.suite].new.1).reached(&[&n.bare]),
            _ => changes[n.suite].reached(&[&n.bare]),
        };
        if fs.is_empty() {
            continue;
        }
        changed.push(&n.name);
        fs.retain(|f| *f != n.bare);
        let line = match fs.is_empty() {
            true => format!("changed: {}", n.name),
            false => format!("changed: {} (reaches changed {})", n.name, fs.join(", ")),
        };
        page.tests.push(("not checked", line));
    }
    // A changed function no test reaches ran in no test: per program, one line naming them.
    let unreached = unreached_changed(&suites, &changes);
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
    for o in &old_r {
        let (Some(n), Some(op)) = (find(&new_r, &o.name).map(|i| &new_r[i]), &o.r.prop) else {
            continue;
        };
        let (or, nr) = (o.r, n.r);
        if or.error.is_some() || nr.error.is_some() {
            continue;
        }
        let line = match &nr.prop {
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
        let cases = nr.prop.as_ref().map_or(1, |p| p.compiled);
        fewer.push(json!({"name": o.name, "cases": cases, "old_cases": op.compiled, "property": nr.prop.is_some()}));
    }
    let mut properties = Vec::new();
    for r in &new_r {
        if let Some(p) = &r.r.prop {
            // A failed property's cases are its FAIL line below, not evidence.
            let label = if failed(r) { "error" } else { "tested" };
            page.tests.push((label, format!("{}: {}", r.name, p.summary())));
            properties.push(json!({"name": r.name, "cases": p.compiled, "seed": p.seed, "discarded": p.discarded}));
        }
    }
    let mut failures = Vec::new();
    for (i, run) in new.iter().enumerate() {
        let Ok((results, t)) = run else { continue };
        let prefix = suites[i].prefix();
        for r in results.iter().filter(|r| r.error.is_some()) {
            // The FAIL line is labelled; the lines under it share its label.
            let mut lines = result_lines(t, r);
            if let Some(rest) = lines[0].strip_prefix("FAIL ") {
                lines[0] = format!("FAIL {prefix}{rest}");
            }
            page.tests.extend(
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, l)| (if i == 0 { "error" } else { "" }, l)),
            );
            let mut row = result_json(r);
            row["name"] = json!(format!("{prefix}{}", r.name));
            failures.push(row);
        }
    }
    // Named as compared: a test paired with another suite's by that suite's name (F3).
    let mut old_failed = Vec::new();
    for (i, r) in old_runs.iter().enumerate() {
        let Some(Ok(rs)) = r else { continue };
        let name = |b: &str| match old_r.iter().find(|o| o.suite == i && o.bare == b) {
            Some(o) => o.name.clone(),
            None => format!("{}{b}", suites[i].prefix()),
        };
        old_failed.extend((rs.iter().filter(|r| r.error.is_some())).map(|r| name(&r.name)));
    }
    page.tests.extend(
        old_failed
            .iter()
            .map(|n| ("not checked", format!("failed in old: {n}"))),
    );
    // Mutants of NEW only, once the root's tests ran (R1b1, R1b2), with each entry's that ran.
    let runs: Vec<mutants::SuiteRun> = (new.iter().enumerate())
        .filter_map(|(i, r)| {
            let (results, tests) = r.as_ref().ok()?;
            Some(mutants::SuiteRun {
                suite: i,
                tests,
                results,
            })
        })
        .collect();
    let (mutants, unfinished, survived) = match runs.first() {
        Some(r) if r.suite == 0 => mutants::run(inputs, &suites, &changes, &runs, &mut page.tests),
        _ => (json!(null), false, false),
    };
    let down: Vec<String> = (old_down.iter()).map(|&i| suites[i].prefix()).collect();
    resources(&old_r, &new_r, &changed, &down, page);
    let failed = new.iter().any(|r| r.is_err()) || !failures.is_empty() || unfinished;
    let added: Vec<&str> = added.iter().map(|a| a.name.as_str()).collect();
    let value = json!({
        "grants": inputs.test.file.as_ref().map(|(t, _)| t), "fakes": fakes_json(&inputs.test),
        "old": sides["old"], "new": sides["new"], "added": added, "removed": removed, "changed": changed,
        "unreached_changed": unreached, "fewer_cases": fewer, "properties": properties, "failures": failures, "old_failed": old_failed,
        "setup_error": null, "mutants": mutants,
    });
    (value, !removed.is_empty() || !fewer.is_empty() || survived, failed)
}

/// Each function's, constant's and type's tokens by key (tests included), as text: whitespace
/// and comments aside, string literals exact. None if `prog` does not check.
fn fn_texts(prog: &Program) -> BTreeMap<String, String> {
    let Ok(p) = kio_host::check_program(&prog.files) else {
        return BTreeMap::new();
    };
    (p.fn_spans().into_iter().chain(p.item_spans()))
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

/// Whether function `k` of the program `c` holds runs as the root's (`root`) of its key: every
/// function it may run, and every constant and type those name, has the same tokens in both.
fn same_as_root(root: &Changes, c: &Changes, k: &str) -> bool {
    let Some(p) = &c.new else { return false };
    let fs = lang::reachable(p, [k]);
    let same = |k: &String| root.texts.get(k) == c.texts.get(k);
    fs.iter().all(same) && lang::items_used(p, &fs).iter().all(same)
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

/// Per suite's program (`changes`, the root's then each `--entry`'s), its changed functions
/// `main` reaches that no test reaches (`lang::reachable` from every test, as the report's
/// `reached only from tests:`), in key order; a program with none is left out. A function
/// counts as reached by another program's tests when they reach one of its key and it runs as
/// there (`same_as_root`, #290, #312), so a module two entries share is tested by either's.
fn unreached_changed<'a>(suites: &[Suite<'a>], changes: &[Changes]) -> Vec<(Option<&'a str>, Vec<String>)> {
    let reach: Vec<BTreeSet<String>> = changes.iter().map(tests_reach).collect();
    let mut out = Vec::new();
    for (i, c) in changes.iter().enumerate() {
        let tested = |k: &String| {
            (0..changes.len()).any(|j| reach[j].contains(k) && (j == i || same_as_root(&changes[j], c, k)))
        };
        let fs: Vec<String> = c.reached(&["main"]).into_iter().filter(|k| !tested(k)).collect();
        if !fs.is_empty() {
            out.push((suites[i].entry, fs));
        }
    }
    out
}

/// What the tests of `c`'s NEW program may run, themselves included.
fn tests_reach(c: &Changes) -> BTreeSet<String> {
    let Some(p) = &c.new else { return BTreeSet::new() };
    let tests: Vec<String> = p.tests().into_iter().map(|t| t.name).collect();
    lang::reachable(p, tests.iter().map(String::as_str))
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
/// that failed on either side stopped early: it is left out, and counted. A row for a test not in
/// `changed_tests` (it reaches no changed function) says so. `old_down`: the prefix of each suite
/// whose OLD tests did not run (R1d2).
fn resources(old: &[Shown], new: &[Shown], changed_tests: &[&str], old_down: &[String], page: &mut Page) {
    // Every test that failed in NEW, and those on both sides that failed in OLD only.
    let failed = new.iter().filter(|n| self::failed(n)).count();
    let mut failed_old = 0;
    let pairs: Vec<(&str, Used, Used)> = (old.iter())
        .filter_map(|o| {
            let n = new.iter().find(|n| n.name == o.name)?;
            let (or, nr) = (o.r, n.r);
            if nr.error.is_some() {
                return None;
            }
            if or.error.is_some() {
                failed_old += 1;
                return None;
            }
            Some((o.name.as_str(), or.used?, nr.used?))
        })
        .collect();
    for prefix in old_down {
        (page.resources).push(format!("old: {prefix}the tests do not run; left out of Resources"));
    }
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
            let mut line = row(name, "peak memory", o, n);
            if !changed_tests.contains(name) {
                line += "; unchanged code";
            }
            page.resources.push(line);
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
        .filter(|n| !self::failed(n) && !old.iter().any(|o| o.name == n.name))
        .filter_map(|n| Some((n.name.as_str(), n.r.used?)))
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
    let entries = (inputs.entries.iter()).map(|e| {
        let old = e.old.as_ref().map_or(&none, |o| &o.1);
        let ro = read_only_dbs(e.grants.as_ref(), &e.extra);
        (Some(e.name.as_str()), old, &e.new.1, e.old_absent, ro)
    });
    let ro = read_only_dbs(inputs.grants.as_ref(), &inputs.extra);
    let root = (None, &inputs.old.1, &inputs.new.1, inputs.old_absent, ro);
    for (entry, old, prog, added, ro) in [root].into_iter().chain(entries) {
        let Ok(p) = kio_host::check_program(&prog.files) else {
            continue;
        };
        // Each changed function marked below, and the grants it is marked under.
        let mut marked: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        let changes = Changes::of(old, prog);
        let prefix = entry.map_or(String::new(), |e| format!("{e}: "));
        let a = lang::authority(&p);
        for (param, methods) in a.funcs.get("main").into_iter().flatten() {
            let Some((_, ty)) = a.main.iter().find(|(n, _)| n == param) else {
                continue;
            };
            let Some(&(_, of, mut what)) = BLIND_SPOTS.iter().find(|(t, _, _)| t == ty) else {
                continue;
            };
            // A `Db` the grants file binds `:ro` changes nothing (R1d7).
            if ty == "Db" && ro.contains(param) {
                what = "the SQL text and what it reads";
            }
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
            for f in &users {
                marked.entry(f.clone()).or_default().push(param);
            }
            page.not_checked
                .push(format!("{prefix}{param}: {ty} {}: {what}", reached.join(", ")));
            rows.push(
                json!({"entry": entry, "grant": param, "type": ty, "methods": reached, "what": what, "changed": users}),
            );
        }
        // Each changed function once, not once per grant (R1c): those marked under the same
        // grants share a line.
        let mut groups: Vec<(Vec<&str>, &Vec<&str>)> = Vec::new();
        for (f, grants) in &marked {
            match groups.iter_mut().find(|(_, g)| *g == grants) {
                Some((fs, _)) => fs.push(f),
                None => groups.push((vec![f], grants)),
            }
        }
        let why = if added { "new entry" } else { "changed" };
        for (fs, grants) in groups {
            page.not_checked
                .push(format!("{prefix}{}: {} ({why})", fs.join(", "), grants.join(", ")));
        }
        // A leaf bound to two or more grants of one type (L32): which call site binds which is
        // behaviour, which no line shows; a reordering of those calls moves nothing above.
        let origins = lang::origins(&p);
        for ((f, leaf), grants) in &origins {
            if f == "main" || grants.len() < 2 {
                continue;
            }
            let ty = |g: &String| a.main.iter().find(|(n, _)| n == g).map(|(_, t)| t.clone());
            let mut tys: Vec<Option<String>> = grants.iter().map(ty).collect();
            tys.sort();
            tys.dedup();
            if tys.len() == grants.len() {
                continue;
            }
            // Marked when a caller's text changed (R1a4): the callers `main` reaches.
            let users: Vec<String> = (lang::reachable(&p, ["main"]).into_iter())
                .filter(|c| c != f && changes.changed.contains(c) && lang::reachable(&p, [c.as_str()]).contains(f))
                .collect();
            let mark = match users.is_empty() {
                true => String::new(),
                false => format!(" (changed: {})", users.join(", ")),
            };
            let from: Vec<String> = grants.iter().cloned().collect();
            let what = "which call uses which is behaviour";
            page.not_checked
                .push(format!("{prefix}{f}: {leaf} from {}; {what}{mark}", from.join(", ")));
            rows.push(
                json!({"entry": entry, "function": f, "leaf": leaf, "from": from, "what": what, "changed": users}),
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
    let mut fakes_changed = Vec::new();
    for (prefix, t) in test_grants(inputs) {
        for (i, (n, f)) in t.fakes.iter().enumerate() {
            page.not_checked.push(format!(
                "{prefix}{}: Http: tests ran against {}, not a server; the real endpoint's answers are untested",
                ascii(n),
                ascii(f)
            ));
            // The answers changed with the code: read them as part of the change (R1d7).
            let first = t.fakes.iter().position(|(_, x)| x == f) == Some(i);
            if let Some((added, removed)) = fake_change(t, f).filter(|_| first) {
                let what = format!("{added} -{removed} lines");
                page.not_checked
                    .push(format!("{prefix}fake {} changed (+{what})", ascii(f)));
                fakes_changed.push(json!({"path": f, "added": added, "removed": removed}));
            }
        }
    }
    page.not_checked.push(SAME_AUTHOR.into());
    page.not_checked_json = json!({"methods": rows, "deps": dep_rows, "fakes": fakes_json(&inputs.test), "fakes_changed": fakes_changed, "author": SAME_AUTHOR});
}

/// The lines added and removed from the fake FILE `f` beside OLD's program to NEW's, when both
/// were read and differ (a diff too large to compare counts every line).
fn fake_change(t: &TestGrants, f: &str) -> Option<(usize, usize)> {
    let new = t.fake_files.iter().find(|(p, _)| p == f)?.1.as_ref().ok()?;
    let old = match t.old_fakes.iter().find(|(p, _)| p == f)? {
        (_, Ok((_, OldGrants::File(_, b)))) => b,
        _ => return None,
    };
    if old == new {
        return None;
    }
    let (o, n) = (String::from_utf8_lossy(old), String::from_utf8_lossy(new));
    let count = |d: &[String], c: char| d.iter().filter(|l| l.starts_with(c) && !l.starts_with("@@")).count();
    Some(match unified(&o, &n) {
        Ok(d) => (count(&d, '+'), count(&d, '-')),
        Err(_) => (n.lines().count(), o.lines().count()),
    })
}

/// The `--db` grants of a grants file and the flags after it that are `:ro`: their names.
fn read_only_dbs(grants: Option<&(String, Vec<u8>)>, extra: &[String]) -> Vec<String> {
    let text = grants.map_or("", |(_, b)| std::str::from_utf8(b).unwrap_or_default());
    let g = grants.map_or("", |(g, _)| g.as_str());
    let mut flags = captured(true, || grants_flags(text, g, &[])).0.unwrap_or_default();
    flags.extend(extra.iter().cloned());
    (flags.chunks(2))
        .filter(|p| p[0] == "--db")
        .filter_map(|p| p[1].split_once('='))
        .filter(|(_, spec)| kio_host::kv::split_spec(spec).1)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Each fake of a tests' grants file, for `--json`.
fn fakes_json(t: &TestGrants) -> serde_json::Value {
    json!(t
        .fakes
        .iter()
        .map(|(n, f)| json!({"grant": n, "path": f}))
        .collect::<Vec<_>>())
}

/// Each program one side loads: its root, then each `--entry` it has; whether its root is no
/// file (OLD's modules of a new entry, `read_old`).
fn side_programs(inputs: &Inputs, new: bool) -> Vec<(&Program, bool)> {
    let root = match new {
        true => (&inputs.new.1, false),
        false => (&inputs.old.1, inputs.old_absent),
    };
    let entries = (inputs.entries.iter()).filter_map(|e| match new {
        true => Some((&e.new.1, false)),
        false => e.old.as_ref().map(|o| (&o.1, e.old_absent)),
    });
    [root].into_iter().chain(entries).collect()
}

/// The dependencies one side's programs load, each once.
fn side_deps(inputs: &Inputs, new: bool) -> Vec<kio_host::Dep> {
    let mut deps: Vec<kio_host::Dep> = Vec::new();
    for d in side_programs(inputs, new).into_iter().flat_map(|(p, _)| &p.deps) {
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
        // Every file of the root and each entry; one they share (a module) once. A root that is
        // no file is absent, and must stay so.
        let mut each: Vec<(String, Option<String>)> = (side_programs(inputs, new).into_iter())
            .flat_map(|(prog, absent)| {
                (0..prog.files.len()).map(move |i| match i == 0 && absent {
                    true => (prog.files[0].0.clone(), None),
                    false => (json_path(prog, i), Some(hash(prog.files[i].1.as_bytes()))),
                })
            })
            .collect();
        each.sort();
        each.dedup();
        shown.extend(each.iter().cloned());
        (page.identity).extend(each.iter().map(|(p, h)| {
            let h = h.as_ref().map_or("absent".into(), |h| format!("sha256:{h}"));
            format!("{side} {} {h}", kio_host::escape_line(p))
        }));
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
    let mut file = |what: &str, f: &Option<(String, Vec<u8>)>, show: fn(&str) -> String| {
        f.as_ref().map(|(g, bytes)| {
            let path = show(g);
            page.identity.push(format!("{what} {path} sha256:{}", hash(bytes)));
            shown.push((g.clone(), Some(hash(bytes))));
            json!({"path": g, "sha256": hash(bytes)})
        })
    };
    let old_of = |b: &Beside| b.as_ref().ok().and_then(|(_, o)| o.file());
    let grants = file("grants", &inputs.grants, kio_host::escape_line);
    let old_grants = file("old grants", &old_of(&inputs.old_grants), kio_host::escape_line);
    let (mut tests_json, mut test_fakes, mut old_test_fakes) = (Vec::new(), Vec::new(), Vec::new());
    for (prefix, t) in test_grants(inputs) {
        let what = if prefix.is_empty() {
            "test grants"
        } else {
            "entry test grants"
        };
        tests_json.push(file(&format!("{prefix}{what}"), &t.file, kio_host::escape_line));
        // A FILE that could not be read, or that `kio test` refuses, answered nothing: its error
        // is the tests' on both sides.
        for (p, r) in t.fake_files.iter().filter(|(p, _)| t.fakes_ok.contains(p)) {
            let read = r.as_ref().ok().map(|b| (p.clone(), b.clone()));
            test_fakes.extend(file(&format!("{prefix}test fake"), &read, ascii));
        }
        for (_, old) in &t.old_fakes {
            old_test_fakes.extend(file(
                &format!("{prefix}old test fake"),
                &old_of(old),
                kio_host::escape_line,
            ));
        }
    }
    // The root's first, then each entry's own.
    let entry_test_grants: Vec<_> = tests_json.split_off(1).into_iter().flatten().collect();
    let test_grants = tests_json.pop().flatten();
    let entry_grants: Vec<_> = (inputs.entries.iter())
        .filter_map(|e| file(&format!("{}: entry grants", e.name), &e.grants, kio_host::escape_line))
        .collect();
    let old_entry_grants: Vec<_> = (inputs.entries.iter())
        .filter_map(|e| {
            file(
                &format!("{}: old entry grants", e.name),
                &old_of(&e.old_grants),
                kio_host::escape_line,
            )
        })
        .collect();
    let mut files = Vec::new();
    let requirements = inputs.requirements.as_ref().map(|(_, sides)| {
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
    let mutations = inputs.mutations.as_ref().map(|m| {
        let h = hash(&m.bytes);
        let inside = if m.inside { " (inside NEW's tree)" } else { "" };
        page.identity
            .push(format!("mutations {} sha256:{h}{inside}", ascii(&m.path)));
        shown.push((m.path.clone(), Some(h.clone())));
        json!({"path": m.path, "sha256": h, "inside_new": m.inside})
    });
    page.identity.push(format!("test seed {}", inputs.seed));
    shown.extend(files);
    page.changed = changed(&shown);
    json!({
        "kio": version, "commit": commit, "old": sides["old"], "new": sides["new"], "deps": deps,
        "grants": grants, "test_grants": test_grants, "test_fakes": test_fakes, "entry_test_grants": entry_test_grants, "old_test_fakes": old_test_fakes, "entry_grants": entry_grants, "old_grants": old_grants, "old_entry_grants": old_entry_grants, "requirements": requirements, "mutations": mutations, "seed": inputs.seed,
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
