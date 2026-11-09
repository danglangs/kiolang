//! L7b adversarial review: attacks on rule 1 and the limits through generic structs and
//! enums, each of which must keep failing, and generic-type programs the interpreter and
//! compiler must agree on (the differential harness picks these literals up).
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

const BOX: &str = "struct Box<T> { v: T }";
const TREE: &str = "enum Tree<T> { Leaf, Node(T) }";
const CAP_ARG: &str = "`T` cannot be `Dir`: capabilities are parameters only";
const CAP_VALUE: &str = "capability used as a value";

#[test]
fn a_capability_never_reaches_a_type_argument() {
    let cases = [
        // From a value: a literal or variant checks its values as expressions, never operands.
        ("fn main(d: Dir) { let b = Box { v: d }; }", CAP_VALUE, "d"),
        ("fn main(d: Dir) { let b = Box { v: Some(d) }; }", CAP_VALUE, "d"),
        ("fn main(d: Dir) { let t = Tree::Node(d); }", CAP_VALUE, "d"),
        ("fn main(d: Dir) { let b = if true { Box { v: 1 } } else { Box { v: d } }; }", CAP_VALUE, "d"),
        ("fn main(d: Dir) { let n = Box { v: d }.v.read(\"a\"); }", CAP_VALUE, "d"),
        ("struct Page<T> { n: Int, items: List<T> } fn main(d: Dir) { let p = Page { n: d, items: [1] }; }", CAP_VALUE, "d"),
        // From a generic call whose argument is a capability operand.
        ("fn wrap<T>(x: T) -> Box<T> { Box { v: x } } fn main(d: Dir) { let b = wrap(d); }", CAP_ARG, "d"),
        ("fn wrap<T>(x: T) -> Box<T> { Box { v: x } } fn main(d: Dir) { let b: Box<Int> = wrap(d); }", CAP_ARG, "d"),
        ("fn f<T>(b: Tree<T>, a: T) -> Int { 1 } fn main(d: Dir) { f(Tree::Leaf, d); }", CAP_ARG, "d"),
        ("fn f<T>(a: T, b: Tree<T>) -> Int { 1 } fn main(d: Dir) { f(d, Tree::Leaf); }", CAP_ARG, "d"),
        ("struct Pair<A, B> { a: A, b: B } fn mk<A, B>(a: A, b: B) -> Pair<A, B> { Pair { a: a, b: b } } fn main(d: Dir) { mk(1, d); }", "`B` cannot be `Dir`: capabilities are parameters only", "d"),
        ("struct M<T> { m: Map<Str, T> } fn f<T>(x: T) -> M<T> { let mut m: Map<Str, T> = Map::new(); m.insert(\"a\", x); M { m: m } } fn main(d: Dir) { f(d); }", CAP_ARG, "d"),
        ("fn id<T>(x: T) -> T { x } fn mk<T>(x: T) -> Box<T> { Box { v: x } } fn main(d: Dir) { mk(id(d)); }", CAP_ARG, "d"),
        // Written, wherever a type goes.
        ("fn main(d: Dir) { let b: Box<Dir> = Box { v: d }; }", CAP_ARG, "Dir"),
        ("fn main(d: Dir) { let b: Option<Box<Dir>> = None; }", CAP_ARG, "Dir"),
        ("fn take(b: Box<Dir>) {} fn main(d: Dir) { take(Box { v: d }); }", CAP_ARG, "Dir"),
        ("fn take(t: Tree<Dir>) {} fn main(d: Dir) { take(Tree::Leaf); }", CAP_ARG, "Dir"),
        ("fn mk(d: Dir) -> Box<Dir> { Box { v: d } } fn main(d: Dir) {}", CAP_ARG, "Dir"),
        ("fn f(g: fn(Box<Dir>)) {} fn main(d: Dir) {}", CAP_ARG, "Dir"),
        ("fn main(d: Dir) { let t: Tree<Box<List<Dir>>> = Tree::Leaf; }", "`T` cannot be `List<Dir>`: capabilities are parameters only", "List<Dir>"),
        ("fn helper(d: Dir) {} fn main(d: Dir) { let b: Box<fn(Dir)> = Box { v: helper }; }", "`T` cannot be `fn(Dir)`: capabilities are parameters only", "fn(Dir)"),
        ("fn helper() -> Box<Option<Dir>> { Box { v: None } } fn main(d: Dir) {}", "`T` cannot be `Option<Dir>`: capabilities are parameters only", "Option<Dir>"),
        // A `fn` value or closure holding a capability cannot be a field value either.
        ("struct H<T> { f: fn(T) -> Int } fn helper(d: Dir) -> Int { 1 } fn main(d: Dir) { let h = H { f: helper }; }", "function values cannot take or return capabilities", "helper"),
        ("struct H<T> { f: fn(T) -> Int } fn main(d: Dir) { let h: H<Int> = H { f: |x| { d.read(\"a\"); x } }; }", "closure captures capability `d`", "d"),
        // A type parameter inside a generic function has no capability methods.
        ("fn f<T>(b: Box<T>) -> Int { b.v.read(\"a\"); 1 } fn main(d: Dir) {}", "no method `read` on T", "b.v.read(\"a\")"),
        // A type parameter cannot take a capability's name and so be one in disguise.
        ("struct S<Dir> { d: Dir } fn main(d: Dir) {}", "`Dir` is a built-in type", "Dir"),
        ("struct S<WsSend> { d: WsSend } fn main(d: Dir) {}", "`WsSend` is a built-in type", "WsSend"),
        ("fn f<Http>(h: Http) -> Http { h } fn main(d: Dir) {}", "`Http` is a built-in type", "Http"),
    ];
    for (src, msg, at) in cases {
        let src = format!("{BOX} {TREE} {src}");
        assert_eq!(rejected(&[("main", &src)]), (msg.to_string(), at.to_string()), "{src}");
    }
}

