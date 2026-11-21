//! R1b1: generated mutants. Each is NEW's text with one token-level edit in a changed function
//! that a `main` (the root's or an entry's) and a test reach, checked, then run on the tests that
//! reach it through `kio test`'s path (fresh sandboxes, the test grants only), stopping at the
//! first failure. R1b2: the mutants of a mutations file (`--mutations`), run the same way.
use super::*;
use lang::lex::{Kind, Token};

/// `--mutants` unless given, and its most.
pub(super) const DEFAULT: usize = 20;
pub(super) const MAX: usize = 500;

/// A test's fuel in a mutant run: this many times what it used on NEW.
const FUEL_FACTOR: u64 = 10;

/// The wall time every mutant run together may take; past it the run stops, an `error`.
const BUDGET_SECS: u64 = 300;

/// One edit of NEW's text: in file `file`, bytes `start..end` become `to`, inside function `func`.
struct Site {
    file: usize,
    start: usize,
    end: usize,
    to: String,
    func: String,
}

/// How a mutant's runs ended: the test that failed and how, the first test a limit of T's own
/// stopped (not a kill: the unmutated run sat at that limit), and the first property whose every
/// case was discarded (not a kill either).
#[derive(Default)]
struct End {
    killed: Option<(String, String)>,
    stopped: Option<(String, String)>,
    discarded: Option<String>,
}

