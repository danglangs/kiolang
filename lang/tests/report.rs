use lang::{check, report};

mod common;

fn rep(src: &str) -> String {
    report(&check(src, &common::caps()).expect("program checks"))
}

const LOAD: &str = r#"fn load(d: Dir) -> Result<Bytes, IoErr> { d.read("a") }"#;
const SAVE: &str = r#"fn save(o: Dir, b: Bytes) -> Result<(), IoErr> { o.create("b", b) }"#;

#[test]
fn spec_example() {
    let src = format!(
        "{LOAD}\n{SAVE}
fn main(input: Dir, out: Dir) -> Result<(), IoErr> {{ let b = load(input)?; save(out, b) }}
fn pure(n: Int) -> Int {{ n }}"
    );
    let want = "load : d read\nmain : input read, out create\npure : none\nsave : o create";
    assert_eq!(rep(&src), want);
}

#[test]
fn readme_example() {
    // The README shows this file and this exact report.
    let src = include_str!("../../examples/summarize.kio");
    assert_eq!(rep(src), "main      : input read, out create\nsummarize : none");
}

#[test]
fn readme_fx_reports() {
    // The README's pitch and catch show these files and these exact reports.
    let report_kio = include_str!("../../examples/fx/report.kio");
    let leak_kio = include_str!("../../examples/fx/leak.kio");
    let main = "main      : input read, fx get, out write";
    assert_eq!(rep(report_kio), format!("{main}\nsummarize : none"));
    assert_eq!(rep(leak_kio), format!("{main}\nsummarize : fx get"));
}

#[test]
fn two_level_helper_chain() {
    let src = format!(
        "{LOAD}
fn mid(x: Dir) -> Result<Bytes, IoErr> {{ load(x) }}
fn main(input: Dir) -> Result<(), IoErr> {{ let _b = mid(input)?; Ok(()) }}"
    );
    assert_eq!(rep(&src), "load : d read\nmain : input read\nmid  : x read");
}