#[test]
fn a_capability_never_reaches_a_module_generic_type() {
    let web =
        "pub struct Box<T> { v: T } pub enum Tree<T> { Leaf, Node(T) } pub fn wrap<T>(x: T) -> Box<T> { Box { v: x } }";
    let cases = [
        (
            "mod web; fn main(d: Dir) { let b: web::Box<Dir> = web::Box { v: d }; }",
            CAP_ARG,
            "Dir",
        ),
        (
            "mod web; fn main(d: Dir) { let b = web::Box { v: d }; }",
            CAP_VALUE,
            "d",
        ),
        ("mod web; fn main(d: Dir) { let b = web::wrap(d); }", CAP_ARG, "d"),
        (
            "mod web; fn take(t: web::Tree<Dir>) {} fn main(d: Dir) { take(web::Tree::Leaf); }",
            CAP_ARG,
            "Dir",
        ),
        (
            "mod web; fn helper(d: Dir) {} fn main(d: Dir) { let b: web::Box<fn(Dir)> = web::Box { v: helper }; }",
            "`T` cannot be `fn(Dir)`: capabilities are parameters only",
            "fn(Dir)",
        ),
    ];
    for (src, msg, at) in cases {
        assert_eq!(
            rejected(&[("main", src), ("web", web)]),
            (msg.to_string(), at.to_string()),
            "{src}"
        );
    }
    // Inside the module too.
    let web = "pub struct Box<T> { v: T } struct Job { b: Box<Dir> } pub fn go(d: Dir) {}";
    let src = "mod web; fn main(d: Dir) { web::go(d); }";
    assert_eq!(
        rejected(&[("main", src), ("web", web)]),
        (CAP_ARG.to_string(), "Dir".to_string())
    );
}