/// The Tests section's mutant lines, pushed to `rows`: NEW's checked `changes`, its tests set up
/// as `base` and their unmutated `results`. `--json`'s value; whether the runs did not finish or
/// an entry of the mutations file could not run (an `error`); and whether one of its mutants
/// survived (exit 3).
pub(super) fn run(
    inputs: &Inputs,
    changes: &Changes,
    base: &Tests,
    results: &[TestResult],
    rows: &mut Vec<Row>,
) -> (serde_json::Value, bool, bool) {
    let (Some(root), true) = (&changes.new, inputs.mutants > 0 || inputs.mutations.is_some()) else {
        return (json!(null), false, false);
    };
    let new = &inputs.new.1;
    // A test holding a grant that can tell one run from the next never runs a mutant: its
    // verdict could come from the run, not the edit.
    let ok = |r: &&TestResult| r.error.is_none() && r.used.is_some();
    let live = live_grants(
        base,
        &results.iter().filter(ok).map(|r| r.name.as_str()).collect::<Vec<_>>(),
    );
    skipped_lines(&live, rows);
    // The tests that passed on NEW and may run a mutant; `reach` pairs each with the functions
    // it may run in program `p`.
    let passed: Vec<&TestResult> = (results.iter().filter(ok))
        .filter(|r| !live.iter().any(|(t, _, _)| *t == r.name))
        .collect();
    let reach = |p: &lang::Program| -> Vec<(&TestResult, BTreeSet<String>)> {
        (passed.iter())
            .map(|r| (*r, lang::reachable(p, [r.name.as_str()])))
            .collect()
    };
    let root_reach = reach(root);
    let tests: Vec<String> = root.tests().into_iter().map(|t| t.name).collect();
    let deps: Vec<String> = new.deps.iter().map(|d| format!("{}.kio", d.name)).collect();
    let spans: BTreeMap<String, lang::Span> = root.fn_spans().into_iter().collect();
    // The changed functions each program's `main` reaches; an entry's as the root's of its key,
    // when it runs as the root's (`same_as_root`, as `unreached_changed` relates them).
    let mut funcs: BTreeSet<String> = changes.reached(&["main"]).into_iter().collect();
    let none = no_program();
    for e in &inputs.entries {
        let c = Changes::of(e.old.as_ref().map_or(&none, |o| &o.1), &e.new.1);
        funcs.extend(
            c.reached(&["main"])
                .into_iter()
                .filter(|k| same_as_root(changes, &c, k)),
        );
    }
    if inputs.mutants == 0 {
        funcs.clear();
    }
    let mut sites = Vec::new();
    // Each function with sites: its key and how many.
    let mut groups: Vec<(String, usize)> = Vec::new();
    for f in funcs {
        let Some(span) = spans.get(&f) else { continue };
        let file = span.file as usize;
        let reached = root_reach.iter().any(|(_, r)| r.contains(&f));
        if !reached || tests.contains(&f) || deps.contains(&new.files[file].0) {
            continue;
        }
        let Some(text) = new.files[file].1.get(span.start..span.end) else {
            continue;
        };
        // Each comparison and logical operator, as the bytes between its operands.
        let ops: Vec<(usize, usize, &str)> = (lang::operators(root, &f).into_iter())
            .filter(|(l, r, _)| l.file == span.file && r.file == span.file)
            .map(|(l, r, op)| (l.end.saturating_sub(span.start), r.start.saturating_sub(span.start), op))
            .collect();
        let edits = edits(text, &ops);
        if !edits.is_empty() {
            groups.push((f.clone(), edits.len()));
        }
        for (start, end, to) in edits {
            let (start, end) = (span.start + start, span.start + end);
            sites.push(Site {
                file,
                start,
                end,
                to,
                func: f.clone(),
            });
        }
    }
    sites.sort_by_key(|s| (s.file, s.start, s.end));
    let order = draw(&sites, &groups, inputs.seed, inputs.mutants);
    let fuel_t = limit(&base.flags, "--fuel");
    let runner = Runner {
        base,
        fuel_t,
        fuel_max: fuel_t.unwrap_or(Limits::default().fuel),
        start: std::time::Instant::now(),
    };
    let written: &[Written] = inputs.mutations.as_ref().map_or(&[], |m| &m.entries);
    let total = written.len() + order.len();
    let (mut did_not_check, mut ran, mut unfinished) = (0, 0, None);
    let mut out: Vec<serde_json::Value> = Vec::new();
    // Each entry of the mutations file that did not run: its message and hint.
    let mut errors: Vec<(String, String)> = Vec::new();
    // The written mutants first: each names a behaviour the reviewer asked to be constrained.
    let file = inputs.mutations.as_ref().map(|m| (ascii(&m.path), m.inside));
    for w in written {
        let source = file.as_ref().map_or(String::new(), |f| f.0.clone());
        let at = format!("{source}:{}", w.line);
        let entry_checked;
        let (prog, path, checked) = match &w.program {
            None => (new, &inputs.new.0, Some(root)),
            Some(p) => match inputs.entries.iter().find(|e| plain(&e.name) == plain(p)) {
                Some(e) => {
                    entry_checked = kio_host::check_program(&e.new.1.files).ok();
                    (&e.new.1, &e.new.0, entry_checked.as_ref())
                }
                None => {
                    let p = ascii(p);
                    errors.push((
                        format!("{at}: {p} is not an --entry of this review"),
                        format!("add `--entry {p}`, or name a function of the root without a program"),
                    ));
                    continue;
                }
            },
        };
        // An entry that does not check is Authority's error already.
        let Some(checked) = checked else { continue };
        let site = match place(prog, checked, &lang::reachable(checked, ["main"]), w) {
            Ok(s) => s,
            Err((msg, hint)) => {
                errors.push((format!("{at}: {msg}"), hint));
                continue;
            }
        };
        match runner.run(prog, path, &site, &reach(checked)) {
            Ok(Ran::DidNotCheck(why)) => errors.push((
                format!("{at}: the mutant does not check: {why}"),
                "change the `+` text so the program still checks; no test can run a program the checker refuses".into(),
            )),
            Ok(Ran::Ran(end, tests_run, fuel)) => {
                ran += 1;
                // T's ceiling, not a test, ended it: the reviewer's question is unanswered.
                if let (None, Some((t, how))) = (&end.killed, &end.stopped) {
                    let limit = how.trim_start_matches("stopped by T's ");
                    errors.push((
                        format!("{at}: the mutant was {how} in {t}, so no test could pass or fail on it"),
                        format!("raise T's {limit} (the limit named) so the mutant can finish"),
                    ));
                }
                let (from, (n, col, _)) = shown_site(&prog.files[site.file].1, &site);
                let shown = (
                    (n, col),
                    from,
                    shown_text(&cut(&site.to, 60)),
                    ascii_chars(&w.description),
                );
                let program = w.program.as_ref().map(|p| ascii(p));
                let source = json!({"source": source, "written": true, "entry": w.line, "program": program});
                out.push(row(
                    source,
                    &site,
                    json_path(prog, site.file),
                    shown,
                    &end,
                    (tests_run, fuel),
                ));
            }
            Err(stop) => {
                unfinished = Some(stop.line(ran, total));
                break;
            }
        }
    }
    for &i in order.iter().take_while(|_| unfinished.is_none()) {
        let s = &sites[i];
        match runner.run(new, &inputs.new.0, s, &root_reach) {
            Ok(Ran::DidNotCheck(_)) => did_not_check += 1,
            Ok(Ran::Ran(end, tests_run, fuel)) => {
                ran += 1;
                let (from, (n, col, around)) = shown_site(&new.files[s.file].1, s);
                let to = shown_text(&s.to);
                let description = match to.is_empty() {
                    true => format!("`{from}` dropped in `{around}`"),
                    false => format!("`{from}` -> `{to}` in `{around}`"),
                };
                let source = json!({"source": "generated", "written": false});
                out.push(row(
                    source,
                    s,
                    json_path(new, s.file),
                    ((n, col), from, to, description),
                    &end,
                    (tests_run, fuel),
                ));
            }
            Err(stop) => {
                unfinished = Some(stop.line(ran, total));
                break;
            }
        }
    }
    let drawn: BTreeSet<&str> = order.iter().map(|&i| sites[i].func.as_str()).collect();
    if !out.is_empty() || !sites.is_empty() {
        let generated = (!sites.is_empty()).then_some((order.len(), sites.len(), did_not_check));
        lines(&out, inputs.seed, generated, file.as_ref(), rows);
    }
    for (f, n) in groups.iter().filter(|(f, _)| !drawn.contains(f.as_str())) {
        let s = if *n == 1 { "site" } else { "sites" };
        rows.push(("not checked", format!("no mutant drawn: {f} ({n} {s})")));
    }
    // Constants are not mutated: name those the functions with sites use.
    let mut consts: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (f, _) in &groups {
        for c in lang::items_used(root, [f]).into_iter().filter(|k| root.is_const(k)) {
            consts.entry(c).or_default().push(f);
        }
    }
    if !consts.is_empty() {
        let each: Vec<String> = consts
            .iter()
            .map(|(c, fs)| format!("{c} used by {}", fs.join(", ")))
            .collect();
        rows.push(("not checked", format!("constants are not mutated: {}", each.join("; "))));
    }
    let hinted = unfinished.iter().map(|(m, h)| (m, h.as_ref()));
    for (msg, hint) in errors.iter().map(|(m, h)| (m, Some(h))).chain(hinted) {
        rows.push(("error", format!("mutants: {msg}")));
        if let Some(h) = hint {
            rows.push(("", format!("  hint: {h}")));
        }
    }
    // A written mutant no test killed: one the reviewer asked to be caught and none was.
    let survived = (out.iter()).any(|m| m["written"] == true && m["killed_by"].is_null() && m["stopped_by"].is_null());
    let skipped: Vec<_> = (live.iter())
        .map(|(t, g, kind)| json!({"test": t, "grant": g, "holds": kind.0}))
        .collect();
    let mutations = inputs.mutations.as_ref().map(|m| {
        let errors: Vec<_> = errors.iter().map(|(m, h)| json!({"message": m, "hint": h})).collect();
        json!({"path": m.path, "inside_new": m.inside, "entries": m.entries.len(), "errors": errors})
    });
    let value = json!({
        "seed": inputs.seed, "requested": inputs.mutants, "sites": sites.len(), "made": ran,
        "drawn": order.len(), "did_not_check": did_not_check, "error": unfinished.as_ref().map(|u| &u.0), "rows": out,
        "skipped": skipped, "constants": consts.keys().collect::<Vec<_>>(), "mutations": mutations,
    });
    (value, unfinished.is_some() || !errors.is_empty(), survived)
}

