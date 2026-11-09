//! L7a adversarial review: attacks on rule 1 and the report through generic functions, each
//! of which must keep failing, and generic programs the interpreter and compiler must agree on
//! (the differential harness picks these literals up).
mod common;

use lang::{check, check_files, report, run, CapTable, Caps, Value};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }
    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

fn shown(src: &str) -> String {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || p.display(&run(&p, &mut NoCaps).unwrap_or_else(|e| panic!("{}", e.msg))))
}

/// The first diagnostic's message and the source it points at.
fn rejected(files: &[(&str, &str)]) -> (String, String) {
    let diags = check_files(files, &common::caps()).expect_err("should be rejected");
    let d = &diags[0];
    let src = files[d.span.file as usize].1;
    (d.msg.clone(), src[d.span.start..d.span.end].to_string())
}

const KEEP: &str = "fn keep<T>(d: Dir, x: T) -> T { let _r = d.read(\"a\"); x }";
const CAP_ARG: &str = "`T` cannot be `Dir`: capabilities are parameters only";

#[test]
fn a_capability_never_reaches_a_type_parameter() {
    let cases = [
        // Through an argument checked after the others.
        ("fn or<T>(a: Option<T>, b: T) -> T { b } fn main(d: Dir) { or(None, d); }", CAP_ARG, "d"),
        ("fn ap<T, U>(x: T, f: fn(T) -> U) -> U { f(x) } fn main(d: Dir) -> Int { ap(d, |y| 1) }", CAP_ARG, "d"),
        ("fn ap<T, U>(f: fn(T) -> U, x: T) -> U { f(x) } fn main(d: Dir) -> Int { ap(|y: Dir| 1, d) }", CAP_ARG, "d"),
        // Through the expected type, with `!` arguments that fix nothing.
        ("fn id<T>(x: T) -> T { x } fn take(d: Dir) {} fn main() { take(id({ return; })); }", CAP_ARG, "id({ return; })"),
        ("fn un<T, E>(r: Result<T, E>) -> T { match r { Ok(x) => x, Err(_) => { loop {} } } } fn take(d: Dir) {} fn main(d: Dir) { take(un(Ok(d))); }", CAP_ARG, "un(Ok(d))"),
        ("fn pick<T>(a: T, b: T) -> T { a } fn main(d: Dir) { pick({ return; }, d).read(\"a\"); }", CAP_ARG, "d"),
        ("fn un<T, E>(r: Result<T, E>) -> T { match r { Ok(x) => x, Err(_) => { loop {} } } } fn main(d: Dir) -> Int { un::<Int, Dir>(Err(d)) }", "`E` cannot be `Dir`: capabilities are parameters only", "Dir"),
        // Inside a generic function that holds a capability.
        ("fn f<T>(d: Dir, x: T) -> T { let y: T = d; y } fn main(input: Dir) -> Int { f(input, 1) }", "capability `d` bound to local `y`", "d"),
        ("fn f<T>(d: Dir, x: T) -> T { d } fn main(input: Dir) -> Int { f(input, 1) }", "capability used as a value", "d"),
        ("fn f<T>(d: Dir, x: T) -> T { let c = |y: T| y; c(d) } fn main(input: Dir) -> Int { f(input, 1) }", "expected T, found Dir", "d"),
        ("fn f<T>(d: Dir, x: T, b: Bool) -> T { if b { x } else { d } } fn main(input: Dir) -> Int { f(input, 1, true) }", "capability used as a value", "d"),
        // A `T` value where a capability is expected.
        ("fn keep(d: Dir) {} fn g<T>(x: T) { keep(x); } fn main() { g(5) }", "expected Dir, found T", "x"),
        ("fn g<T>(d: T) -> Int { d.read(\"a\"); 1 } fn main(input: Dir) -> Int { g(1) }", "no method `read` on T", "d.read(\"a\")"),
        // Generic functions with capability parameters are not values.
        (
            "fn keep<T>(d: Dir, x: T) -> T { x } fn ap(f: fn(Dir, Int) -> Int) {} fn main() { ap(keep); }",
            "function values cannot take or return capabilities",
            "fn(Dir, Int) -> Int",
        ),
        ("fn id<T>(x: T) -> T { x } fn main(d: Dir) { let f: fn(Dir) -> Dir = id; }", "function values cannot take or return capabilities", "fn(Dir) -> Dir"),
    ];
    for (src, msg, at) in cases {
        assert_eq!(rejected(&[("main", src)]), (msg.to_string(), at.to_string()), "{src}");
    }
    let web = "pub fn keep<T>(d: Dir, x: T) -> T { let _r = d.read(\"a\"); x }
               pub fn id<T>(x: T) -> T { x }
               pub fn mk<T>() -> T { loop {} }
               pub fn val<T>(f: fn(T) -> T, x: T) -> T { f(x) }";
    let modules = [
        (
            "mod web; fn main(input: Dir) { web::id::<Dir>(input); }",
            CAP_ARG,
            "Dir",
        ),
        ("mod web; fn main(input: Dir) { web::id(input); }", CAP_ARG, "input"),
        (
            "mod web; fn take(d: Dir) {} fn main() { take(web::mk()); }",
            CAP_ARG,
            "web::mk()",
        ),
        (
            "mod web; fn main(input: Dir) -> Int { web::val(|d: Dir| d, input); 1 }",
            CAP_ARG,
            "input",
        ),
        (
            "mod web; fn main(input: Dir) -> Int { let f = web::keep::<Int>; 1 }",
            "function values cannot take or return capabilities",
            "web::keep::<Int>",
        ),
    ];
    for (root, msg, at) in modules {
        assert_eq!(
            rejected(&[("main", root), ("web", web)]),
            (msg.to_string(), at.to_string()),
            "{root}"
        );
    }
}

