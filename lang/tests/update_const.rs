//! L15 struct update `S { a: e, ..base }` and L16 constants `const N: T = lit;`.
mod common;

use lang::{check, check_files, run, CapTable, Caps, Value};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

/// `main`'s value as `kio run` prints it.
fn shown(files: &[(&str, &str)]) -> String {
    let p = check_files(files, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || {
        let v = run(&p, &mut NoCaps).unwrap_or_else(|e| panic!("{}", e.msg));
        p.display(&v)
    })
}

fn runs(src: &str, want: &str) {
    assert_eq!(shown(&[("main", src)]), want, "{src}");
}

/// The single diagnostic for `files`: message, the source it points at, and hint.
fn rejected_files(files: &[(&str, &str)]) -> (String, String, Option<String>) {
    let mut diags = check_files(files, &CapTable::default()).expect_err("should be rejected");
    diags.retain(|d| d.msg != "no `main` function");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    let src = files[d.span.file as usize].1;
    (d.msg.clone(), src[d.span.start..d.span.end].to_string(), d.hint.clone())
}

fn rejected(src: &str, msg: &str, at: &str, hint: Option<&str>) {
    let got = rejected_files(&[("main", src)]);
    assert_eq!(got, (msg.into(), at.into(), hint.map(String::from)), "{src}");
}

const P: &str = "struct P { x: Int, y: Int, s: Str, xs: List<Int> } ";