/// One mutant's `--json` row: where it came from (`source`, merged in), its site in NEW's file
/// `file`, `((line, col), from, to, description)` as shown, how its runs ended and `(tests run,
/// fuel)`.
fn row(
    source: serde_json::Value,
    s: &Site,
    file: String,
    ((line, col), from, to, description): ((usize, usize), String, String, String),
    end: &End,
    (tests_run, fuel): (usize, u64),
) -> serde_json::Value {
    let how = (end.killed.as_ref().map(|k| k.1.clone()))
        .or(end
            .stopped
            .as_ref()
            .filter(|_| end.killed.is_none())
            .map(|s| s.1.clone()))
        .or(end.discarded.as_ref().map(|_| "discarded".to_string()));
    let mut row = json!({
        "function": s.func, "file": file, "line": line, "col": col,
        "from": from, "to": to, "description": description,
        "killed_by": end.killed.as_ref().map(|k| &k.0),
        "stopped_by": end.stopped.as_ref().filter(|_| end.killed.is_none()).map(|s| &s.0),
        "discarded_by": end.discarded.as_ref().filter(|_| end.killed.is_none() && end.stopped.is_none()),
        "how": how, "tests_run": tests_run, "fuel": fuel,
    });
    if let (Some(r), Some(src)) = (row.as_object_mut(), source.as_object()) {
        r.extend(src.clone());
    }
    row
}

/// Where written entry `w` applies in `prog` (`checked` is it checked, `mains` what its `main`
/// may run): the one place its `-` text occurs in its function, or the message and hint of why
/// it does not apply.
fn place(
    prog: &Program,
    checked: &lang::Program,
    mains: &BTreeSet<String>,
    w: &Written,
) -> Result<Site, (String, String)> {
    let f = ascii(&w.func);
    let spans: BTreeMap<String, lang::Span> = checked.fn_spans().into_iter().collect();
    let in_prog = w.program.as_ref().map_or("NEW".to_string(), |p| ascii(p));
    let Some(span) = spans.get(&w.func) else {
        return Err((
            format!("{in_prog} has no function {f}"),
            "name it as the page does, `flow::finish`; an --entry's own function as `worker.kio: deliver`".into(),
        ));
    };
    let file = span.file as usize;
    let deps: Vec<String> = prog.deps.iter().map(|d| format!("{}.kio", d.name)).collect();
    if deps.contains(&prog.files[file].0) {
        return Err((
            format!("{f} is in a dependency, whose pin a mutant would break"),
            "mutate the program's own functions; a dependency is tested where it is written".into(),
        ));
    }
    // A test, or a helper only tests call: a kill there measures nothing `main` does.
    if !mains.contains(&w.func) {
        return Err((
            format!("{f} is not reached from {in_prog}'s main"),
            "name a function `main` reaches; a test or a helper only tests call is not one".into(),
        ));
    }
    let text = prog.files[file].1.get(span.start..span.end).unwrap_or_default();
    let found: Vec<usize> = text.match_indices(w.from.as_str()).map(|(i, _)| i).collect();
    match found[..] {
        [i] => Ok(Site {
            file,
            start: span.start + i,
            end: span.start + i + w.from.len(),
            to: w.to.clone(),
            func: w.func.clone(),
        }),
        [] => Err((
            format!("the `-` text is not in {f}"),
            format!("the code it pinned has changed: copy the text from {f} in NEW again, or drop the entry"),
        )),
        _ => Err((
            format!("the `-` text occurs {} times in {f}", found.len()),
            "lengthen it until it occurs once; an entry names a function and a snippet, never a line".into(),
        )),
    }
}