#[test]
fn the_report_charges_every_generic_call_shape() {
    let web = "pub fn keep<T>(d: Dir, x: T) -> T { let _r = d.read(\"a\"); x }
               pub fn hop<T>(d: Dir, x: T) -> T { keep(d, x) }
               pub fn two<T>(x: T, d: Dir) -> T { let _r = d.list(\"\"); x }
               pub fn both<T>(d: Dir, x: T) -> T { let c = |y: T| y; let _r = d.exists(\"a\"); c(x) }
               pub fn go(d: Dir) -> Int { web::keep::<Int>(d, 1) + keep::<Int>(d, 2) }";
    let root = "mod web;
                fn main(input: Dir) -> Int {
                  [web::keep::<Int>(input, 1)][0] + web::hop(input, 2) + web::two::<Str>(\"s\", input).len()
                    + Some(web::both(input, 3)).unwrap_or(0) + web::go(input)
                }";
    let p = check_files(&[("main", root), ("web", web)], &common::caps()).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(
        report(&p),
        "main      : input exists list read\n\
         web::both : d exists\n\
         web::go   : d read\n\
         web::hop  : d read\n\
         web::keep : d read\n\
         web::two  : d list"
    );
    let src = format!(
        "{KEEP} fn first<T>(xs: List<T>) -> T {{ xs[0] }} fn id<T>(x: T) -> T {{ x }}
         fn main(input: Dir) -> Int {{
           match keep(input, true) {{ true => id(first([keep::<Int>(input, 1)])), false => keep(input, keep(input, 0)) }}
         }}"
    );
    let p = check(&src, &common::caps()).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(
        report(&p),
        "first : none\nid    : none\nkeep  : d read\nmain  : input read"
    );
}

#[test]
fn never_typed_arguments_make_instances_that_run() {
    // `Some({ return 7; })` is `Option<!>`: an instance `id<Option<!>>` the run never reaches.
    assert_eq!(
        shown("fn id<T>(x: T) -> T { x } fn main() -> Int { let o = id(Some({ return 7; })); 1 }"),
        "7"
    );
    assert_eq!(
        shown("fn id<T>(x: T) -> T { x } fn w<T>(x: T) -> List<T> { [x] } fn main() -> Int { if false { w(id(Some({ return 7; }))); } 3 }"),
        "3"
    );
    assert_eq!(
        shown("fn ap<T, U>(x: T, f: fn(T) -> U) -> U { f(x) } fn main() -> Int { if false { ap(1, |x| { return 9; }); } 6 }"),
        "6"
    );
}

#[test]
fn opaque_values_of_every_kind_round_trip() {
    // `match` and closures over a `T` that is a type `match` would otherwise refuse.
    let m = "fn f<T>(x: T) -> T { match x { y => y } }";
    assert_eq!(shown(&format!("{m} fn main() -> Float {{ f(1.5) }}")), "1.5");
    assert_eq!(shown(&format!("{m} fn main() -> List<Int> {{ f([1, 2]) }}")), "[1, 2]");
    assert_eq!(
        shown(&format!(
            "{m} fn inc(x: Int) -> Int {{ x + 1 }} fn main() -> Int {{ (f(inc))(1) }}"
        )),
        "2"
    );
    assert_eq!(shown(&format!("{m} fn main() {{ f(()) }}")), "()");
    assert_eq!(
        shown(&format!("{m} fn main() -> Int {{ let mut m: Map<Str, Int> = Map::new(); m.insert(\"a\", 4); f(m).get(\"a\").unwrap_or(0) }}")),
        "4"
    );
    let c = "fn f<T>(x: T) -> fn() -> fn() -> T { || || x }";
    assert_eq!(shown(&format!("{c} fn main() -> Str {{ ((f(\"s\"))())() }}")), "s");
    assert_eq!(shown(&format!("{c} fn main() -> Float {{ ((f(2.5))())() }}")), "2.5");
    let r = "fn g<T>(x: T, n: Int) -> T { if n == 0 { x } else { let c = || g(x, n - 1); c() } }";
    assert_eq!(
        shown(&format!("{r} fn main() -> List<Str> {{ g([\"q\"], 3) }}")),
        "[\"q\"]"
    );
    let v = "fn f<T>(x: T) -> T { x }";
    assert_eq!(
        shown(&format!("{v} fn main() -> List<fn(Str) -> Str> {{ [f, f::<Str>] }}")),
        "[<fn f<Str>>, <fn f<Str>>]"
    );
    assert_eq!(
        shown(&format!(
            "{v} fn main() -> fn(List<Option<Str>>) -> List<Option<Str>> {{ f::<List<Option<Str>>> }}"
        )),
        "<fn f<List<Option<Str>>>>"
    );
}

#[test]
fn type_parameter_names_that_are_not_types() {
    // Not refused as built-in types: a value name is another namespace. Documented as found.
    for t in ["Some", "None", "Ok", "Err", "Never", "Fn"] {
        let src = format!("fn f<{t}>(x: {t}) -> {t} {{ x }} fn main() -> Int {{ f(1) }}");
        assert_eq!(shown(&src), "1", "{t}");
    }
    // A type parameter hides nothing from another module: `web::Row` still resolves.
    let root = "mod web; fn f<Row>(x: Row) -> Int { let r = web::Row { b: 1 }; r.b } fn main() -> Int { f(1) }";
    let p = check_files(
        &[("main", root), ("web", "pub struct Row { b: Int }")],
        &CapTable::default(),
    )
    .unwrap();
    assert_eq!(
        common::big_stack(move || p.display(&run(&p, &mut NoCaps).unwrap())),
        "1"
    );
}

#[test]
fn a_wide_type_argument_is_refused_as_instantiating_without_end() {
    // 2000 parameters in one hand-written `fn` type pass the 256-constructor bound meant for
    // recursion; the message is wrong for it (review finding).
    let src = format!(
        "fn id<T>(x: T) -> T {{ x }} fn main() {{ id::<fn({}) -> Int>({{ return; }}); }}",
        vec!["Int"; 2000].join(", ")
    );
    let (msg, _) = rejected(&[("main", &src)]);
    assert_eq!(msg, "this generic function instantiates without end");
}
