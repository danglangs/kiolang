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
fn two_level_helper_chain() {
    let src = format!(
        "{LOAD}
fn mid(x: Dir) -> Result<Bytes, IoErr> {{ load(x) }}
fn main(input: Dir) -> Result<(), IoErr> {{ let _b = mid(input)?; Ok(()) }}"
    );
    assert_eq!(rep(&src), "load : d read\nmain : input read\nmid  : x read");
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