#[test]
fn update_takes_the_other_fields() {
    let src = "struct P { x: Int, y: Int, s: Str, xs: List<Int> }
               fn main() -> P { let p = P { x: 1, y: 2, s: \"a\", xs: [1] }; P { y: 5, ..p } }";
    runs(src, "P { x: 1, y: 5, s: \"a\", xs: [1] }");
    // No field written, and every field written: Rust accepts both.
    let src = "struct P { x: Int, y: Int, s: Str, xs: List<Int> }
               fn main() -> List<P> {
                 let p = P { x: 1, y: 2, s: \"a\", xs: [1] };
                 [P { ..p }, P { xs: [], s: \"b\", y: 3, x: 4, ..p }]
               }";
    runs(
        src,
        "[P { x: 1, y: 2, s: \"a\", xs: [1] }, P { x: 4, y: 3, s: \"b\", xs: [] }]",
    );
}

#[test]
fn update_evaluates_written_fields_then_base() {
    let src = "struct P { x: Int, y: Int, s: Str, xs: List<Int> }
               fn main() -> Str {
                 let p = P { x: 1, y: 2, s: \"a\", xs: [1] };
                 let mut log = \"\";
                 let q = P { y: { log += \"y\"; 7 }, x: { log += \"x\"; 8 }, ..{ log += \"b\"; p } };
                 let r = P { x: 0, y: 0, s: \"\", xs: [], ..{ log += \"!\"; q } };
                 log + \" \" + q.x.to_str() + q.y.to_str() + q.s + r.s
               }";
    runs(src, "yxb! 87a");
    // The base sees what the written fields did to a local.
    let src = "struct P { x: Int, y: Int, s: Str, xs: List<Int> }
               fn main() -> P {
                 let mut p = P { x: 1, y: 2, s: \"a\", xs: [1] };
                 let other = P { x: 9, y: 9, s: \"z\", xs: [] };
                 P { x: { let old = p.x; p = other; old }, ..p }
               }";
    runs(src, "P { x: 1, y: 9, s: \"z\", xs: [] }");
}

#[test]
fn update_from_a_call_and_copies() {
    let src = "struct P { x: Int, y: Int, s: Str, xs: List<Int> }
               fn mk(n: Int) -> P { P { x: n, y: n, s: \"m\", xs: [n] } }
               fn main() -> Str {
                 let q = P { x: 1, ..mk(4) };
                 let p = mk(2);
                 let mut r = P { y: 3, ..p };
                 r.xs.push(5);
                 r.s += \"!\";
                 let last = P { s: \"t\", ..r };
                 q.x.to_str() + q.y.to_str() + \" \" + p.xs.len().to_str() + p.s + \" \" + last.xs.len().to_str() + last.s
               }";
    runs(src, "14 1m 2t");
}

#[test]
fn update_generic_struct() {
    let src = "struct Page<T> { items: List<T>, next: Option<Int>, title: Str }
               fn bump<T>(p: Page<T>) -> Page<T> { Page { next: Some(1), ..p } }
               fn main() -> List<Page<Str>> {
                 let p = Page { items: [\"a\"], next: None, title: \"t\" };
                 let q = Page { title: \"u\", ..p };
                 let empty: Page<Str> = Page { items: [], ..q };
                 [bump(q), empty]
               }";
    runs(
        src,
        "[Page { items: [\"a\"], next: Some(1), title: \"u\" }, Page { items: [], next: None, title: \"u\" }]",
    );
    rejected(
        "struct B<T> { v: T, n: Int } fn f(b: B<Str>) -> B<Int> { B { v: 1, ..b } }",
        "`T` is Int here but Str at `..`",
        "1",
        Some("`B` is declared as `struct B<T>`"),
    );
}

#[test]
fn update_errors() {
    let q = "struct Q { x: Int } ";
    rejected(
        &format!("{P}{q}fn f(q: Q) -> P {{ P {{ x: 1, ..q }} }}"),
        "expected P, found Q",
        "q",
        Some("`..` takes the fields not written from another `P`"),
    );
    rejected(
        &format!("{P}fn f() -> P {{ P {{ ..5 }} }}"),
        "expected P, found Int",
        "5",
        Some("`..` takes the fields not written from another `P`"),
    );
    rejected(
        &format!("{P}fn f(p: P) -> P {{ P {{ z: 1, ..p }} }}"),
        "no field `z` on P",
        "1",
        Some("did you mean `s`?"),
    );
    let parsed = |src: &str| {
        let d = lang::parse::parse(src).expect_err("should be rejected");
        (d.msg, src[d.span.start..d.span.end].to_string(), d.hint.unwrap())
    };
    assert_eq!(
        parsed("fn f(p: P) -> P { P { ..p, } }"),
        (
            "cannot use a comma after the base struct".into(),
            "..p,".into(),
            "the base struct must be last: remove this comma".into()
        )
    );
    assert_eq!(
        parsed("fn f(p: P) -> P { P { ..p, x: 1 } }"),
        (
            "cannot use a comma after the base struct".into(),
            "..p,".into(),
            "the base struct must be last: move `..base` after the fields".into()
        )
    );
}

#[test]
fn update_across_modules() {
    let web = "pub struct P { pub_x: Int, y: Int } pub fn p() -> P { P { pub_x: 1, y: 2 } }";
    let main = "mod web; fn main() -> web::P { web::P { y: 3, ..web::p() } }";
    assert_eq!(shown(&[("main", main), ("web", web)]), "web::P { pub_x: 1, y: 3 }");
}

#[test]
fn constants() {
    let src = "const MAX: Int = 64;
               const LOW: Int = -3;
               const HALF: Float = -0.5;
               const NAME: Str = \"kio\";
               pub const ON: Bool = true;
               fn main() -> Str {
                 let f = |x: Int| x + MAX;
                 let scaled = HALF * 4.0;
                 NAME + \" \" + f(LOW).to_str() + \" \" + scaled.to_str() + if ON { \" on\" } else { \" off\" }
               }";
    runs(src, "kio 61 -2 on");
    // A local shadows a constant, as in Rust.
    runs(
        "const N: Int = 1; fn main() -> Int { let a = N; let N = 5; a + N }",
        "6",
    );
    runs(
        "const N: Int = 2; fn twice<T>(x: T) -> List<T> { [x, x] } fn main() -> List<Int> { twice(N) }",
        "[2, 2]",
    );
}

#[test]
fn constant_errors() {
    rejected(
        "const A: Int = \"x\"; fn main() {}",
        "expected Int, found Str",
        "\"x\"",
        None,
    );
    rejected(
        "const A: Float = 1; fn main() {}",
        "expected Float, found Int",
        "1",
        None,
    );
    let literal = Some("write the value, like `64`, or compute it in a function: `fn name() -> T { .. }`");
    rejected(
        "const A: Int = 1; const B: Int = A; fn main() {}",
        "a constant's value must be a literal",
        "A",
        literal,
    );
    rejected(
        "const B: Int = 1 + 2; fn main() {}",
        "a constant's value must be a literal",
        "1 + 2",
        literal,
    );
    rejected(
        "const A: List<Int> = []; fn main() {}",
        "a constant is an `Int`, `Float`, `Bool` or `Str`",
        "List<Int>",
        Some("use a function for other values: `fn name() -> T { .. }`"),
    );
    let rename = Some("rename one of them");
    rejected(
        "const f: Int = 1; fn f() {} fn main() {}",
        "the name `f` is already used by a function",
        "const f: Int = 1;",
        rename,
    );
    rejected(
        "struct P { x: Int } const P: Int = 1; fn main() {}",
        "the name `P` is already used by a type",
        "const P: Int = 1;",
        rename,
    );
    rejected(
        "const A: Int = 1; const A: Int = 2; fn main() {}",
        "the name `A` is already used by another constant",
        "const A: Int = 2;",
        rename,
    );
    rejected(
        "const N: Int = 1; fn main() -> Int { match 3 { N => 1, _ => 0 } }",
        "constants cannot be patterns",
        "N",
        Some("bind a name and compare in the arm: `x => if x == N { .. } else { .. }`"),
    );
    rejected(
        "const N: Int = 1; fn main() { N = 2; }",
        "cannot assign to constant `N`",
        "N",
        Some("assign to a `let mut` copy: `let mut x = N;`"),
    );
    let parsed = |src: &str| {
        let d = lang::parse::parse(src).expect_err("should be rejected");
        (d.msg, src[d.span.start..d.span.end].to_string(), d.hint.unwrap())
    };
    assert_eq!(
        parsed("fn main() { const X: Int = 1; }"),
        (
            "a constant is declared at the top level of a file".into(),
            "const".into(),
            "move it out of the function, or use `let`".into()
        )
    );
    assert_eq!(
        parsed("const X = 1;"),
        (
            "a constant needs a type".into(),
            "=".into(),
            "write the type: `const X: Int = ..`".into()
        )
    );
    assert_eq!(
        parsed("static X: Int = 1;"),
        (
            "`static` is not supported in kio".into(),
            "static".into(),
            "kio has no globals; use `const` for a fixed value, or pass values as arguments".into()
        )
    );
}

#[test]
fn constants_across_modules() {
    let web = "pub const LIMIT: Int = 10; const SECRET: Int = 1; pub fn twice() -> Int { LIMIT * 2 + SECRET }";
    let main = "mod web; fn main() -> Int { web::LIMIT + web::twice() }";
    assert_eq!(shown(&[("main", main), ("web", web)]), "31");
    let main = "mod web; fn main() -> Int { web::SECRET }";
    assert_eq!(
        rejected_files(&[("main", main), ("web", web)]),
        (
            "`SECRET` is private to `web`".into(),
            "web::SECRET".into(),
            Some("mark it `pub const SECRET` in web.kio".into())
        )
    );
    let main = "mod web; const TOP: Int = 1; fn main() -> Int { web::f() }";
    let web = "pub fn f() -> Int { TOP }";
    let (msg, at, hint) = rejected_files(&[("main", main), ("web", web)]);
    assert_eq!((msg.as_str(), at.as_str()), ("unknown name `TOP`", "TOP"));
    assert!(hint.unwrap().starts_with("a module cannot use the root's items"));
    let main = "mod web; fn main() -> Int { match 1 { web::LIMIT => 1, _ => 0 } }";
    let web = "pub const LIMIT: Int = 10;";
    assert_eq!(
        rejected_files(&[("main", main), ("web", web)]).0,
        "constants cannot be patterns"
    );
}

#[test]
fn constants_leave_no_trace_in_ir() {
    let caps = CapTable::default();
    let p = check("const N: Int = 7; fn main() -> Int { N + N }", &caps).unwrap();
    let ir = format!("{:?}", lang::lower::lower(&p, &caps));
    assert!(!ir.contains("\"N\""), "{ir}");
    assert_eq!(ir.matches("Int(7)").count(), 2, "{ir}");
}
