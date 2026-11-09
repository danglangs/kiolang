//! L7a: generic functions, checked once with opaque type parameters and monomorphized.
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

/// Runs `src` and displays `main`'s result.
fn shown(src: &str) -> String {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || {
        let v = run(&p, &mut NoCaps).unwrap_or_else(|e| panic!("{}", e.msg));
        p.display(&v)
    })
}

/// The single diagnostic for `src`: message, the source it points at, and hint.
fn reject(src: &str) -> (String, String, String) {
    let diags = check(src, &common::caps()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    let hint = d.hint.clone().unwrap_or_default();
    (d.msg.clone(), src[d.span.start..d.span.end].to_string(), hint)
}

const FIRST: &str = "fn first<T>(xs: List<T>) -> Option<T> { if xs.len() == 0 { None } else { Some(xs[0]) } }";

#[test]
fn inferred_from_arguments() {
    let src = format!(
        "{FIRST}
         fn pair<A, B>(a: A, b: B) -> List<Str> {{ [show(a), show(b)] }}
         fn show<T>(x: T) -> Str {{ \"?\" }}
         struct P {{ x: Int }}
         fn main() -> Str {{
           let a = first([1, 2]).unwrap_or(0);
           let b = first([\"x\"]).unwrap_or(\"none\");
           let c = first([P {{ x: 7 }}]).unwrap_or(P {{ x: 0 }}).x;
           a.to_str() + b + c.to_str() + pair(1, \"s\")[1]
         }}"
    );
    assert_eq!(shown(&src), "1x7?");
}

#[test]
fn inferred_from_the_expected_type() {
    let src = "fn empty<T>() -> List<T> { [] }
               fn nothing<T>() -> Option<T> { None }
               fn count(xs: List<Str>) -> Int { xs.len() }
               fn get() -> Option<Int> { nothing() }
               fn main() -> Int {
                 let mut xs: List<Int> = empty();
                 xs.push(4);
                 xs.len() + count(empty()) + get().unwrap_or(10)
               }";
    assert_eq!(shown(src), "11");
}

#[test]
fn turbofish() {
    let src = format!(
        "{FIRST}
         fn empty<T>() -> List<T> {{ [] }}
         fn main() -> Int {{ empty::<Str>().len() + first::<Int>([5]).unwrap_or(0) }}"
    );
    assert_eq!(shown(&src), "5");
}

#[test]
fn generic_calls_generic_and_recurses() {
    let src = format!(
        "{FIRST}
         fn rest<T>(xs: List<T>) -> List<T> {{
           let mut out: List<T> = [];
           for i in 1..xs.len() {{ out.push(xs[i]); }}
           out
         }}
         fn second<T>(xs: List<T>) -> Option<T> {{ first(rest(xs)) }}
         fn last<T>(xs: List<T>) -> Option<T> {{ if xs.len() < 2 {{ first(xs) }} else {{ last(rest(xs)) }} }}
         fn main() -> Str {{ second([1, 2, 3]).unwrap_or(0).to_str() + last([\"a\", \"b\", \"c\"]).unwrap_or(\"\") }}"
    );
    assert_eq!(shown(&src), "2c");
}

#[test]
fn opaque_values_are_stored_matched_and_returned() {
    let src = "fn or<T>(o: Option<T>, d: T) -> T { match o { Some(x) => x, None => d } }
               fn lookup<T>(m: Map<Str, T>, k: Str, d: T) -> T { m.get(k).unwrap_or(d) }
               fn same<T>(x: T) -> T { let y: T = x; match y { z => z } }
               fn wrap<T>(x: T) -> Result<List<T>, Str> { Ok([x, x]) }
               fn main() -> Str {
                 let mut m: Map<Str, Str> = Map::new();
                 m.insert(\"k\", \"true\");
                 or(Some(1), 2).to_str() + or(None, \"d\") + lookup(m, \"k\", \"false\")
                   + same(3).to_str() + wrap(4).unwrap_or([]).len().to_str()
               }";
    assert_eq!(shown(src), "1dtrue32");
}

/// `None`, `[]` and `Map::new()` take their type from a parameter another argument fixed.
#[test]
fn arguments_typed_by_another_argument() {
    let src = "fn or<T>(o: Option<T>, d: T) -> T { match o { Some(x) => x, None => d } }
               fn concat<T>(a: List<T>, b: List<T>) -> List<T> { let mut out = a; for x in b { out.push(x); } out }
               fn size<T>(m: Map<Str, T>, d: T) -> Int { m.len() }
               fn main() -> Int { or(None, 5) + concat([], [1, 2]).len() + size(Map::new(), true) }";
    assert_eq!(shown(src), "7");
}

#[test]
fn closures_and_function_values() {
    let src = format!(
        "{FIRST}
         fn apply<T, U>(f: fn(T) -> U, x: T) -> U {{ f(x) }}
         fn map<T, U>(xs: List<T>, f: fn(T) -> U) -> List<U> {{
           let mut out: List<U> = [];
           for x in xs {{ out.push(f(x)); }}
           out
         }}
         fn twice<T>(x: T, f: fn(T) -> T) -> T {{ let g = |y: T| f(f(y)); g(x) }}
         fn main() -> Str {{
           let k = 10;
           let f: fn(List<Int>) -> Option<Int> = first;
           let g = first::<Str>;
           apply(|x| x + k, 1).to_str()
             + apply(|s: Str| s.len(), \"abc\").to_str()
             + map([1, 2], |x| x * 2)[1].to_str()
             + twice(3, |x| x * x).to_str()
             + f([4]).unwrap_or(0).to_str()
             + g([\"g\"]).unwrap_or(\"\")
             + apply(first, [5]).unwrap_or(0).to_str()
             + apply(first::<Int>, [6]).unwrap_or(0).to_str()
         }}"
    );
    assert_eq!(shown(&src), "1134814g56");
}

#[test]
fn generic_function_values_display_their_instance() {
    let src = format!("{FIRST} fn main() -> fn(List<Str>) -> Option<Str> {{ first }}");
    assert_eq!(shown(&src), "<fn first<Str>>");
}

#[test]
fn generic_functions_in_modules() {
    let root = "mod util;
                fn main() -> Int { util::first([3]).unwrap_or(0) + util::first::<Int>([4]).unwrap_or(0) + util::size(util::id([1])) }";
    let util = "pub fn first<T>(xs: List<T>) -> Option<T> { if xs.len() == 0 { None } else { Some(id(xs[0])) } }
                pub fn id<T>(x: T) -> T { x }
                pub fn size<T>(xs: List<T>) -> Int { xs.len() }";
    let p = check_files(&[("main", root), ("util", util)], &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let v = common::big_stack(move || p.display(&run(&p, &mut NoCaps).unwrap()));
    assert_eq!(v, "8");
    let private = "fn first<T>(xs: List<T>) -> Option<T> { None }";
    let diags = check_files(&[("main", root), ("util", private)], &CapTable::default()).unwrap_err();
    assert_eq!(diags[0].msg, "`first` is private to `util`");
}

#[test]
fn conflicting_type_arguments() {
    let src = "fn same<T>(a: T, b: T) -> T { a } fn main() { same(1, \"a\"); }";
    let (msg, at, hint) = reject(src);
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`T` is Int here but Str at argument 2", "1")
    );
    assert_eq!(hint, "`same` is declared as `fn same<T>(a: T, b: T)`");
    let src = "fn get<T>(xs: List<T>) -> Int { 0 } fn main() { get(5); }";
    let (msg, at, _) = reject(src);
    assert_eq!((msg.as_str(), at.as_str()), ("expected List<T>, found Int", "5"));
}

#[test]
fn cannot_infer() {
    let (msg, at, hint) = reject("fn empty<T>() -> List<T> { [] } fn main() { let n = empty().len(); }");
    assert_eq!((msg.as_str(), at.as_str()), ("cannot infer `T`", "empty()"));
    assert_eq!(hint, "write `empty::<Int>(..)`");
    let (msg, at, hint) = reject(&format!("{FIRST} fn main() {{ let f = first; }}"));
    assert_eq!(
        (msg.as_str(), at.as_str(), hint.as_str()),
        ("cannot infer `T`", "first", "write `first::<Int>`")
    );
    let src = "fn call<T, U>(f: fn(T) -> U) -> U { loop {} } fn main() { call(|x| x); }";
    let (msg, _, hint) = reject(src);
    assert_eq!(
        (msg.as_str(), hint.as_str()),
        ("cannot infer `T`", "write `call::<Int, Int>(..)`")
    );
    let src = "fn or<T>(o: Option<T>) -> Int { 0 } fn main() { or(None); }";
    assert_eq!(reject(src).0, "cannot infer `T`");
    // `!` fixes nothing.
    let src = "fn id<T>(x: T) -> T { x } fn main() { id({ return; }); }";
    assert_eq!(reject(src).0, "cannot infer `T`");
    assert_eq!(
        shown("fn id<T>(x: T) -> T { x } fn main() -> Int { id({ return 1; }) }"),
        "1"
    );
    // A generic function as an argument whose parameter types are not known yet.
    let src = "fn f<T>(g: fn(T) -> T) -> Int { 0 } fn id<U>(x: U) -> U { x } fn main() -> Int { f(id) }";
    assert_eq!(reject(src).0, "cannot infer `T`");
}

/// A generic function as a value takes its type arguments from the expected `fn` type, which
/// it must then match as a whole.
#[test]
fn function_values_from_the_expected_type() {
    let src = "fn id<T>(x: T) -> T { x }
               fn ap<T>(f: fn(T) -> T, x: T) -> T { f(x) }
               fn main() -> Int { let g: fn(Int) -> Int = id; g(3) + id::<Int>(4) + ap(id, 7) }";
    assert_eq!(shown(src), "14");
    let src = "fn id<T>(x: T) -> T { x } fn main() { let g: fn(Int) -> Str = id; }";
    let (msg, at, _) = reject(src);
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("expected fn(Int) -> Str, found fn(Int) -> Int", "id")
    );
}

#[test]
fn turbofish_errors() {
    let (msg, at, hint) = reject(&format!("{FIRST} fn main() {{ first::<Int, Str>([1]); }}"));
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("`first` takes 1 type arguments, found 2", "first::<Int, Str>")
    );
    assert_eq!(hint, "`first` is declared as `fn first<T>(xs: List<T>)`");
    let (msg, _, hint) = reject("fn double(x: Int) -> Int { x * 2 } fn main() { double::<Int>(1); }");
    assert_eq!(
        (msg.as_str(), hint.as_str()),
        ("`double` is not generic", "remove `::<..>`: write `double`")
    );
    let (msg, at, _) = reject(&format!("{FIRST} fn main() {{ first::<Str>([1]); }}"));
    assert_eq!((msg.as_str(), at.as_str()), ("expected Str, found Int", "1"));
    let (msg, at, _) = reject("fn id<T>(x: T) -> T { x } fn main() { id::<Str>(1); }");
    assert_eq!((msg.as_str(), at.as_str()), ("expected Str, found Int", "1"));
    let (msg, _, hint) = reject(&format!("{FIRST} fn main() {{ let first = 1; first::<Int>([1]); }}"));
    assert_eq!(
        (msg.as_str(), hint.as_str()),
        ("the local `first` hides the function `first`", "rename the local")
    );
}

/// Rule 1: a type parameter is never a capability, however it would be fixed.
#[test]
fn type_parameters_are_never_capabilities() {
    let id = "fn id<T>(x: T) -> T { x }";
    let cases = [
        (
            format!("{id} fn main(d: Dir) {{ id(d); }}"),
            "`T` cannot be `Dir`: capabilities are parameters only",
            "d",
        ),
        (
            format!("{id} fn main(d: Dir) {{ id::<Dir>(d); }}"),
            "`T` cannot be `Dir`: capabilities are parameters only",
            "Dir",
        ),
        (
            format!("{id} fn main() {{ let f = id::<Option<Dir>>; }}"),
            "`T` cannot be `Option<Dir>`: capabilities are parameters only",
            "Option<Dir>",
        ),
        (
            "fn mk<T>() -> T { loop {} } fn take(d: Dir) {} fn main() { take(mk()); }".to_string(),
            "`T` cannot be `Dir`: capabilities are parameters only",
            "mk()",
        ),
        (
            "fn wrap<T>(x: T) -> List<T> { [x] } fn main(d: Dir) { wrap(d); }".to_string(),
            "`T` cannot be `Dir`: capabilities are parameters only",
            "d",
        ),
    ];
    for (src, msg, at) in cases {
        let (m, a, hint) = reject(&src);
        assert_eq!((m.as_str(), a.as_str()), (msg, at), "{src}");
        assert_eq!(
            hint, "pass the Dir to a function whose parameter has type `Dir`",
            "{src}"
        );
    }
    // A generic function takes capabilities with concrete types, charged as any function's.
    let src = "fn keep<T>(d: Dir, x: T) -> T { let _r = d.read(\"a\"); x }
               fn main(input: Dir) -> Int { keep(input, 1) + keep::<Int>(input, 2) }";
    let p = check(src, &common::caps()).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(report(&p), "keep : d read\nmain : input read");
    let (msg, _, _) = reject("fn keep<T>(d: Dir, x: T) -> T { x } fn main() { let f = keep::<Int>; }");
    assert_eq!(msg, "function values cannot take or return capabilities");
}

#[test]
fn type_parameters_are_opaque() {
    let cases = [
        (
            "fn f<T>(x: T) -> Int { x.len() }",
            "no method `len` on T",
            "`T` is a type parameter, which has no methods; take a function for it, like `f: fn(T) -> Str`",
        ),
        (
            "fn f<T>(x: T) -> T { x + x }",
            "`+` cannot be applied to T and T",
            "`T` is a type parameter, which has no operators; take a function for it, like `add: fn(T, T) -> T`",
        ),
        (
            "fn f<T>(x: T, y: T) -> Bool { x == y }",
            "cannot compare `T` values",
            "add the bound: `T: Eq`",
        ),
        (
            "fn f<T>(xs: List<T>, y: T) -> Bool { xs.contains(y) }",
            "cannot compare `T` values",
            "add the bound: `T: Eq`",
        ),
        (
            "fn f<T>(xs: List<T>) -> Bool { xs == xs }",
            "cannot compare `List<T>` values",
            "add the bound: `T: Eq`",
        ),
        (
            "fn f<T>(x: T) -> Int { x.n }",
            "no field `n` on T",
            "only structs have fields",
        ),
        (
            "fn f<T>(x: T) -> T { match x { 1 => x, _ => x } }",
            "expected T, found Int",
            "",
        ),
        ("fn f<T>() -> T { 1 }", "expected T, found Int", ""),
        (
            "fn f<T>(m: Map<T, Int>) {}",
            "`T` cannot be a Map key",
            "Map keys must be Int or Str",
        ),
    ];
    for (src, msg, hint) in cases {
        let src = format!("{src} fn main() {{}}");
        let (m, _, h) = reject(&src);
        assert_eq!((m.as_str(), h.as_str()), (msg, hint), "{src}");
    }
}

#[test]
fn type_parameter_declarations() {
    let cases = [
        (
            "fn main<T>() {}",
            "`main` cannot be generic",
            "remove `<..>`: `main` takes only capabilities",
            "T",
        ),
        (
            "fn f<t>(x: t) {} fn main() {}",
            "type parameters start with an uppercase letter: `t`",
            "rename it `T`",
            "t",
        ),
        (
            "fn f<T, T>() {} fn main() {}",
            "duplicate type parameter `T`",
            "rename one of them",
            "T",
        ),
        (
            "fn f<Int>() {} fn main() {}",
            "`Int` is a built-in type",
            "name type parameters `T`, `U`, ..",
            "Int",
        ),
        (
            "fn f<Dir>(d: Dir) {} fn main() {}",
            "`Dir` is a built-in type",
            "name type parameters `T`, `U`, ..",
            "Dir",
        ),
        (
            "struct T { n: Int } fn f<T>(x: T) {} fn main() {}",
            "type parameter `T` has the name of struct `T`",
            "rename the type parameter",
            "T",
        ),
        (
            "fn f<T>(x: T<Int>) {} fn main() {}",
            "type parameter `T` takes no type arguments",
            "write `T`",
            "T<Int>",
        ),
    ];
    for (src, msg, hint, at) in cases {
        let (m, a, h) = reject(src);
        assert_eq!((m.as_str(), h.as_str()), (msg, hint), "{src}");
        assert_eq!(a, at, "{src}");
    }
    // A type parameter is only in scope in its own function.
    assert_eq!(
        reject("fn f<T>(x: T) {} fn g(x: T) {} fn main() {}").0,
        "unknown type `T`"
    );
}

/// `f<T>` calling `f<List<T>>` would make instances without end; the checker stops at 64.
#[test]
fn instantiation_without_end() {
    let src = "fn grow<T>(x: T, n: Int) -> Int { if n == 0 { 0 } else { grow([x], n - 1) } }
               fn main() -> Int { grow(1, 3) }";
    let (msg, at, hint) = reject(src);
    assert_eq!(
        (msg.as_str(), at.as_str()),
        ("this generic function instantiates without end", "grow")
    );
    assert_eq!(
        hint,
        "each call gives `grow` larger type arguments; keep them the same in calls inside `grow`"
    );
    // Doubling types are refused by size long before 64 instances.
    let src = "fn dbl<T>(x: T, n: Int) -> Int { if n == 0 { 0 } else { let r: Result<T, T> = Ok(x); dbl(r, n - 1) } }
               fn main() -> Int { dbl(1, 3) }";
    assert_eq!(reject(src).0, "this generic function instantiates without end");
    // Through two functions, and only when instantiated at all.
    let src = "fn a<T>(x: T) -> Int { b(Some(x)) } fn b<T>(x: T) -> Int { a([x]) } fn main() -> Int { a(1) }";
    assert_eq!(reject(src).0, "this generic function instantiates without end");
    let src = "fn a<T>(x: T) -> Int { b(Some(x)) } fn b<T>(x: T) -> Int { a([x]) } fn main() -> Int { 0 }";
    assert!(check(src, &CapTable::default()).is_ok());
    // A fixed, bounded nesting is fine: 64 distinct instances.
    let mut src = "fn id<T>(x: T) -> T { x } fn main() { let x0 = 0;".to_string();
    for i in 1..64 {
        src += &format!(" let x{i} = id([x{}]);", i - 1);
    }
    src += " }";
    assert!(check(&src, &CapTable::default()).is_ok());
}