/// What runs each mutant: T's tests as set up (`base`), T's own `--fuel` and `--timeout`, the
/// most fuel a test may have, and when the runs began (the budget is shared).
struct Runner<'a> {
    base: &'a Tests,
    fuel_t: Option<u64>,
    fuel_max: u64,
    start: std::time::Instant,
}

/// How one mutant went: it did not check (the first error, shown), or its runs' end, the tests
/// run and the fuel they used.
enum Ran {
    DidNotCheck(String),
    Ran(End, usize, u64),
}

/// Why the mutant runs stopped before the last: the budget ran out, or a run failed inside kio.
enum Stop {
    Budget,
    Internal(String),
}

impl Stop {
    /// The `error` line and its hint, `ran` of `total` mutants having run.
    fn line(&self, ran: usize, total: usize) -> (String, Option<String>) {
        match self {
            Stop::Budget => (
                format!("{ran} of {total} ran; budget exhausted ({BUDGET_SECS} s)"),
                Some("each mutant runs the tests that reach it: lower `--mutants`, write fewer entries, or speed up the slowest test".into()),
            ),
            Stop::Internal(e) => (format!("{ran} of {total} ran; a run failed in kio: {e}"), None),
        }
    }
}

impl Runner<'_> {
    /// `prog` (at `path`) with edit `s`, checked, then run on the tests of `reaching` that reach
    /// its function, examples first, until one fails.
    fn run(
        &self,
        prog: &Program,
        path: &str,
        s: &Site,
        reaching: &[(&TestResult, BTreeSet<String>)],
    ) -> Result<Ran, Stop> {
        let over = || self.start.elapsed().as_secs() >= BUDGET_SECS;
        if over() {
            return Err(Stop::Budget);
        }
        let mut files = prog.files.clone();
        files[s.file].1.replace_range(s.start..s.end, &s.to);
        let program = match kio_host::check_program(&files) {
            Ok(p) => p,
            Err(diags) => {
                let first = diags.first().map_or(String::new(), |d| d.message.clone());
                return Ok(Ran::DidNotCheck(ascii_chars(&cut(&one_line(&first), 120))));
            }
        };
        let base = self.base;
        let mut run = Tests {
            prog: Program {
                files,
                paths: prog.paths.clone(),
                ids: prog.ids.clone(),
                deps: prog.deps.clone(),
            },
            flags: Vec::new(),
            grants_file: base.grants_file.clone(),
            program,
            filter: String::new(),
            path: path.to_string(),
            given: base.given.clone(),
            cases: base.cases,
            seed: base.seed,
            fakes: base.fakes.clone(),
            records: base.records.clone(),
            mutant: true,
            engine: base.engine.clone(),
        };
        // Examples first: the cheapest kill.
        let mut reaching: Vec<&TestResult> = (reaching.iter())
            .filter(|(_, r)| r.contains(&s.func))
            .map(|(t, _)| *t)
            .collect();
        reaching.sort_by_key(|t| (t.prop.is_some(), t.name.as_str()));
        let all = run.program.tests();
        let (mut end, mut tests_run, mut fuel) = (End::default(), 0, 0);
        for t in reaching {
            let Some(tf) = all.iter().find(|x| x.name == t.name) else {
                continue;
            };
            // A mutant cut short is not counted as run.
            if over() {
                return Err(Stop::Budget);
            }
            let ten = t.used.map_or(u64::MAX, |u| u.fuel.saturating_mul(FUEL_FACTOR));
            let cap = ten.clamp(1, self.fuel_max);
            run.flags = base.flags.clone();
            run.flags.extend(["--fuel".to_string(), cap.to_string()]);
            let r = run_test(&run, tf);
            tests_run += 1;
            fuel += r.used.map_or(0, |u| u.fuel);
            let Some(e) = &r.error else { continue };
            let name = t.name.clone();
            // T's own limit, not ten times the test's fuel, stopped it: not a kill.
            let t_limit = match e["kind"].as_str().unwrap_or_default() {
                "discard" => {
                    end.discarded.get_or_insert(name);
                    continue;
                }
                "fuel" if self.fuel_t == Some(cap) && cap < ten => Some(format!("--fuel {cap}")),
                k @ ("timeout" | "memory" | "stack" | "calls" | "io") => {
                    limit(&base.flags, &format!("--{k}")).map(|n| format!("--{k} {n}"))
                }
                _ => None,
            };
            if let Some(l) = t_limit {
                end.stopped.get_or_insert((name, format!("stopped by T's {l}")));
                continue;
            }
            end.killed = Some((name, how(e).map_err(Stop::Internal)?));
            break;
        }
        Ok(Ran::Ran(end, tests_run, fuel))
    }
}

