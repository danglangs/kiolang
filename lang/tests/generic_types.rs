//! L7b: generic structs and enums, checked once with opaque type parameters and monomorphized
//! per instance. The differential harness runs these programs compiled too.
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

/// Runs a program of `files` and displays `main`'s result.
fn shown_files(files: &[(&str, &str)]) -> String {
    let p = check_files(files, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || {
        let v = run(&p, &mut NoCaps).unwrap_or_else(|e| panic!("{}", e.msg));
        p.display(&v)
    })
}

fn shown(src: &str) -> String {
    shown_files(&[("main", src)])
}

/// The single diagnostic for `src`: message, the source it points at, and hint.
fn reject(src: &str) -> (String, String, String) {
    let diags = check(src, &common::caps()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    let hint = d.hint.clone().unwrap_or_default();
    (d.msg.clone(), src[d.span.start..d.span.end].to_string(), hint)
}

fn rejected(src: &str, msg: &str, at: &str, hint: &str) {
    assert_eq!(
        reject(src),
        (msg.to_string(), at.to_string(), hint.to_string()),
        "{src}"
    );
}

#[test]
fn struct_literals_infer_from_fields_and_the_expected_type() {
    let src = "struct Page<T> { items: List<T>, next: Option<Int> }
               fn main() -> List<Page<Str>> {
                 let a = Page { items: [1, 2], next: Some(3) };
                 let mut b: Page<Str> = Page { items: [], next: None };
                 b.items.push(\"x\");
                 b.next = Some(a.items[1] + a.next.unwrap_or(0));
                 [b, Page { items: [\"y\"], next: None }]
               }";
    assert_eq!(
        shown(src),
        "[Page { items: [\"x\"], next: Some(5) }, Page { items: [\"y\"], next: None }]"
    );
}

#[test]
fn enum_variants_infer_from_payloads_and_the_expected_type() {
    let src = "enum Tree<T> { Leaf, Node(List<Tree<T>>, T) }
               fn sum(t: Tree<Int>) -> Int {
                 match t {
                   Tree::Leaf => 0,
                   Tree::Node(kids, x) => { let mut s = x; for k in kids { s += sum(k); } s }
                 }
               }
               fn main() -> Int {
                 let leaf: Tree<Int> = Tree::Leaf;
                 let t = Tree::Node([Tree::Node([], 2), leaf, Tree::Leaf], 1);
                 sum(t) + sum(Tree::Leaf)
               }";
    assert_eq!(shown(src), "3");
    let src = "enum Tree<T> { Leaf, Node(List<Tree<T>>, T) }
               fn main() -> Tree<Str> { Tree::Node([Tree::Leaf], \"a\") }";
    assert_eq!(shown(src), "Tree::Node([Tree::Leaf], \"a\")");
}

#[test]
fn two_type_parameters_and_nested_instances() {
    let src = "enum Either<A, B> { Left(A), Right(B) }
               struct Pair<A, B> { a: A, b: B }
               fn main() -> Pair<Either<Int, Str>, Pair<Bool, List<Int>>> {
                 let e: Either<Int, Str> = Either::Right(\"r\");
                 Pair { a: e, b: Pair { a: true, b: [1] } }
               }";
    assert_eq!(
        shown(src),
        "Pair { a: Either::Right(\"r\"), b: Pair { a: true, b: [1] } }"
    );
    // Matched by value: patterns take the instance's payload types.
    let src = "enum Either<A, B> { Left(A), Right(B) }
               fn show(e: Either<Int, Str>) -> Str { match e { Either::Left(n) => n.to_str(), Either::Right(s) => s } }
               fn main() -> Str { show(Either::Left(4)) + show(Either::Right(\"x\")) }";
    assert_eq!(shown(src), "4x");
}

#[test]
fn generic_types_inside_generic_functions() {
    let src = "struct Page<T> { items: List<T>, next: Option<Int> }
               fn first<T>(p: Page<T>) -> Option<T> { if p.items.len() == 0 { None } else { Some(p.items[0]) } }
               fn wrap<T>(x: T) -> Page<T> { Page { items: [x], next: None } }
               fn empty<T>() -> Page<T> { Page { items: [], next: Some(0) } }
               fn main() -> Str {
                 let e: Page<Str> = empty();
                 first(wrap(\"a\")).unwrap_or(\"none\") + first(e).unwrap_or(\"-\") + first(wrap(7)).unwrap_or(0).to_str()
               }";
    assert_eq!(shown(src), "a-7");
    let src = "enum Tree<T> { Leaf, Node(List<Tree<T>>, T) }
               fn count<T>(t: Tree<T>) -> Int {
                 match t { Tree::Leaf => 0, Tree::Node(kids, _) => { let mut n = 1; for k in kids { n += count(k); } n } }
               }
               fn single<T>(x: T) -> Tree<T> { Tree::Node([Tree::Leaf], x) }
               fn main() -> Int { count(Tree::Node([single(\"a\"), single(\"b\")], \"c\")) + count(single(1)) }";
    assert_eq!(shown(src), "4");
}

#[test]
fn generic_types_in_modules() {
    let web = "pub struct Page<T> { pub_items: List<T> }
               pub enum Tree<T> { Leaf, Node(List<Tree<T>>, T) }
               pub fn leaf<T>() -> Tree<T> { Tree::Leaf }
               pub fn page<T>(x: T) -> Page<T> { Page { pub_items: [x] } }";
    let root = "mod web;
                fn main() -> List<web::Page<Int>> {
                  let t: web::Tree<Int> = web::Tree::Leaf;
                  let u = web::Tree::Node([t, web::leaf()], 2);
                  let n = match u { web::Tree::Node(kids, x) => kids.len() + x, web::Tree::Leaf => 0 };
                  [web::Page { pub_items: [n] }, web::page(5)]
                }";
    assert_eq!(
        shown_files(&[("main", root), ("web", web)]),
        "[web::Page { pub_items: [4] }, web::Page { pub_items: [5] }]"
    );
}

#[test]
fn equality_on_instances_whose_arguments_allow_it() {
    let src = "struct Pair<A, B> { a: A, b: B }
               enum Opt<T> { No, Yes(T) }
               fn main() -> List<Bool> {
                 let p = Pair { a: 1, b: \"s\" };
                 let o: Opt<Int> = Opt::No;
                 [p == Pair { a: 1, b: \"s\" }, p != Pair { a: 2, b: \"s\" }, o == Opt::Yes(1), Opt::Yes(\"a\") == Opt::Yes(\"a\")]
               }";
    assert_eq!(shown(src), "[true, true, false, true]");
    rejected(
        "struct Box<T> { v: T } fn same<T>(a: Box<T>, b: Box<T>) -> Bool { a == b } fn main() {}",
        "cannot compare `Box<T>` values",
        "a == b",
        "a type parameter has no `==` yet; take a function for it, like `eq: fn(T, T) -> Bool`",
    );
    rejected(
        "struct Box<T> { v: T } fn inc(x: Int) -> Int { x } fn main() -> Bool { Box { v: inc } == Box { v: inc } }",
        "cannot compare `Box<fn(Int) -> Int>` values",
        "Box { v: inc } == Box { v: inc }",
        "functions have no equality",
    );
}

#[test]
fn json_style_enum_is_unchanged() {
    let src = "enum Json { Null, Num(Int), Str(Str), Arr(List<Json>), Obj(Map<Str, Json>) }
               fn size(j: Json) -> Int {
                 match j { Json::Arr(xs) => { let mut n = 1; for x in xs { n += size(x); } n }, _ => 1 }
               }
               fn main() -> Int { size(Json::Arr([Json::Null, Json::Num(1), Json::Arr([Json::Str(\"a\")])])) }";
    assert_eq!(shown(src), "5");
}

#[test]
fn type_parameters_are_checked_as_on_functions() {
    rejected(
        "struct S<t> { a: t } fn main() {}",
        "type parameters start with an uppercase letter: `t`",
        "t",
        "rename it `T`",
    );
    rejected(
        "enum E<T, T> { A(T) } fn main() {}",
        "duplicate type parameter `T`",
        "T",
        "rename one of them",
    );
    rejected(
        "struct S<Dir> { a: Int } fn main() {}",
        "`Dir` is a built-in type",
        "Dir",
        "name type parameters `T`, `U`, ..",
    );
    rejected(
        "struct P { a: Int } struct S<P> { a: Int } fn main() {}",
        "type parameter `P` has the name of struct `P`",
        "P",
        "rename the type parameter",
    );
    rejected(
        "enum E<None> { A } fn main() {}",
        "`None` is a built-in constructor",
        "None",
        "name type parameters `T`, `U`, ..",
    );
    rejected(
        "struct S<T> { a: T } fn f(s: S<Int>) -> Int { s.a.len() } fn main() {}",
        "no method `len` on Int",
        "s.a.len()",
        "Int has: to_float, to_str",
    );
}

#[test]
fn uses_give_every_type_argument() {
    rejected(
        "struct Page<T> { items: List<T> } fn f(p: Page) {} fn main() {}",
        "`Page` takes 1 type arguments, found 0",
        "Page",
        "`Page` is declared as `struct Page<T>`",
    );
    rejected(
        "enum E<A, B> { X(A), Y(B) } fn f(e: E<Int>) {} fn main() {}",
        "`E` takes 2 type arguments, found 1",
        "E<Int>",
        "`E` is declared as `enum E<A, B>`",
    );
    rejected(
        "struct P { a: Int } fn f(p: P<Int>) {} fn main() {}",
        "`P` takes 0 type arguments, found 1",
        "P<Int>",
        "write `P`",
    );
}

#[test]
fn literals_and_variants_that_cannot_be_inferred() {
    rejected(
        "enum Tree<T> { Leaf, Node(List<Tree<T>>, T) } fn main() { let t = Tree::Leaf; }",
        "cannot infer `T` for `Tree::Leaf`",
        "Tree::Leaf",
        "add a type: `let t: Tree<Int> = Tree::Leaf;`",
    );
    rejected(
        "enum Tree<T> { Leaf, Node(List<Tree<T>>, T) } fn main() { let t = Tree::Node([Tree::Leaf], Tree::Leaf); }",
        "cannot infer `T` for `Tree::Node(..)`",
        "Tree::Node([Tree::Leaf], Tree::Leaf)",
        "add a type: `let t: Tree<Int> = Tree::Node(..);`",
    );
    rejected(
        "struct Page<T> { items: List<T>, n: Option<T> } fn main() { let p = Page { items: [], n: None }; }",
        "cannot infer `T` for `Page { .. }`",
        "Page { items: [], n: None }",
        "add a type: `let p: Page<Int> = Page { .. };`",
    );
    rejected(
        "struct P<T> { a: T, b: T } fn main() { let p = P { a: 1, b: \"s\" }; }",
        "`T` is Int here but Str at field `b`",
        "1",
        "`P` is declared as `struct P<T>`",
    );
    rejected(
        "enum E<T> { Two(T, T) } fn main() { let e = E::Two(1, true); }",
        "`T` is Int here but Bool at value 2",
        "1",
        "`E::Two` is declared as `enum E<T>`",
    );
    rejected(
        "struct Page<T> { items: List<T> } fn main() { let p: Page<Str> = Page { items: [1] }; }",
        "expected Page<Str>, found Page<Int>",
        "Page { items: [1] }",
        "",
    );
    rejected(
        "enum Tree<T> { Leaf } fn take<T>(t: Tree<T>) {} fn main() { take(Tree::Leaf); }",
        "cannot infer `T`",
        "take(Tree::Leaf)",
        "write `take::<Int>(..)`",
    );
}

#[test]
fn rule_1_no_type_argument_holds_a_capability() {
    let cap = "`T` cannot be `Dir`: capabilities are parameters only";
    let hint = "pass the Dir to a function whose parameter has type `Dir`";
    rejected(
        "struct Box<T> { v: T } fn f(b: Box<Dir>) {} fn main() {}",
        cap,
        "Dir",
        hint,
    );
    rejected(
        "struct Box<T> { v: T } fn main(d: Dir) { let b: Box<List<Dir>> = Box { v: [] }; }",
        "`T` cannot be `List<Dir>`: capabilities are parameters only",
        "List<Dir>",
        hint,
    );
    rejected(
        "struct Box<T> { v: T } fn main(d: Dir) { let b = Box { v: d }; }",
        "capability used as a value",
        "d",
        "call methods on the parameter itself",
    );
    rejected(
        "enum E<T> { A(T) } fn main(d: Dir) { let e = E::A(d); }",
        "capability used as a value",
        "d",
        "call methods on the parameter itself",
    );
    rejected(
        "struct Box<T> { v: T } fn main() { let b: Box<fn(Dir)> = Box { v: |d: Dir| {} }; }",
        "`T` cannot be `fn(Dir)`: capabilities are parameters only",
        "fn(Dir)",
        hint,
    );
    rejected(
        "enum E<T> { A(T), B } fn take(d: Dir) {} fn f<T>(e: E<T>) -> T { match e { E::A(x) => x, E::B => { loop {} } } } fn main(d: Dir) { take(f(E::B)); }",
        cap,
        "f(E::B)",
        hint,
    );
}

#[test]
fn a_type_that_contains_itself_is_refused_per_instance() {
    let hint = "use indexes into a `List` instead";
    rejected(
        "struct W<T> { t: T } struct X { w: W<X> } fn main() {}",
        "struct `X` contains itself",
        "struct X { w: W<X> }",
        hint,
    );
    rejected(
        "struct S<T> { s: Option<S<List<T>>> } fn main() {}",
        "struct `S` contains itself",
        "struct S<T> { s: Option<S<List<T>>> }",
        hint,
    );
    // Finite: a type argument that is an instance of the same type, and recursion through List.
    let src = "struct W<T> { t: T } enum Tree<T> { Leaf, Node(List<Tree<T>>, T) }
               fn main() -> W<W<Tree<Int>>> { W { t: W { t: Tree::Node([Tree::Leaf], 1) } } }";
    assert_eq!(shown(src), "W { t: W { t: Tree::Node([Tree::Leaf], 1) } }");
}

#[test]
fn instances_of_a_type_are_bounded() {
    let grow = "struct S<T> { s: List<S<List<T>>> }";
    // Unused, it is accepted, as an unused generic function that would recurse is.
    assert_eq!(shown(&format!("{grow} fn main() -> Int {{ 1 }}")), "1");
    rejected(
        &format!("{grow} fn main() {{ let s: S<Int> = S {{ s: [] }}; }}"),
        "generic type `S` instantiates without end",
        grow,
        "a field or payload holds this type with larger type arguments; keep them the same",
    );
    // Named only as a type argument, through a type parameter of a generic function.
    rejected(
        &format!("{grow} fn g<T>() -> Int {{ let x: Option<T> = None; 1 }} fn main() -> Int {{ g::<S<Int>>() }}"),
        "generic type `S` instantiates without end",
        grow,
        "a field or payload holds this type with larger type arguments; keep them the same",
    );
    // Through a generic function that wraps its argument each time.
    let src = "struct W<T> { t: T }
               fn f<T>(x: T, n: Int) -> Int { if n == 0 { 0 } else { f(W { t: x }, n - 1) } }
               fn main() -> Int { f(1, 3) }";
    let (msg, _, _) = reject(src);
    assert_eq!(msg, "this generic function instantiates without end");
}
