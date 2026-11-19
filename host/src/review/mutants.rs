//! R1b1: generated mutants. Each is NEW's text with one token-level edit in a changed function
//! that a `main` (the root's or an entry's) and a test reach, checked, then run on the tests that
//! reach it through `kio test`'s path (fresh sandboxes, the test grants only), stopping at the
//! first failure.
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
/// as `base` and their unmutated `results`. `--json`'s value, and whether the runs did not finish
/// (an `error`: the budget ran out, or a run failed in kio, not in the program).
pub(super) fn run(
    inputs: &Inputs,
    changes: &Changes,
    base: &Tests,
    results: &[TestResult],
    rows: &mut Vec<Row>,
) -> (serde_json::Value, bool) {
    let (Some(root), true) = (&changes.new, inputs.mutants > 0) else {
        return (json!(null), false);
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
    // The tests that passed on NEW and may run a mutant, and the functions each may run.
    let passed: Vec<(&TestResult, BTreeSet<String>)> = (results.iter().filter(ok))
        .filter(|r| !live.iter().any(|(t, _, _)| *t == r.name))
        .map(|r| (r, lang::reachable(root, [r.name.as_str()])))
        .collect();
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
    let mut sites = Vec::new();
    // Each function with sites: its key and how many.
    let mut groups: Vec<(String, usize)> = Vec::new();
    for f in funcs {
        let Some(span) = spans.get(&f) else { continue };
        let file = span.file as usize;
        let reached = passed.iter().any(|(_, r)| r.contains(&f));
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
    let timeout_t = limit(&base.flags, "--timeout");
    let fuel_max = fuel_t.unwrap_or(Limits::default().fuel);
    let start = std::time::Instant::now();
    let over = || start.elapsed().as_secs() >= BUDGET_SECS;
    let exhausted = |ran: usize| format!("{ran} of {} ran; budget exhausted ({BUDGET_SECS} s)", order.len());
    let (mut did_not_check, mut ran, mut unfinished) = (0, 0, None);
    let mut out: Vec<serde_json::Value> = Vec::new();
    'mutants: for &i in &order {
        if over() {
            unfinished = Some(exhausted(ran));
            break;
        }
        let s = &sites[i];
        let mut files = new.files.clone();
        files[s.file].1.replace_range(s.start..s.end, &s.to);
        let Ok(program) = kio_host::check_program(&files) else {
            did_not_check += 1;
            continue;
        };
        ran += 1;
        let mut run = Tests {
            prog: Program {
                files,
                paths: new.paths.clone(),
                ids: new.ids.clone(),
                deps: new.deps.clone(),
            },
            flags: Vec::new(),
            grants_file: base.grants_file.clone(),
            program,
            filter: String::new(),
            path: base.path.clone(),
            cases: base.cases,
            seed: base.seed,
            fakes: base.fakes.clone(),
            records: base.records.clone(),
            mutant: true,
        };
        // Examples first: the cheapest kill.
        let mut reaching: Vec<&TestResult> = (passed.iter())
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
                ran -= 1;
                unfinished = Some(exhausted(ran));
                break 'mutants;
            }
            let ten = t.used.map_or(u64::MAX, |u| u.fuel.saturating_mul(FUEL_FACTOR));
            let cap = ten.clamp(1, fuel_max);
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
                "fuel" if fuel_t == Some(cap) && cap < ten => Some(format!("--fuel {cap}")),
                "timeout" => timeout_t.map(|n| format!("--timeout {n}")),
                _ => None,
            };
            if let Some(l) = t_limit {
                end.stopped.get_or_insert((name, format!("stopped by T's {l}")));
                continue;
            }
            match how(e) {
                Ok(how) => end.killed = Some((name, how)),
                Err(e) => {
                    ran -= 1;
                    unfinished = Some(format!("{ran} of {} ran; a run failed in kio: {e}", order.len()));
                    break 'mutants;
                }
            }
            break;
        }
        let (from, line) = shown_site(&new.files[s.file].1, s);
        let to = shown_text(&s.to);
        let description = match to.is_empty() {
            true => format!("`{from}` dropped in `{}`", line.1),
            false => format!("`{from}` -> `{to}` in `{}`", line.1),
        };
        let how = (end.killed.as_ref().map(|k| k.1.clone()))
            .or(end
                .stopped
                .as_ref()
                .filter(|_| end.killed.is_none())
                .map(|s| s.1.clone()))
            .or(end.discarded.as_ref().map(|_| "discarded".to_string()));
        out.push(json!({
            "source": "generated", "function": s.func, "file": json_path(new, s.file), "line": line.0,
            "from": from, "to": to, "description": description,
            "killed_by": end.killed.as_ref().map(|k| &k.0),
            "stopped_by": end.stopped.as_ref().filter(|_| end.killed.is_none()).map(|s| &s.0),
            "discarded_by": end.discarded.as_ref().filter(|_| end.killed.is_none() && end.stopped.is_none()),
            "how": how, "tests_run": tests_run, "fuel": fuel,
        }));
    }
    let drawn: BTreeSet<&str> = order.iter().map(|&i| sites[i].func.as_str()).collect();
    if !sites.is_empty() {
        lines(&out, inputs.seed, (order.len(), sites.len(), did_not_check), rows);
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
    if let Some(e) = &unfinished {
        rows.push(("error", format!("mutants: {e}")));
    }
    let skipped: Vec<_> = (live.iter())
        .map(|(t, g, kind)| json!({"test": t, "grant": g, "holds": kind.0}))
        .collect();
    let value = json!({
        "seed": inputs.seed, "requested": inputs.mutants, "sites": sites.len(), "made": ran,
        "drawn": order.len(), "did_not_check": did_not_check, "error": unfinished, "rows": out,
        "skipped": skipped, "constants": consts.keys().collect::<Vec<_>>(),
    });
    (value, unfinished.is_some())
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

/// The page's lines for the mutants that ran (`rows`, `--json`'s), drawn at `seed`: `(drawn,
/// sites, did not check)`.
fn lines(out: &[serde_json::Value], seed: u64, (drawn, sites, bad): (usize, usize, usize), rows: &mut Vec<Row>) {
    let killed: Vec<&serde_json::Value> = out.iter().filter(|m| !m["killed_by"].is_null()).collect();
    let stopped = out.iter().filter(|m| !m["stopped_by"].is_null()).count();
    let stopped = match stopped {
        0 => String::new(),
        n => format!(", {n} stopped by T's limits"),
    };
    rows.push((
        "tested",
        format!(
            "mutants: {} of {} killed{stopped} (generated at seed {seed}: {drawn} drawn of {sites} sites, {bad} did not check)",
            killed.len(),
            out.len()
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
    for m in out.iter().filter(|m| m["killed_by"].is_null()) {
        let (f, d) = (text(m, "function"), text(m, "description"));
        let line = match (m["stopped_by"].as_str(), m["discarded_by"].as_str()) {
            (Some(t), _) => format!("{} in {t}: generated: {f}: {d}", text(m, "how")),
            (_, Some(p)) => format!("survived: generated: {f}: {d} ({p} discarded every case)"),
            _ => format!("survived: generated: {f}: {d}"),
        };
        rows.push(("not checked", line));
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

/// The site's text, its line number, and the site with up to `AROUND` characters of its line on
/// each side: each one line, cut, and ASCII as the binds line shows a grant.
fn shown_site(text: &str, s: &Site) -> (String, (usize, String)) {
    let n = text[..s.start].matches('\n').count() + 1;
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
    (ascii_chars(&from), (n, around))
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