/// The last value of limit `flag` among `flags`.
fn limit(flags: &[String], flag: &str) -> Option<u64> {
    (flags.chunks(2).rev())
        .find(|p| p[0] == flag)
        .and_then(|p| p[1].parse().ok())
}

/// The sites drawn: `n` of them, seeded by `seed`. Each function's sites (`groups`, in key order)
/// are shuffled, the functions too, then taken round-robin, so one long function cannot take
/// every draw. Returned in file order.
fn draw(sites: &[Site], groups: &[(String, usize)], seed: u64, n: usize) -> Vec<usize> {
    let mut rng = lang::prop::Rng::new(seed);
    let mut shuffle = |v: &mut Vec<usize>| {
        for i in (1..v.len()).rev() {
            v.swap(i, (rng.next_u64() % (i as u64 + 1)) as usize);
        }
    };
    let mut each: Vec<Vec<usize>> = (groups.iter())
        .map(|(f, _)| (0..sites.len()).filter(|&i| sites[i].func == *f).collect())
        .collect();
    each.iter_mut().for_each(&mut shuffle);
    let mut turn: Vec<usize> = (0..each.len()).collect();
    shuffle(&mut turn);
    let mut order = Vec::new();
    for round in 0.. {
        let next: Vec<usize> = turn.iter().filter_map(|&g| each[g].get(round).copied()).collect();
        if next.is_empty() || order.len() >= n {
            break;
        }
        order.extend(next.into_iter().take(n - order.len()));
    }
    order.sort();
    order
}

/// What a grant holds that can tell one run from the next, with the flag that takes its place.
type Live = (&'static str, &'static str);
const REAL_HOST: Live = ("a real host", "--http {g}=fake:FILE");
const SYSTEM_CLOCK: Live = ("the system clock", "--clock {g}=fixed:2026-01-01T00:00:00Z");
const OS_RANDOM: Live = ("OS randomness", "--rng {g}=seed:1");

/// Each of `tests` (by name) that T grants a real `Http` (a fake is taken out of the flags), the
/// system clock or OS randomness: `(test, grant, what it holds)`, in test order.
fn live_grants(base: &Tests, tests: &[&str]) -> Vec<(String, String, Live)> {
    let mut out = Vec::new();
    for t in base.program.tests().iter().filter(|t| tests.contains(&t.name.as_str())) {
        for (name, ty) in &t.params {
            let live = (base.flags.chunks(2)).find_map(|p| {
                let (n, v) = p[1].split_once('=').unwrap_or((&p[1], ""));
                match (ty.as_str(), p[0].as_str()) {
                    _ if n != name => None,
                    ("Http", "--http") => Some(REAL_HOST),
                    ("Clock", "--clock") if !v.starts_with("fixed:") => Some(SYSTEM_CLOCK),
                    ("Rng", "--rng") if !v.starts_with("seed:") => Some(OS_RANDOM),
                    _ => None,
                }
            });
            if let Some(live) = live {
                out.push((t.name.clone(), name.clone(), live));
            }
        }
    }
    out
}

/// One line per grant in `live`, naming the tests that hold it.
fn skipped_lines(live: &[(String, String, Live)], rows: &mut Vec<Row>) {
    let mut by: BTreeMap<(&str, Live), Vec<&str>> = BTreeMap::new();
    for (t, g, kind) in live {
        by.entry((g.as_str(), *kind)).or_default().push(t);
    }
    for ((g, (what, flag)), ts) in by {
        let they = if ts.len() == 1 { "it holds" } else { "they hold" };
        rows.push((
            "not checked",
            format!(
                "mutants skipped {}: {they} {what} ({g}), which can tell runs apart; use {}",
                ts.join(", "),
                flag.replace("{g}", g)
            ),
        ));
    }
}

