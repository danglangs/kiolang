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

/// `lang::report_diff` of two programs, each `(name, source)` files with the root first.
fn diff(old: &[(&str, &str)], new: &[(&str, &str)]) -> Vec<String> {
    let caps = common::caps();
    let p = |f: &[(&str, &str)]| lang::check_files(f, &caps).expect("program checks");
    lang::report_diff(&p(old), &p(new))
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
    assert_eq!(diff(&[("a.kio", &old())], &[("a.kio", &old())]), Vec::<String>::new());
}

#[test]
fn diff_added_method() {
    let new = old().replace(
        r#"links.delete("k")?;"#,
        r#"links.delete("k")?; let _o = links.get("k")?;"#,
    );
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        ["+ delete: links get", "+ main: links get"]
    );
}

#[test]
fn diff_removed_method() {
    let new = old().replace(r#"let _w = log.write("gone"); "#, "");
    assert_eq!(diff(&[("a.kio", &old())], &[("a.kio", &new)]), ["- delete: log write"]);
}

#[test]
fn diff_new_main_parameter() {
    // The new grant comes first; `main` then reaches `hook send` through `notify`, a new function.
    let new = old()
        .replace(
            "fn main(links: Kv, log: Log)",
            "fn main(links: Kv, log: Log, hook: Http)",
        )
        .replace("create(links, log)?;", "create(links, log)?; notify(hook);")
        + r#"fn notify(hook: Http) { let _r = hook.send("POST", "/", [], "{}".to_bytes()); }"#;
    let want = ["+ main: grant hook: Http", "+ main: hook send", "+ notify: hook send"];
    assert_eq!(diff(&[("a.kio", &old())], &[("a.kio", &new)]), want);
}

#[test]
fn diff_new_main_parameter_unused() {
    // A grant `main` asks for is authority even when nothing calls it yet.
    let new = old().replace("fn main(links: Kv, log: Log)", "fn main(links: Kv, log: Log, out: Dir)");
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        ["+ main: grant out: Dir"]
    );
}

#[test]
fn diff_grant_type_changed() {
    let (old, new) = ("fn main(d: Dir) -> Int { 1 }", "fn main(d: Kv) -> Int { 1 }");
    assert_eq!(
        diff(&[("a.kio", old)], &[("a.kio", new)]),
        ["+ main: grant d: Kv", "- main: grant d: Dir"]
    );
}

#[test]
fn diff_new_function_counts_in_full() {
    // `log write` and `links keys` both count, though `main` could already write the log.
    let new = old() + r#"fn audit(log: Log, links: Kv) -> Int { let _w = log.write("x"); let _k = links.keys(""); 0 }"#;
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &new)]),
        ["+ audit: log write, links keys"]
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
    let want = [
        "- create: links set, log write",
        "+ store::create: links set, log write",
    ];
    assert_eq!(
        diff(&[("a.kio", &old())], &[("a.kio", &root), ("store.kio", &store)]),
        want
    );
}