/// A generic function is listed once: its capability parameters have concrete types, so its
/// authority is the same for every instance. A turbofish call is charged as any call.
#[test]
fn generic_function_listed_once() {
    let src = format!(
        "{LOAD} {SAVE}
fn via<T>(d: Dir, x: T) -> T {{ let _b = load(d); x }}
fn put<T>(o: Dir, x: T) -> T {{ let _r = save(o, \"\".to_bytes()); x }}
fn main(input: Dir, out: Dir) -> Int {{ via(input, 1) + via(input, \"a\").len() + put::<Int>(out, 2) }}"
    );
    let want = "load : d read\nmain : input read, out create\nput  : o create\nsave : o create\nvia  : d read";
    assert_eq!(rep(&src), want);
}

#[test]
fn mutual_recursion_gives_both_the_union() {
    let src = r#"
fn ping(a: Dir, n: Int) -> Result<(), IoErr> {
    let _b = a.read("r")?;
    if n > 0 { pong(a, n - 1) } else { Ok(()) }
}
fn pong(b: Dir, n: Int) -> Result<(), IoErr> {
    let x = b.read("r")?;
    let _ = ping(b, n);
    b.create("w", x)
}
fn main(d: Dir) -> Result<(), IoErr> { ping(d, 3) }"#;
    assert_eq!(
        rep(src),
        "main : d create read\nping : a create read\npong : b create read"
    );
}

#[test]
fn unused_capability_is_omitted() {
    let src = r#"
fn f(a: Dir, b: Dir) -> Result<Bytes, IoErr> { b.read("x") }
fn g(a: Dir) -> Int { 1 }
fn main(x: Dir, y: Dir) -> Result<(), IoErr> { let _b = f(x, y)?; Ok(()) }"#;
    assert_eq!(rep(src), "f    : b read\ng    : none\nmain : y read");
}

#[test]
fn untaken_branch_is_reported() {
    let src = r#"
fn main(d: Dir) -> Result<(), IoErr> {
    if false { d.create("x", "y".to_bytes()) } else { Ok(()) }
}"#;
    assert_eq!(rep(src), "main : d create");
}

#[test]
fn for_loops_are_walked() {
    let src = format!(
        "{LOAD}\nfn main(d: Dir) {{ for x in [1] {{ let _b = load(d); }} for i in 0..1 {{ let _c = d.read(\"a\"); }} }}"
    );
    assert_eq!(rep(&src), "load : d read\nmain : d read");
}

#[test]
fn dir_list_write_remove_exists_check_and_report() {
    let src = r#"
fn main(d: Dir) -> Result<Bool, IoErr> {
    let names: List<Str> = d.list("")?;
    d.write("a", "x".to_bytes())?;
    d.remove("a")?;
    let e: Bool = d.exists("a")?;
    Ok(e && names.len() == 0)
}"#;
    assert_eq!(rep(src), "main : d exists list remove write");
}

#[test]
fn function_values_carry_no_authority() {
    // `apply` calls a value of `fn` type, which cannot take a capability, so only `d` is charged.
    let src = format!(
        "{LOAD}
fn size(b: Bytes) -> Int {{ b.len() }}
fn apply(f: fn(Bytes) -> Int, d: Dir) -> Int {{ match load(d) {{ Ok(b) => f(b), Err(_) => 0 }} }}
fn main(input: Dir) -> Int {{ let load = size; apply(load, input) }}"
    );
    assert_eq!(
        rep(&src),
        "apply : d read\nload  : d read\nmain  : input read\nsize  : none"
    );
}

#[test]
fn lifted_closures_are_not_listed() {
    // A closure reaches no capability; its body counts as part of the function it is written in.
    let src = format!(
        "{LOAD}
fn main(input: Dir) -> Int {{ let n = 1; let f = |b: Bytes| b.len() + n; match load(input) {{ Ok(b) => f(b), Err(_) => 0 }} }}"
    );
    assert_eq!(rep(&src), "load : d read\nmain : input read");
}

#[test]
fn a_shadowed_capability_is_not_charged_with_other_methods() {
    let src = "fn main(d: Dir) -> Str { let d = 5; let f = || d.to_str(); f() }";
    let p = check(src, &lang::CapTable::v0()).unwrap();
    assert_eq!(report(&p).trim(), "main : none");
}

/// A local `Map` named like the `Http` parameter, whose `get` shares a name with `Http.get`.
const SHADOW: &str = r#"let api: Map<Str, Int> = Map::new(); let _v = api.get("x");"#;

#[test]
fn a_shadowing_local_is_not_charged() {
    let src = format!("fn main(api: Http) {{ {SHADOW} }}");
    assert_eq!(rep(&src), "main : none");
}

#[test]
fn shadowing_follows_scopes() {
    let m = r#"let m: Map<Str, Int> = Map::new();"#;
    let real = r#"let _r = api.get("/x");"#;
    let none = [
        // Nested blocks.
        format!(r#"{{ {SHADOW} {{ let _w = api.get("y"); {{ let _z = api.get("z"); }} }} }}"#),
        // A closure parameter.
        format!(r#"{m} let f = |api: Map<Str, Int>| api.get("x"); let _v = f(m);"#),
        // A match arm binding.
        format!(r#"{m} let _v = match Some(m) {{ Some(api) => api.get("x"), None => None }};"#),
        // An if-let binding.
        format!(r#"{m} if let Some(api) = Some(m) {{ let _v = api.get("x"); }}"#),
        // A for variable.
        format!(r#"{m} for api in [m] {{ let _v = api.get("x"); }}"#),
    ];
    for body in none {
        assert_eq!(
            rep(&format!("fn main(api: Http) {{ {body} }}")),
            "main : none",
            "{body}"
        );
    }
    // The real call after the shadow's block ends is charged.
    let after = format!("fn main(api: Http) {{ {{ {SHADOW} }} {real} }}");
    assert_eq!(rep(&after), "main : api get");
    let after = format!("fn main(api: Http) {{ for api in [1] {{ let _s = api.to_str(); }} {real} }}");
    assert_eq!(rep(&after), "main : api get");
}

#[test]
fn a_module_function_parameter_of_the_same_name_is_not_charged() {
    let root = r#"mod web;
fn main(api: Http) { let m: Map<Str, Int> = Map::new(); let _v = web::look(m); let _r = api.get("/x"); }"#;
    let web = r#"pub fn look(api: Map<Str, Int>) -> Option<Int> { api.get("x") }"#;
    let caps = common::caps();
    let p = lang::check_files(&[("a.kio", root), ("web.kio", web)], &caps).unwrap();
    assert_eq!(report(&p), "main      : api get\nweb::look : none");
}

/// `lang::report_diff` of two programs, each `(name, source)` files with the root first.
fn diff(old: &[(&str, &str)], new: &[(&str, &str)]) -> lang::AuthorityDiff {
    let caps = common::caps();
    let p = |f: &[(&str, &str)]| lang::check_files(f, &caps).expect("program checks");
    lang::report_diff(&p(old), &p(new))
}

/// The diff with these grant and function lines.
fn want(grants: &[&str], funcs: &[&str], added: bool) -> lang::AuthorityDiff {
    let own = |ls: &[&str]| ls.iter().map(|l| l.to_string()).collect();
    // No line here carries a covered gain, so a grant line adds when it shows a `+`.
    let grant_added = grants.iter().any(|l| l.starts_with('+') || l.contains("(+"));
    lang::AuthorityDiff {
        grants: own(grants),
        funcs: own(funcs),
        added,
        grant_added,
    }
}

#[test]
fn diff_a_real_call_after_a_shadowing_baseline() {
    // The shadowing baseline charges nothing, so the real call is new authority.
    let old = format!("fn main(api: Http) {{ {SHADOW} }}");
    let new = format!(r#"fn main(api: Http) {{ {{ {SHADOW} }} let _r = api.get("/x"); }}"#);
    assert_eq!(
        diff(&[("a.kio", &old)], &[("a.kio", &new)]),
        want(&["~ api: Http (+get)"], &["+ main: api get"], true)
    );
}

const CREATE: &str = r#"fn create(links: Kv, log: Log) -> Result<(), KvErr> { links.set("k", "v".to_bytes())?; let _w = log.write("made"); Ok(()) }"#;
const DIFF_OLD: &str = r#"
fn delete(links: Kv, log: Log) -> Result<(), KvErr> { links.delete("k")?; let _w = log.write("gone"); Ok(()) }
fn main(links: Kv, log: Log) -> Result<(), KvErr> { create(links, log)?; delete(links, log) }
"#;

fn old() -> String {
    format!("{CREATE}{DIFF_OLD}")
}

#[test]
fn diff_no_change() {
    assert_eq!(diff(&[("a.kio", &old())], &[("a.kio", &old())]), want(&[], &[], false));
}

#[test]
fn diff_added_method() {
    let new = old().replace(
        r#"links.delete("k")?;"#,
        r#"links.delete("k")?; let _o = links.get("k")?;"#,
    );
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        want(
            &["~ links: Kv (+get)"],
            &["+ delete: links get", "+ main: links get"],
            true
        )
    );
}

#[test]
fn diff_removed_method_main_still_reaches() {
    // `main` still writes the log through `create`: no grant changed.
    let new = old().replace(r#"let _w = log.write("gone"); "#, "");
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        want(&[], &["- delete: log write"], false)
    );
}

#[test]
fn diff_method_gained_and_lost_on_one_grant() {
    let new = old()
        .replace(r#"links.delete("k")?;"#, r#"let _k = links.keys("")?;"#)
        .replace(r#"links.set("k", "v".to_bytes())?;"#, "");
    let got = diff(&[("a.kio", &old())], &[("a.kio", &new)]);
    assert_eq!(got.grants, ["~ links: Kv (+keys -delete -set)"]);
    assert!(got.added);
}

#[test]
fn diff_new_main_parameter() {
    // The new grant with what `main` reaches on it; `notify` is a new function.
    let new = old()
        .replace(
            "fn main(links: Kv, log: Log)",
            "fn main(links: Kv, log: Log, hook: Http)",
        )
        .replace("create(links, log)?;", "create(links, log)?; notify(hook);")
        + r#"fn notify(hook: Http) { let _r = hook.send("POST", "/", [], "{}".to_bytes(), 0); }"#;
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        want(
            &["+ hook: Http (send)"],
            &["+ main: hook send", "+ notify: hook send"],
            true
        )
    );
}

#[test]
fn diff_new_main_parameter_unused() {
    // A grant `main` asks for is authority even when nothing calls it yet.
    let new = old().replace("fn main(links: Kv, log: Log)", "fn main(links: Kv, log: Log, out: Dir)");
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        want(&["+ out: Dir"], &[], true)
    );
}

#[test]
fn diff_removed_main_parameter() {
    let new = old()
        .replace("fn main(links: Kv, log: Log)", "fn main(links: Kv)")
        .replace("create(links, log)?; delete(links, log)", r#"links.delete("k")"#);
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        want(
            &["~ links: Kv (-set)", "- log: Log (write)"],
            &["- main: links set, log write"],
            false
        )
    );
}

#[test]
fn diff_grant_type_changed() {
    let (old, new) = ("fn main(d: Dir) -> Int { 1 }", "fn main(d: Kv) -> Int { 1 }");
    assert_eq!(
        diff(&[("a.kio", old)], &[("a.kio", new)]),
        want(&["+ d: Kv", "- d: Dir"], &[], true)
    );
}

#[test]
fn diff_new_function_counts_in_full() {
    // `log write` and `links keys` both count, though `main` could already write the log. `main`
    // does not call `audit`, so no grant line changes.
    let new = old() + r#"fn audit(log: Log, links: Kv) -> Int { let _w = log.write("x"); let _k = links.keys(""); 0 }"#;
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        want(&[], &["+ audit: log write, links keys"], true)
    );
}

#[test]
fn diff_function_moved_to_module() {
    // `create` and `store::create` are different functions: one removed and one added, each in
    // full. `main`'s own authority is unchanged.
    let root = format!(
        "mod store;\n{}",
        DIFF_OLD.replace("create(links, log)?;", "store::create(links, log)?;")
    );
    let store = format!("pub {CREATE}");
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &root), ("store.kio", &store)]),
        want(
            &[],
            &[
                "- create: links set, log write",
                "+ store::create: links set, log write"
            ],
            true
        )
    );
}

/// kio-web payhook's step 2: a new `Hmac` grant reaching `verify` through two helpers, and a
/// `Clock`. Two grant lines, though five functions changed.
#[test]
fn diff_payhook_step_2_is_two_grant_lines() {
    let old = r#"
fn handle(body: Str) -> Int { body.len() }
fn receive(body: Str) -> Int { handle(body) }
fn main(log: Log) { let _w = log.write(receive("x").to_str()); }
"#;
    let new = r#"
fn check_signature(stripe: Hmac, body: Str, mac: Bytes) -> Bool { stripe.verify(body.to_bytes(), mac) }
fn handle(stripe: Hmac, now: Clock, body: Str) -> Int {
    if now.now_ms() < 0 { return 0; }
    if check_signature(stripe, body, "".to_bytes()) { body.len() } else { 0 }
}
fn receive(stripe: Hmac, now: Clock, body: Str) -> Int { handle(stripe, now, body) }
fn main(log: Log, stripe: Hmac, now: Clock) { let _w = log.write(receive(stripe, now, "x").to_str()); }
"#;
    let got = diff(&[("a.kio", old)], &[("a.kio", new)]);
    assert_eq!(got.grants, ["+ stripe: Hmac (verify)", "+ now: Clock (now_ms)"]);
    assert_eq!(
        got.funcs,
        [
            "+ check_signature: stripe verify",
            "+ handle: stripe verify, now now_ms",
            "+ main: stripe verify, now now_ms",
            "+ receive: stripe verify, now now_ms",
        ]
    );
}

/// A call of each capability method on `x`, for the coverage tests (P6).
fn call(ty: &str, m: &str) -> String {
    let args = match (ty, m) {
        ("Dir", "create" | "write") => r#""a", "x".to_bytes()"#,
        ("Dir", "list") => r#""""#,
        ("Dir", _) => r#""a""#,
        ("Http", "get") => r#""/""#,
        ("Http", "send") => r#""POST", "/", [], "x".to_bytes(), 0"#,
        ("Http", "send_all") => "[]",
        ("Kv", "set") => r#""k", "v".to_bytes()"#,
        ("Kv", "swap") => r#""k", None, None"#,
        ("Kv", "scan") => r#""", None, 10"#,
        ("Kv", "add") => r#""k", 1"#,
        ("Kv", "keys") => r#""""#,
        ("Kv", _) => r#""k""#,
        ("Db", "transaction") => "[], []",
        ("Db", _) => r#""select 1", []"#,
        ("Hmac", "sign") => r#""m".to_bytes()"#,
        ("Hmac", "verify") => r#""m".to_bytes(), "m".to_bytes()"#,
        ("WsSend", "text") => r#""hi""#,
        ("WsSend", "binary") => r#""hi".to_bytes()"#,
        ("WsSend", "close") => "1000",
        _ => panic!("no call for {ty}.{m}"),
    };
    format!("let _r = x.{m}({args});")
}

fn main_calling(ty: &str, m: &str) -> String {
    format!("fn main(x: {ty}) {{ {} }}", call(ty, m))
}

fn covers(ty: &str, by: &str, m: &str) -> bool {
    lang::COVERS
        .iter()
        .any(|(t, n, b)| *t == ty && *n == m && b.contains(&by))
}

#[test]
fn diff_covered_gain_adds_nothing_and_covering_gain_does() {
    for (ty, m, by) in lang::COVERS {
        for c in *by {
            // Swapping `c` for `m`, which `c` covers: shown, not added.
            let got = diff(&[("a.kio", &main_calling(ty, c))], &[("a.kio", &main_calling(ty, m))]);
            let line = format!("~ x: {ty} (+{m} -{c}); {m} is covered by {c}");
            assert_eq!(got.grants, [line.as_str()], "{ty}.{m} by {c}");
            assert_eq!(
                got.funcs,
                [
                    format!("+ main: x {m}; {m} is covered by {c}"),
                    format!("- main: x {c}")
                ]
            );
            assert!(!got.added && !got.grant_added, "{line}");
            // Swapping `m` for `c`: added unless `m` covers `c` too.
            let got = diff(&[("a.kio", &main_calling(ty, m))], &[("a.kio", &main_calling(ty, c))]);
            let back = covers(ty, m, c);
            assert_eq!((got.added, got.grant_added), (!back, !back), "{ty}.{c} after {m}");
        }
    }
}

#[test]
fn diff_mixed_line_adds_the_uncovered_gain() {
    let old = r#"fn main(out: Dir) { let _r = out.write("a", "x".to_bytes()); }"#;
    let new = r#"fn main(out: Dir) { let _r = out.create("a", "x".to_bytes()); let _s = out.remove("b"); }"#;
    let got = diff(&[("a.kio", old)], &[("a.kio", new)]);
    assert_eq!(
        got.grants,
        ["~ out: Dir (+create +remove -write); create is covered by write"]
    );
    assert_eq!(
        got.funcs,
        [
            "+ main: out create remove; create is covered by write",
            "- main: out write"
        ]
    );
    assert!(got.added && got.grant_added);
}

#[test]
fn diff_covered_gain_beside_the_covering_method() {
    // The base still reaches `read` and now also `exists`: covered, nothing lost.
    let old = r#"fn main(d: Dir) { let _r = d.read("a"); }"#;
    let new = r#"fn main(d: Dir) { let _r = d.read("a"); let _e = d.exists("a"); }"#;
    let got = diff(&[("a.kio", old)], &[("a.kio", new)]);
    assert_eq!(got.grants, ["~ d: Dir (+exists); exists is covered by read"]);
    assert!(!got.added);
}

#[test]
fn diff_function_lines_use_that_functions_base_methods() {
    // `save` swaps `write` for `create`: covered. `log_it` gains `create` on a parameter whose
    // base only read: added, though `main` (through `save`) reached `write`.
    let old = r#"
fn save(o: Dir) { let _r = o.write("a", "x".to_bytes()); }
fn log_it(o: Dir) { let _r = o.read("a"); }
fn main(out: Dir) { save(out); log_it(out); }
"#;
    let new = old
        .replace(r#"o.write("a", "x".to_bytes())"#, r#"o.create("a", "x".to_bytes())"#)
        .replace(
            r#"o.read("a")"#,
            r#"o.read("a"); let _c = o.create("b", "y".to_bytes())"#,
        );
    let got = diff(&[("a.kio", old)], &[("a.kio", &new)]);
    assert_eq!(got.grants, ["~ out: Dir (+create -write); create is covered by write"]);
    assert_eq!(
        got.funcs,
        [
            "+ log_it: o create",
            "+ main: out create; create is covered by write",
            "- main: out write",
            "+ save: o create; create is covered by write",
            "- save: o write",
        ]
    );
    assert!(got.added && !got.grant_added);
}

#[test]
fn diff_function_line_renamed_parameter_is_not_covered() {
    // Parameters match by name: `y` is new to `f`, so nothing in the base covers its gain.
    let old = r#"fn f(x: Dir) { let _r = x.write("a", "x".to_bytes()); } fn main(d: Dir) { f(d); }"#;
    let new = r#"fn f(y: Dir) { let _r = y.create("a", "x".to_bytes()); } fn main(d: Dir) { f(d); }"#;
    let got = diff(&[("a.kio", old)], &[("a.kio", new)]);
    assert_eq!(got.funcs[0], "+ f: y create");
    assert!(got.added && !got.grant_added);
}

#[test]
fn diff_new_grant_with_covered_methods_is_added() {
    let got = diff(
        &[("a.kio", "fn main() -> Int { 1 }")],
        &[("a.kio", &main_calling("Dir", "create"))],
    );
    assert_eq!(got.grants, ["+ x: Dir (create)"]);
    assert!(got.added && got.grant_added);
}

#[test]
fn diff_review_rows_that_do_not_cover() {
    // `close` sends a code, not a payload; `list` misses symlinks and fails on large directories.
    for (ty, from, to) in [
        ("WsSend", "close", "text"),
        ("WsSend", "close", "binary"),
        ("Dir", "list", "exists"),
    ] {
        let got = diff(
            &[("a.kio", &main_calling(ty, from))],
            &[("a.kio", &main_calling(ty, to))],
        );
        assert_eq!(got.grants, [format!("~ x: {ty} (+{to} -{from})")]);
        assert!(got.added, "{ty}.{to} after {from}");
    }
}

/// Every caps.wit method is classified in `COVERS`, once, and the table names only real
/// methods: a new method must be placed before the diff can treat it.
#[test]
fn covers_lists_every_caps_method() {
    let caps = common::caps();
    let real: Vec<(String, String)> = caps
        .caps
        .iter()
        .flat_map(|c| c.methods.iter().map(move |(m, _, _)| (c.name.clone(), m.clone())))
        .collect();
    for (ty, m) in &real {
        let n = lang::COVERS.iter().filter(|(t, n, _)| t == ty && n == m).count();
        assert_eq!(
            n, 1,
            "{ty}.{m} must appear once in lang::COVERS (covering or standalone)"
        );
    }
    for (ty, m, by) in lang::COVERS {
        assert!(!by.contains(m), "{ty}.{m} covers itself");
        for x in std::iter::once(m).chain(by.iter()) {
            assert!(
                real.contains(&(ty.to_string(), x.to_string())),
                "{ty}.{x} is not a caps method"
            );
        }
    }
}