/// The page's lines for the mutants that ran (`rows`, `--json`'s): the generated ones drawn at
/// `seed`, `(drawn, sites, did not check)` when there were sites, and the mutations file's, its
/// path shown and whether it is inside NEW's tree.
fn lines(
    out: &[serde_json::Value],
    seed: u64,
    generated: Option<(usize, usize, usize)>,
    file: Option<&(String, bool)>,
    rows: &mut Vec<Row>,
) {
    let killed: Vec<&serde_json::Value> = out.iter().filter(|m| !m["killed_by"].is_null()).collect();
    let stopped = out.iter().filter(|m| !m["stopped_by"].is_null()).count();
    let stopped = match stopped {
        0 => String::new(),
        n => format!(", {n} stopped by T's limits"),
    };
    let is_generated = |m: &&serde_json::Value| m["written"] != true;
    // Of the mutants from one source: `K of N`.
    let of = |generated: bool| {
        let (k, n) = (
            killed.iter().filter(|m| is_generated(m) == generated).count(),
            out.iter().filter(|m| is_generated(m) == generated).count(),
        );
        format!("{k} of {n}")
    };
    let mut parts = Vec::new();
    if let Some((drawn, sites, bad)) = generated {
        let counts = if file.is_some() {
            format!(" {}", of(true))
        } else {
            String::new()
        };
        parts.push(format!(
            "generated{counts} at seed {seed}: {drawn} drawn of {sites} sites, {bad} did not check"
        ));
    }
    if let Some((path, inside)) = file {
        let inside = if *inside { " (inside NEW's tree)" } else { "" };
        parts.push(format!("{path}{inside} {}", of(false)));
    }
    rows.push((
        "tested",
        format!(
            "mutants: {} of {} killed{stopped} ({})",
            killed.len(),
            out.len(),
            parts.join("; ")
        ),
    ));
    // Per killing test: its kills, and those that were not a failed assertion or `Err`.
    let mut by: BTreeMap<&str, (usize, BTreeMap<&str, usize>)> = BTreeMap::new();
    for m in &killed {
        let e = by.entry(m["killed_by"].as_str().unwrap_or_default()).or_default();
        e.0 += 1;
        let how = m["how"].as_str().unwrap_or_default();
        if how != "failed" {
            *e.1.entry(how).or_default() += 1;
        }
    }
    let mut by: Vec<_> = by.into_iter().collect();
    by.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    let each: Vec<String> = (by.iter())
        .map(|(name, (n, hows))| {
            let hows: Vec<String> = hows.iter().map(|(h, k)| format!("{k} {h}")).collect();
            match hows.is_empty() {
                true => format!("{name} {n}"),
                false => format!("{name} {n} ({})", hows.join(", ")),
            }
        })
        .collect();
    if !each.is_empty() {
        rows.push(("tested", format!("killed by: {}", each.join(", "))));
    }
    let text = |m: &serde_json::Value, k: &str| m[k].as_str().unwrap_or_default().to_string();
    // A written mutant stopped by T's limit is an `error` line of its own.
    let shown = |m: &&serde_json::Value| m["killed_by"].is_null() && (is_generated(m) || m["stopped_by"].is_null());
    for m in out.iter().filter(shown) {
        let (f, d) = (text(m, "function"), text(m, "description"));
        // The edit's place in NEW, so two mutants with one snippet are told apart.
        let at = format!("{}:{}:{}", ascii(&text(m, "file")), m["line"], m["col"]);
        // A written mutant: where it is in the file, its program, its edit, then what it breaks,
        // quoted so it cannot pass for the edit.
        let (source, f, d) = match is_generated(&m) {
            true => (format!("generated: {at}"), f, d),
            false => {
                let f = m["program"].as_str().map_or(f.clone(), |p| format!("{p}: {f}"));
                let n = m["tests_run"].as_u64().unwrap_or_default();
                let tests = if n == 1 { "test" } else { "tests" };
                let (from, to, d) = (text(m, "from"), text(m, "to"), d.replace('"', "\\\""));
                let d = format!("`{from}` -> `{to}`, ran {n} {tests}: \"{d}\"");
                (format!("{}:{}: {at}", text(m, "source"), m["entry"]), f, d)
            }
        };
        let line = match (m["stopped_by"].as_str(), m["discarded_by"].as_str()) {
            (Some(t), _) => format!("{} in {t}: {source}: {f}: {d}", text(m, "how")),
            (_, Some(p)) => format!("survived: {source}: {f}: {d} ({p} discarded every case)"),
            _ => format!("survived: {source}: {f}: {d}"),
        };
        rows.push(("not checked", line));
    }
}

/// The most bytes of a mutations file read.
const FILE_MAX: u64 = 64 << 10;

/// What a mutations file holds, for its errors' hints.
const FORMAT: &str = "an entry is `function: what it breaks`, a `- ` line with text that occurs once in that function, and a `+ ` line with what replaces it (`+` alone removes it); `---` between entries and `#` comments are allowed; an --entry's own function is `worker.kio: deliver: ..`";

/// A mutations file (`--mutations`, R1b2): its path as given, its bytes (hashed in Identity),
/// whether its canonical path is inside NEW's directory, and its entries.
pub(super) struct File {
    pub(super) path: String,
    pub(super) bytes: Vec<u8>,
    pub(super) inside: bool,
    entries: Vec<Written>,
}

/// One entry of a mutations file: its line, the `--entry` it names (`None`: the root), the
/// function, what the mutant breaks, and the `-` text and what replaces it.
struct Written {
    line: usize,
    program: Option<String>,
    func: String,
    description: String,
    from: String,
    to: String,
}

