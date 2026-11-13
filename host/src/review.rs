//! `kio review NEW --base OLD`: one page from the evidence `kio check` and `kio test` produce
//! (R1a). Every line carries its label; nothing on the page is a verdict.
use super::*;
use std::path::Path;

const USAGE: &str =
    "usage: kio review <new.kio> --base <old.kio> [--grants file] [--test-grants file] [--requirements file] [--json]";

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
    ("Dir", &["write", "create", "remove"], "what is written or removed"),
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
];

const SAME_AUTHOR: &str = "tests and properties were written by the same author as the code unless stated";

/// What the command line names, read: each side's files and path, the grants file for NEW's
/// `main`, the tests' grants file, and the requirements file on each side (`None` when absent).
struct Inputs {
    old: (String, Vec<(String, String)>),
    new: (String, Vec<(String, String)>),
    grants: Option<(String, Vec<u8>)>,
    test_grants: Option<(String, Vec<u8>)>,
    requirements: Option<[(String, Option<String>); 2]>,
}

/// `kio review`: exit 1 if either side does not check, `--grants` does not bind, or a NEW test
/// fails; else 3 if authority was added or a test removed; else 0. 2 for usage.
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
        println!("  {:<12} {p} changed while the review ran", "error");
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
    if !flags.len().is_multiple_of(2) {
        return Err(fail(USAGE.into()));
    }
    for pair in flags.chunks(2) {
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
    let old = (base.to_string(), read_files(base)?);
    let new = (path.to_string(), read_files(path)?);
    let read = |flag: &str, g: Option<&str>| match g {
        Some(g) => match std::fs::read(g) {
            Ok(bytes) => Ok(Some((g.to_string(), bytes))),
            Err(e) => Err(fail(format!("{flag} {g}: cannot read: {e}"))),
        },
        None => Ok(None),
    };
    let grants = read("--grants", grants)?;
    let test_grants = read("--test-grants", test_grants)?;
    if let Some(t) = &test_grants {
        shared_state(&t.0)?;
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
    Ok(Inputs {
        old,
        new,
        grants,
        test_grants,
        requirements,
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

/// The Authority section: `kio check NEW --base OLD`'s lines, and with `--grants` the errors of
/// binding it to NEW's `main`, labelled `error`. Whether both sides check, and whether the grants
/// bind.
fn authority(inputs: &Inputs, page: &mut Page) -> (bool, bool) {
    let (old, new) = (&inputs.old.1, &inputs.new.1);
    let mut errors = Vec::new();
    let checks = match kio_host::diff_files(old, new) {
        Ok(diff) => {
            page.added = diff.added;
            match diff.lines(false).0 {
                lines if lines.is_empty() => page.authority.push(("enforced", "no change in authority".into())),
                lines => page.authority.extend(labelled("enforced", &lines)),
            }
            page.authority_json = json!({"grants": diff.grants, "funcs": diff.funcs, "added": diff.added});
            true
        }
        Err((is_base, diags)) => {
            let (side, files) = if is_base { ("old", old) } else { ("new", new) };
            let msg = format!("the {side} program {} does not check", files[0].0);
            page.authority.push(("error", msg));
            let (_, out) = captured(true, || print_diags(files, diags, "check"));
            push_errors(&mut page.authority, &out.errors);
            errors = out.errors;
            page.authority_json = json!({"grants": [], "funcs": [], "added": false});
            false
        }
    };
    let mut grant_errors = Vec::new();
    if let Some((g, _)) = &inputs.grants {
        let (_, out) = captured(true, || bind(&inputs.new.0, g));
        if !out.errors.is_empty() {
            let msg = format!("--grants {g} does not bind to {}'s main:", inputs.new.0);
            page.authority.push(("error", msg));
            push_errors(&mut page.authority, &out.errors);
        }
        grant_errors = out.errors;
    }
    let binds = grant_errors.is_empty();
    page.authority_json["errors"] = json!(errors);
    page.authority_json["grant_errors"] = json!(grant_errors);
    (checks, binds)
}

/// `kio check path --grants g`'s binding, its errors in `--json`'s object. Not `run`, which
/// refuses `--listen` under `--json`: binding is dry, so a server's grants bind here too.
fn bind(path: &str, g: &str) -> i32 {
    let (flags, grants_file) = match with_grants(&["--grants", g]) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    match read_program(path) {
        Ok((files, ids)) => run_program(&files, &ids, &flags, grants_file.as_deref(), true, None),
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
    page.tests.push(("tested", format!("grants: {shown}")));
    let side = |path: &str| {
        let (tests, out) = captured(true, || test_setup(path, &flags));
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
                let line = format!("{name}: {} passed, {failed} failed", results.len() - failed);
                page.tests.push(("tested", line));
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
    page.tests
        .extend(added.iter().map(|n| ("tested", format!("added: {n}"))));
    page.tests
        .extend(removed.iter().map(|n| ("not checked", format!("removed: {n}"))));
    // A property that ran fewer cases than in OLD, or became an example test, tests less.
    let mut fewer = Vec::new();
    for o in old_r {
        let (Some(op), Some(n)) = (&o.prop, find(new_r, &o.name).map(|i| &new_r[i])) else {
            continue;
        };
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
            page.tests.push(("tested", format!("{}: {}", r.name, p.summary())));
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
    page.tests
        .extend(old_failed.iter().map(|n| ("error", format!("failed in old: {n}"))));
    resources(old_r, new_r, page);
    let failed = new.is_err() || old.is_err() || !failures.is_empty();
    let value = json!({
        "grants": inputs.test_grants.as_ref().map(|(t, _)| t),
        "old": sides["old"], "new": sides["new"], "added": added, "removed": removed,
        "fewer_cases": fewer, "properties": properties, "failures": failures, "old_failed": old_failed,
    });
    (value, !removed.is_empty() || !fewer.is_empty(), failed)
}

/// The Resources section: per test on both sides, fuel and peak memory old -> new when either
/// changed by more than 2%; then over every such test the total fuel and the largest peak.
fn resources(old: &[TestResult], new: &[TestResult], page: &mut Page) {
    let pairs: Vec<(&str, Used, Used)> = (old.iter())
        .filter_map(|o| {
            let n = new.iter().find(|n| n.name == o.name)?;
            Some((o.name.as_str(), o.used?, n.used?))
        })
        .collect();
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
    if pairs.is_empty() {
        page.resources.push("no test ran on both sides".into());
        page.resources_json = json!({"rows": rows, "total": null, "tests": 0});
        return;
    }
    if rows.is_empty() {
        page.resources.push("no test changed by more than 2%".into());
    }
    let label = format!("total over {} tests on both sides", pairs.len());
    page.resources.push(row(&label, "largest peak memory", &o, &n));
    page.resources_json = json!({"rows": rows, "total": as_json("total", &o, &n), "tests": pairs.len()});
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
/// not look at; then the line on who wrote the tests.
fn not_checked(inputs: &Inputs, page: &mut Page) {
    let mut rows = Vec::new();
    if let Ok(p) = kio_host::check_program(&inputs.new.1) {
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
            page.not_checked
                .push(format!("{param}: {ty} {}: {what}", reached.join(", ")));
            rows.push(json!({"grant": param, "type": ty, "methods": reached, "what": what}));
        }
    }
    page.not_checked.push(SAME_AUTHOR.into());
    page.not_checked_json = json!({"methods": rows, "author": SAME_AUTHOR});
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
    for (side, (_, files)) in [("old", &inputs.old), ("new", &inputs.new)] {
        let mut each: Vec<(String, String)> = (0..files.len())
            .map(|i| (json_path(files, i), hash(files[i].1.as_bytes())))
            .collect();
        each.sort();
        shown.extend(each.iter().map(|(p, h)| (p.clone(), Some(h.clone()))));
        page.identity
            .extend(each.iter().map(|(p, h)| format!("{side} {p} sha256:{h}")));
        sides[side] = json!(each
            .iter()
            .map(|(p, h)| json!({"path": p, "sha256": h}))
            .collect::<Vec<_>>());
    }
    let mut file = |what: &str, f: &Option<(String, Vec<u8>)>| {
        f.as_ref().map(|(g, bytes)| {
            page.identity.push(format!("{what} {g} sha256:{}", hash(bytes)));
            shown.push((g.clone(), Some(hash(bytes))));
            json!({"path": g, "sha256": hash(bytes)})
        })
    };
    let grants = file("grants", &inputs.grants);
    let test_grants = file("test grants", &inputs.test_grants);
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
        "kio": version, "commit": commit, "old": sides["old"], "new": sides["new"],
        "grants": grants, "test_grants": test_grants, "requirements": requirements, "seed": DEFAULT_SEED,
        "changed": page.changed,
    })
}

/// Each of `files` (a path and its sha256, `None` for absent) whose contents on disk differ now.
fn changed(files: &[(String, Option<String>)]) -> Vec<String> {
    let hash = |b: &[u8]| lang::digest::to_hex(&lang::digest::sha256(b));
    let mut out: Vec<String> = Vec::new();
    for (path, h) in files {
        if std::fs::read(path).ok().map(|b| hash(&b)) != *h && !out.contains(path) {
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