#[test]
fn the_report_charges_generic_types_holding_nothing() {
    // A generic function with a capability parameter and a generic struct argument: the
    // capability's authority is its own, the same for every instance.
    let src = "struct Box<T> { v: T }
               fn keep<T>(d: Dir, x: Box<T>) -> Int { let r = d.read(\"a\"); 1 }
               fn outer<T>(d: Dir, x: T) -> Int { keep(d, Box { v: x }) }
               fn main(input: Dir, out: Dir) -> Int { keep(input, Box { v: 5 }) + keep::<Str>(input, Box { v: \"a\" }) + outer(out, true) }";
    let p = check(src, &common::caps()).unwrap();
    assert_eq!(
        report(&p),
        "keep  : d read\nmain  : input read, out read\nouter : d read"
    );
    let src = "struct Box<T> { v: T }
               fn main(input: Dir) -> Box<Result<Bytes, IoErr>> { Box { v: input.read(\"a\") } }";
    let p = check(src, &common::caps()).unwrap();
    assert_eq!(report(&p), "main : input read");
}

#[test]
fn instance_explosion_through_types_is_refused() {
    let without_end = |name: &str| format!("generic type `{name}` instantiates without end");
    let cases = [
        ("struct S<T> { a: List<S<Option<T>>> } fn main() { let s: S<Int> = S { a: [] }; }", without_end("S")),
        ("struct S<T> { m: Map<Str, S<Option<T>>> } fn main() { let s: S<Int> = S { m: Map::new() }; }", without_end("S")),
        ("struct S<T> { f: Option<fn(S<Option<T>>) -> Int> } fn main() { let s: S<Int> = S { f: None }; }", without_end("S")),
        ("struct S<T> { a: List<S<List<T>>>, b: List<S<Option<T>>> } fn main() { let s: S<Int> = S { a: [], b: [] }; }", without_end("S")),
        ("struct A<T> { b: List<B<Option<T>>> } struct B<T> { a: List<A<T>> } fn main() { let a: A<Int> = A { b: [] }; }", without_end("B")),
        ("struct W<T> { t: Option<T> } struct X { w: W<X> } fn main() {}", "struct `X` contains itself".to_string()),
        ("struct S<T> { s: Option<S<T>> } fn main() {}", "struct `S` contains itself".to_string()),
        ("struct S<T> { r: Result<S<T>, Str> } fn main() {}", "struct `S` contains itself".to_string()),
        ("struct P<A, B> { q: Option<P<B, A>> } fn main() {}", "struct `P` contains itself".to_string()),
        ("enum Tree<T> { Leaf, Node(Tree<Option<T>>) } fn main() {}", "enum `Tree` contains itself".to_string()),
        ("struct Box<T> { v: T } fn f<T>(x: T, n: Int) -> Int { let g = |y: T| f(Box { v: y }, n - 1); if n == 0 { 0 } else { g(x) } } fn main() -> Int { f(1, 2) }", "this generic function instantiates without end".to_string()),
        ("struct Box<T> { v: T } fn f<T>(b: Box<T>) -> Int { g(b.v) } fn g<T>(x: T) -> Int { f(Box { v: [x] }) } fn main() -> Int { g(1) }", "this generic function instantiates without end".to_string()),
    ];
    for (src, msg) in cases {
        assert_eq!(rejected(&[("main", src)]).0, msg, "{src}");
    }
    // 64 instances reached through fields of types that are not generic are fine; the 65th is
    // refused at the field naming it, not blamed on a growing type.
    let mut src = "struct Box<T> { v: T }\n".to_string();
    for i in 0..65 {
        src += &format!("struct A{i} {{ }} struct N{i} {{ b: Option<Box<A{i}>> }}\n");
    }
    src += "fn main() {}";
    assert_eq!(
        rejected(&[("main", &src)]),
        (
            "`Box` has more than 64 instances".to_string(),
            "struct N64 { b: Option<Box<A64>> }".to_string()
        )
    );
    let ok = src.replace("struct A64 { } struct N64 { b: Option<Box<A64>> }\n", "");
    check(&ok, &common::caps()).unwrap();
    // A type argument wider than 256 types, not only deeper.
    let n = 300;
    let params: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
    let fields: Vec<String> = (0..n).map(|i| format!("f{i}: T{i}")).collect();
    let src = format!(
        "struct Wide<{}> {{ {} }} struct Box<T> {{ v: T }} fn main() {{ let b: Option<Box<Wide<{}>>> = None; }}",
        params.join(", "),
        fields.join(", "),
        vec!["Int"; n].join(", ")
    );
    assert_eq!(
        rejected(&[("main", &src)]).0,
        "a type argument of `Box` is too large: 301 types, at most 256"
    );
}

#[test]
fn a_diverging_value_in_a_type_argument_runs() {
    // `!` inside a type argument (`Box<Option<!>>`) lowers like any other part that never exists.
    let src = "struct Box<T> { v: T }
               fn main() -> Int { let b = Box { v: Some({ return 1; }) }; if b == b { 2 } else { 3 } }";
    assert_eq!(shown(src), "1");
    let src = "enum Tree<T> { Leaf, Node(T) }
               fn main() -> Int { let t = Tree::Node(Some({ return 1; })); match t { Tree::Node(_) => 2, Tree::Leaf => 3 } }";
    assert_eq!(shown(src), "1");
    let src = "struct Box<T> { v: T }
               fn main() -> Box<fn() -> Int> { let b: Box<fn() -> Int> = Box { v: || { loop { } } }; b }";
    assert_eq!(shown(src), "Box { v: <closure> }");
}

#[test]
fn instances_of_one_type_never_mix() {
    let cases = [
        ("fn main() { let xs = [Box { v: 1 }, Box { v: \"a\" }]; }", "expected Box<Int>, found Box<Str>", "Box { v: \"a\" }"),
        ("fn main() { let mut b = Box { v: 1 }; b = Box { v: \"a\" }; }", "expected Box<Int>, found Box<Str>", "Box { v: \"a\" }"),
        ("fn main() { let mut b = Box { v: 1 }; b.v = \"a\"; }", "expected Int, found Str", "\"a\""),
        ("fn main() -> Bool { Box { v: 1 } == Box { v: \"1\" } }", "`==` cannot be applied to Box<Int> and Box<Str>", "Box { v: 1 } == Box { v: \"1\" }"),
        ("struct U1 { } struct U2 { } fn main() -> Bool { Box { v: U1 { } } == Box { v: U2 { } } }", "`==` cannot be applied to Box<U1> and Box<U2>", "Box { v: U1 { } } == Box { v: U2 { } }"),
        ("fn f<T>(x: T) -> Bool { let b = Box { v: x }; b == b } fn main() -> Bool { f(1) }", "cannot compare `Box<T>` values", "b == b"),
        ("fn inc(x: Int) -> Int { x } fn main() -> Bool { let a = Box { v: inc }; a == a }", "cannot compare `Box<fn(Int) -> Int>` values", "a == a"),
        ("struct S<T> { n: Int } fn inc(x: Int) -> Int { x } fn main() -> Bool { let a: S<fn(Int) -> Int> = S { n: 1 }; a == a }", "cannot compare `S<fn(Int) -> Int>` values", "a == a"),
        ("enum Other<T> { Leaf } fn main() -> Int { let t: Tree<Int> = Tree::Node(1); match t { Tree::Node(v) => v, Other::Leaf => 0 } }", "expected Tree<Int>, found Other", "Other::Leaf"),
    ];
    for (src, msg, at) in cases {
        let src = format!("{BOX} {TREE} {src}");
        assert_eq!(rejected(&[("main", &src)]), (msg.to_string(), at.to_string()), "{src}");
    }
}

#[test]
fn instances_display_and_compare_by_value() {
    // Two instances share a name in the IR and in display; values never cross.
    let src = "struct Page<T> { items: List<T> }
               struct Both { a: Page<Int>, b: Page<Str>, c: Page<Page<Int>> }
               fn main() -> Both { let e: Page<Int> = Page { items: [] }; Both { a: Page { items: [1] }, b: Page { items: [\"1\"] }, c: Page { items: [e] } } }";
    assert_eq!(
        shown(src),
        "Both { a: Page { items: [1] }, b: Page { items: [\"1\"] }, c: Page { items: [Page { items: [] }] } }"
    );
    let src = "enum E<T> { A(T, Int), B }
               fn show_i(e: E<Int>) -> Str { match e { E::A(x, y) => (x + y).to_str(), E::B => \"b\" } }
               fn show_s(e: E<Str>) -> Str { match e { E::A(x, y) => x + y.to_str(), E::B => \"b\" } }
               fn show_l(e: E<List<Int>>) -> Str { match e { E::A(x, y) => x.len().to_str() + y.to_str(), E::B => \"b\" } }
               fn main() -> List<Str> { let c: E<Int> = E::B; let d: E<List<Int>> = E::A([], 0); [show_i(E::A(1, 2)), show_s(E::A(\"1\", 2)), show_i(c), show_l(d)] }";
    assert_eq!(shown(src), "[\"3\", \"12\", \"b\", \"00\"]");
    // Variants without a payload, per instance.
    let src = "enum Tree<T> { Leaf, Node(T) }
               fn main() -> List<Bool> {
                 let a: List<Tree<Int>> = [Tree::Leaf, Tree::Leaf];
                 let b: List<Tree<Str>> = [Tree::Leaf, Tree::Node(\"x\")];
                 [a[0] == a[1], b[0] == b[0], b[0] == b[1], a.contains(Tree::Leaf), b.contains(Tree::Node(\"x\"))]
               }";
    assert_eq!(shown(src), "[true, true, false, true, true]");
    // IEEE equality inside instances, and a NaN payload matched and compared.
    let src = "enum Tree<T> { Leaf, Node(T) }
               struct Box<T> { v: T }
               fn main() -> List<Bool> {
                 let n: Tree<Float> = Tree::Node(0.0 / 0.0);
                 let z: Tree<Float> = Tree::Node(0.0);
                 let nz: Tree<Float> = Tree::Node(-0.0);
                 let b: Tree<Box<Float>> = Tree::Node(Box { v: 0.0 / 0.0 });
                 let m = match n { Tree::Node(v) => v == v, Tree::Leaf => true };
                 [n == n, z == nz, n != n, b == b, m]
               }";
    assert_eq!(shown(src), "[false, true, true, false, false]");
    // Nested instances of one enum, displayed and matched.
    let src = "enum E<T> { A(T), B }
               fn main() -> Str {
                 let b: E<Int> = E::B;
                 let x: E<E<E<Int>>> = E::A(E::A(b));
                 let n = match x { E::A(E::A(E::A(n))) => n, E::A(E::A(E::B)) => -1, E::A(E::B) => -2, E::B => -3 };
                 n.to_str() + \" \" + (match x { E::A(_) => \"a\", E::B => \"b\" })
               }";
    assert_eq!(shown(src), "-1 a");
    // Generic function values over instances, named with their arguments.
    let src = "struct Box<T> { v: T }
               enum Tree<T> { Leaf, Node(T) }
               fn id<T>(x: T) -> T { x }
               fn main() -> fn(Box<Tree<Option<Int>>>) -> Box<Tree<Option<Int>>> { id::<Box<Tree<Option<Int>>>> }";
    assert_eq!(shown(src), "<fn id<Box<Tree<Option<Int>>>>>");
    // Type parameters of a struct and of the function using it are kept apart.
    let src = "struct Box<U> { v: U }
               fn f<T, U>(t: T, u: U) -> Box<T> { let b: Box<U> = Box { v: u }; Box { v: t } }
               struct P<A, B> { q: Q<B, A> }
               struct Q<A, B> { a: A, b: B }
               fn main() -> List<Bool> { let p = P { q: Q { a: \"s\", b: 1 } }; let r: P<Int, Str> = P { q: Q { a: \"s\", b: 1 } }; [p == r, f(\"a\", 1) == Box { v: \"a\" }] }";
    assert_eq!(shown(src), "[true, true]");
}