/// The mutations file `path` read once and parsed (`new` is the root's path), or the exit code
/// once the refusal is reported. Read as OLD's grants file is: a regular file by its own name,
/// no symlink as its last component, no other hard link, at most `FILE_MAX` bytes.
pub(super) fn read(path: &str, new: &str) -> Result<File, i32> {
    let at = format!("--mutations {}", ascii(path));
    let not_regular = || {
        fail(format!("{at}: a symlink, a hard link or not a regular file\n  hint: give the file itself; kio reads a mutations file only by its own name"))
    };
    let bytes = match read_regular(path.as_ref(), FILE_MAX, true) {
        Ok(b) => b,
        Err(Unread::TooLarge) => return Err(fail(format!("{at}: over 64 KiB\n  hint: {FORMAT}"))),
        Err(Unread::NotRegular) => return Err(not_regular()),
        Err(Unread::Io(e)) if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
            return Err(not_regular())
        }
        Err(e) => return Err(fail(format!("{at}: {e}"))),
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Err(fail(format!("{at}: not UTF-8\n  hint: {FORMAT}")));
    };
    let entries = parse(&ascii(path), text).map_err(fail)?;
    // Inside by either path: a symlinked directory in NEW's tree still leads from it.
    let dir = Path::new(new).parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or(".".as_ref());
    let canonical = matches!((dir.canonicalize(), Path::new(path).canonicalize()), (Ok(d), Ok(f)) if f.starts_with(&d));
    let lexical = matches!((lexical(dir), lexical(path.as_ref())), (Some(d), Some(f)) if f.starts_with(&d));
    Ok(File {
        path: path.to_string(),
        bytes,
        inside: canonical || lexical,
        entries,
    })
}

/// `p` absolute from the current directory, `.` and `..` resolved by name, no link followed.
fn lexical(p: &Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in std::env::current_dir().ok()?.join(p).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => _ = out.pop(),
            c => out.push(c),
        }
    }
    Some(out)
}

/// An `--entry` name without leading `./`, as a mutations file's header may write it.
fn plain(name: &str) -> &str {
    let mut n = name;
    while let Some(rest) = n.strip_prefix("./") {
        n = rest;
    }
    n
}

/// The entries of mutations file `at` (its path, shown), whose text is `text`, or the error.
fn parse(at: &str, text: &str) -> Result<Vec<Written>, String> {
    let mut out: Vec<Written> = Vec::new();
    // The line the entry being read needs next: 0 its header, 1 its `-`, 2 its `+`.
    let mut part = 0;
    for (i, line) in text.lines().enumerate() {
        let (n, line) = (i + 1, line.trim_end());
        let expected = |what: &str| format!("{at}:{n}: expected {what}\n  hint: {FORMAT}");
        if line.is_empty() || line.starts_with('#') || (part == 0 && line == "---") {
            continue;
        }
        if part == 0 {
            let header = || expected("`function: what it breaks`");
            let Some((first, rest)) = line.split_once(": ") else {
                return Err(header());
            };
            let (program, rest) = match first.ends_with(".kio") {
                true => (Some(first.to_string()), rest),
                false => (None, line),
            };
            let Some((func, description)) = rest.split_once(": ") else {
                return Err(header());
            };
            if func.is_empty() || func.contains(char::is_whitespace) || description.trim().is_empty() {
                return Err(header());
            }
            let (func, description) = (func.to_string(), description.trim().to_string());
            let (from, to) = (String::new(), String::new());
            out.push(Written {
                line: n,
                program,
                func,
                description,
                from,
                to,
            });
            part = 1;
            continue;
        }
        let Some(w) = out.last_mut() else { continue };
        if part == 1 {
            match line.strip_prefix("- ") {
                Some(t) if !t.trim().is_empty() => w.from = t.to_string(),
                _ => return Err(expected("`- text`, the text the mutant replaces")),
            }
            part = 2;
            continue;
        }
        let Some(t) = line.strip_prefix("+ ").or((line == "+").then_some("")) else {
            return Err(expected("`+ text`, what replaces the `-` text"));
        };
        if t == w.from {
            return Err(format!(
                "{at}:{n}: the `+` text is the `-` text, so the mutant changes nothing\n  hint: write the wrong behaviour a test should catch"
            ));
        }
        w.to = t.to_string();
        part = 0;
    }
    match out.last() {
        None => Err(format!("{at}: no entries\n  hint: {FORMAT}")),
        Some(_) if part == 0 => Ok(out),
        Some(w) => Err(format!(
            "{at}:{}: the entry has no `{}` line\n  hint: {FORMAT}",
            w.line,
            if part == 1 { "-" } else { "+" }
        )),
    }
}

/// How a test's error `e` killed a mutant: `failed` (an assertion, an `Err` or a run-time
/// error), or the limit that stopped it. `Err` when kio failed, not the program.
fn how(e: &serde_json::Value) -> Result<String, String> {
    Ok(match e["kind"].as_str().unwrap_or_default() {
        "fuel" => "out of fuel".into(),
        "timeout" => "timeout; machine-dependent".into(),
        k @ ("memory" | "stack" | "calls" | "io") => format!("{k} limit"),
        "internal" => return Err(kio_host::escape_shown(e["message"].as_str().unwrap_or_default())),
        _ => "failed".into(),
    })
}

/// Characters of the site's line shown on each side of it.
const AROUND: usize = 24;

/// The site's text, its 1-based line and column (`lang::line_col`), and the site with up to
/// `AROUND` characters of its line on each side: each one line, cut, and ASCII as the binds line
/// shows a grant.
fn shown_site(text: &str, s: &Site) -> (String, (usize, usize, String)) {
    let (n, col) = lang::line_col(text, s.start);
    let before = &text[text[..s.start].rfind('\n').map_or(0, |i| i + 1)..s.start];
    let after = &text[s.end..text[s.end..].find('\n').map_or(text.len(), |i| s.end + i)];
    let b = before.char_indices().rev().nth(AROUND - 1).map_or(0, |(i, _)| i);
    let a = after.char_indices().nth(AROUND).map_or(after.len(), |(i, _)| i);
    let from = cut(&one_line(&text[s.start..s.end]), 60);
    let mut around = shown_text(&format!("{}{from}{}", &before[b..], &after[..a]));
    if b > 0 {
        around = format!("...{around}");
    }
    if a < after.len() {
        around += "...";
    }
    (ascii_chars(&from), (n, col, around))
}

/// `t` on one line, runs of whitespace one space.
fn one_line(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `t` as the page shows source: one line, ASCII.
fn shown_text(t: &str) -> String {
    ascii_chars(&one_line(t))
}

/// Every edit the operators make in function text `text`, after its signature: `(start, end,
/// replacement)`, byte offsets into `text`. A comparison or logical operator is flipped only
/// where the parser found one (`ops`: the bytes between its operands, and the operator), so
/// `Vec<Int>` is never a site and `LIMIT > n` is.
fn edits(text: &str, ops: &[(usize, usize, &str)]) -> Vec<(usize, usize, String)> {
    let Ok(toks) = lang::lex::lex(text) else {
        return vec![];
    };
    let Some(body) = toks.iter().position(|t| t.kind == Kind::Sym("{")) else {
        return vec![];
    };
    let mut out = Vec::new();
    let sym = |i: usize, s: &'static str| toks.get(i).is_some_and(|t| t.kind == Kind::Sym(s));
    for i in body..toks.len() {
        let t = &toks[i];
        let operator = |s: &str| (ops.iter()).any(|(l, r, op)| *op == s && (*l..*r).contains(&t.span.start));
        let to = match &t.kind {
            Kind::Sym(s) if !operator(s) => None,
            Kind::Sym(s) => [
                ("<", "<="),
                ("<=", "<"),
                (">", ">="),
                (">=", ">"),
                ("==", "!="),
                ("!=", "=="),
                ("&&", "||"),
                ("||", "&&"),
            ]
            .iter()
            .find(|(a, _)| a == s)
            .map(|(_, b)| b.to_string()),
            Kind::Keyword(k) if k == "true" => Some("false".into()),
            Kind::Keyword(k) if k == "false" => Some("true".into()),
            Kind::Int(n) => n.checked_add(1).map(|m| m.to_string()),
            Kind::Str(s) if !s.is_empty() => Some("\"\"".into()),
            Kind::Ident(n) => match n.as_str() {
                "is_ok" => Some("is_err".into()),
                "is_err" => Some("is_ok".into()),
                "is_some" => Some("is_none".into()),
                "is_none" => Some("is_some".into()),
                "Ok" if sym(i + 1, "(") => Some("Err".into()),
                _ => None,
            },
            _ => None,
        };
        if let Some(to) = to {
            out.push((t.span.start, t.span.end, to));
        }
        // `Some(x)` -> `None`.
        if t.kind == Kind::Ident("Some".into()) && sym(i + 1, "(") {
            if let Some(close) = closing(&toks, i + 1) {
                out.push((t.span.start, toks[close].span.end, "None".into()));
            }
        }
    }
    out.extend(dropped(&toks, body));
    out
}

/// The index of the token closing the bracket opened at `open`.
fn closing(toks: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.kind {
            Kind::Sym("(" | "[" | "{") => depth += 1,
            Kind::Sym(")" | "]" | "}") => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Each `;`-terminated statement in a block from `body` on that is an expression, an assignment
/// or `let _x = ..`, as an edit that removes it. A statement starts after the `{`, `;` or `}`
/// before it in its block; one that starts after a block's `}` (`let x = match .. { .. };`)
/// reads as empty and is skipped.
fn dropped(toks: &[Token], body: usize) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    // Each open bracket, and where the statement being read in it starts.
    let mut open: Vec<(&str, usize)> = Vec::new();
    for (i, t) in toks.iter().enumerate().skip(body) {
        match t.kind {
            Kind::Sym(s @ ("(" | "[" | "{")) => open.push((s, i + 1)),
            Kind::Sym(")" | "]") => _ = open.pop(),
            Kind::Sym("}") => {
                open.pop();
                if let Some(top) = open.last_mut() {
                    top.1 = i + 1;
                }
            }
            Kind::Sym(";") => {
                let Some((sym, start)) = open.last_mut() else { continue };
                let from = std::mem::replace(start, i + 1);
                let statement = match &toks[from..i] {
                    [Token {
                        kind: Kind::Ident(_), ..
                    }, ..] => true,
                    [Token {
                        kind: Kind::Keyword(k), ..
                    }, Token {
                        kind: Kind::Ident(n), ..
                    }, ..] => k == "let" && n.starts_with('_'),
                    _ => false,
                };
                if *sym == "{" && statement {
                    out.push((toks[from].span.start, t.span.end, String::new()));
                }
            }
            _ => {}
        }
    }
    out
}
