//! `HashMap<K, V>` (L10): every `Map` test of eval.rs and check.rs mirrored, then what differs
//! from `Map`: insertion order, order-insensitive `==`, keys built to collide.

mod common;

use lang::{check, run, CapTable, Caps, RunErr, Value};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

fn exec(src: &str) -> Result<Value, RunErr> {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || run(&p, &mut NoCaps))
}

fn int(src: &str) -> Value {
    exec(src).unwrap()
}

fn shown(src: &str) -> String {
    let p = check(src, &CapTable::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let v = run(&p, &mut NoCaps).unwrap();
    p.display(&v)
}

fn strs(xs: &[&str]) -> Value {
    Value::List(xs.iter().map(|s| Value::Str((*s).into())).collect::<Vec<_>>().into())
}

/// Checks `src` and returns the single diagnostic's text, hint and the source it points at.
fn reject_with(src: &str, caps: &CapTable) -> (String, String, String) {
    let diags = check(src, caps).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    (
        d.msg.clone(),
        d.hint.clone().unwrap_or_default(),
        src[d.span.start..d.span.end].to_string(),
    )
}

fn reject(src: &str) -> (String, String) {
    let (msg, _, at) = reject_with(src, &CapTable::default());
    (msg, at)
}

fn hint(src: &str) -> String {
    reject_with(src, &CapTable::default()).1
}

fn ok(src: &str) {
    if let Err(d) = check(src, &CapTable::default()) {
        panic!("{d:?}");
    }
}

#[test]
fn word_count() {
    let src = "fn count(words: List<Str>) -> HashMap<Str, Int> {
                   let mut m: HashMap<Str, Int> = HashMap::new();
                   for w in words {
                       let n = match m.get(w) { Some(n) => n, None => 0 };
                       m.insert(w, n + 1);
                   }
                   m
               }
               fn main() -> Int {
                   let m = count([\"b\", \"a\", \"b\", \"b\"]);
                   let a = match m.get(\"a\") { Some(n) => n, None => 0 };
                   let b = match m.get(\"b\") { Some(n) => n, None => 0 };
                   a * 10 + b * 100 + m.len()
               }";
    assert_eq!(int(src), Value::Int(10 + 300 + 2));
}

#[test]
fn keys_are_in_insertion_order() {
    let src = "fn main() -> List<Str> {
                   let mut m: HashMap<Str, Int> = HashMap::new();
                   m.insert(\"b\", 1); m.insert(\"a\", 2); m.insert(\"c\", 3);
                   m.keys()
               }";
    assert_eq!(int(src), strs(&["b", "a", "c"]));
    let src = "fn main() -> List<Int> {
                   let mut m: HashMap<Int, Str> = HashMap::new();
                   m.insert(3, \"x\"); m.insert(-1, \"y\"); m.insert(2, \"z\");
                   m.keys()
               }";
    assert_eq!(int(src), Value::List([3, -1, 2].map(Value::Int).to_vec().into()));
}

/// A replaced value keeps its key's place; a removed key loses it; re-inserting appends.
#[test]
fn order_after_removes_and_reinserts() {
    let src = "fn main() -> List<Str> {
                   let mut m: HashMap<Str, Int> = HashMap::new();
                   m.insert(\"a\", 1); m.insert(\"b\", 2); m.insert(\"c\", 3); m.insert(\"d\", 4);
                   m.remove(\"b\");
                   m.insert(\"a\", 10);
                   m.insert(\"b\", 20);
                   m.remove(\"c\");
                   m.remove(\"zz\");
                   m.insert(\"c\", 30);
                   m.keys()
               }";
    assert_eq!(int(src), strs(&["a", "d", "b", "c"]));
    // Many removes compact the entries; the order stays.
    let src = "fn main() -> HashMap<Int, Int> {
                   let mut m: HashMap<Int, Int> = HashMap::new();
                   for i in 0..100 { m.insert(i, i); }
                   for i in 0..97 { m.remove(i); }
                   m.insert(5, 5);
                   m.insert(98, -1);
                   m
               }";
    assert_eq!(shown(src), "{97: 97, 98: -1, 99: 99, 5: 5}");
}

#[test]
fn get_insert_remove() {
    let src = "fn main() -> Int {
                   let mut m: HashMap<Int, Int> = HashMap::new();
                   m.insert(1, 10);
                   m.insert(1, 20);
                   m.insert(2, 5);
                   m.remove(2);
                   m.remove(9);
                   match m.get(1) { Some(v) => v + m.len() * 1000, None => 0 }
               }";
    assert_eq!(int(src), Value::Int(1020));
    let src = "fn main() -> Bool { let m: HashMap<Str, Int> = HashMap::new(); match m.get(\"x\") { Some(_) => false, None => true } }";
    assert_eq!(int(src), Value::Bool(true));
    let src = "fn main() -> Str { let m: HashMap<Str, Str> = HashMap::new(); m.get(\"k\").unwrap_or(\"d\") }";
    assert_eq!(int(src), Value::Str("d".into()));
}

#[test]
fn in_nested_places() {
    let src = "struct S { m: HashMap<Str, List<Int>> }
               fn main() -> Int {
                   let mut s = S { m: HashMap::new() };
                   s.m.insert(\"a\", [1]);
                   let mut xs = [s];
                   xs[0].m.insert(\"b\", []);
                   xs[0].m.remove(\"a\");
                   xs[0].m.len() * 10 + xs[0].m.keys().len()
               }";
    assert_eq!(int(src), Value::Int(11));
}

#[test]
fn hashmaps_are_copied() {
    let src = "fn main() -> Int {
                   let mut a: HashMap<Int, Int> = HashMap::new();
                   a.insert(1, 1);
                   let mut b = a;
                   b.insert(2, 2);
                   a.insert(1, 7);
                   let x = match a.get(1) { Some(v) => v, None => 0 };
                   let y = match b.get(1) { Some(v) => v, None => 0 };
                   x * 100 + y * 10 + a.len() * 1000 + b.len() * 10000
               }";
    assert_eq!(int(src), Value::Int(700 + 10 + 1000 + 20000));
    // A value read with `get` is a copy.
    let src = "fn main() -> Int {
                   let mut m: HashMap<Int, List<Int>> = HashMap::new();
                   m.insert(1, [1]);
                   let mut xs: List<Int> = match m.get(1) { Some(v) => v, None => [] };
                   xs.push(2);
                   xs[0] = 9;
                   match m.get(1) { Some(v) => v[0] * 10 + v.len(), None => 0 }
               }";
    assert_eq!(int(src), Value::Int(11));
    let src = "fn put(m: HashMap<Int, Int>) -> Int { let mut n = m; n.insert(5, 5); n.len() }
               fn main() -> Int { let m: HashMap<Int, Int> = HashMap::new(); put(m) * 10 + m.len() }";
    assert_eq!(int(src), Value::Int(10));
}

/// Copy-on-write through a callee: the caller's map is unchanged, the callee's result has
/// both changes, in order.
#[test]
fn insert_and_remove_in_callee() {
    let src = "fn change(m: HashMap<Int, Int>) -> HashMap<Int, Int> {
                   let mut n = m;
                   n.insert(1, 7);
                   n.insert(2, 8);
                   n.remove(3);
                   n
               }
               fn main() -> Int {
                   let mut a: HashMap<Int, Int> = HashMap::new();
                   a.insert(1, 1);
                   a.insert(3, 3);
                   let b = change(a);
                   a.len() * 1000 + a.get(1).unwrap_or(0) * 100 + b.len() * 10 + b.get(1).unwrap_or(0)
               }";
    assert_eq!(int(src), Value::Int(2000 + 100 + 20 + 7));
    let src = "fn grow(m: HashMap<Str, List<Int>>) -> HashMap<Str, List<Int>> {
                   let mut n = m;
                   n.insert(\"z\", [9]);
                   n.remove(\"a\");
                   n.insert(\"a\", [8]);
                   n
               }
               fn main() -> List<HashMap<Str, List<Int>>> {
                   let mut a: HashMap<Str, List<Int>> = HashMap::new();
                   a.insert(\"a\", [1]);
                   a.insert(\"b\", []);
                   [grow(a), a]
               }";
    assert_eq!(
        shown(src),
        "[{\"b\": [], \"z\": [9], \"a\": [8]}, {\"a\": [1], \"b\": []}]"
    );
}

#[test]
fn equality_ignores_order() {
    let src = "fn main() -> Bool {
        let mut a: HashMap<Str, Int> = HashMap::new();
        let mut b: HashMap<Str, Int> = HashMap::new();
        a.insert(\"k\", 1);
        b.insert(\"k\", 1);
        a == b
    }";
    assert_eq!(int(src), Value::Bool(true));
    let src = "fn main() -> List<Bool> {
        let mut a: HashMap<Int, List<Int>> = HashMap::new();
        let mut b: HashMap<Int, List<Int>> = HashMap::new();
        a.insert(1, [1]); a.insert(2, []); a.insert(3, [3]);
        b.insert(3, [3]); b.insert(2, []); b.insert(1, [1]);
        let same = a == b;
        let mut c = b;
        c.insert(2, [0]);
        let mut d = b;
        d.remove(3);
        let mut e = d;
        e.insert(4, [3]);
        [same, a != b, a == c, a == d, a == e, d == d]
    }";
    assert_eq!(shown(src), "[true, false, false, false, false, true]");
}

#[test]
fn display() {
    let src = "fn main() -> HashMap<Str, Int> {
                   let mut m: HashMap<Str, Int> = HashMap::new();
                   m.insert(\"b\", 1); m.insert(\"a\\\"\", 2);
                   m
               }";
    assert_eq!(shown(src), "{\"b\": 1, \"a\\\"\": 2}");
    assert_eq!(shown("fn main() -> HashMap<Int, Str> { HashMap::new() }"), "{}");
    let src = "fn main() -> HashMap<Str, List<Float>> {
                   let mut m: HashMap<Str, List<Float>> = HashMap::new();
                   m.insert(\"a\", [1.0 / 0.0, 0.0 / 0.0, 2.5]);
                   m
               }";
    assert_eq!(shown(src), "{\"a\": [inf, NaN, 2.5]}");
    let src = "struct P { m: HashMap<Int, Option<Str>> }
               fn main() -> P {
                   let mut m: HashMap<Int, Option<Str>> = HashMap::new();
                   m.insert(2, None); m.insert(-1, Some(\"y\\n\"));
                   P { m: m }
               }";
    assert_eq!(shown(src), "P { m: {2: None, -1: Some(\"y\\n\")} }");
}

#[test]
fn assignment_of_untyped_values() {
    let src = "fn main() -> Int {
                   let mut m: HashMap<Str, Int> = HashMap::new();
                   m.insert(\"a\", 1);
                   m = HashMap::new();
                   m.len()
               }";
    assert_eq!(int(src), Value::Int(0));
    ok("fn main() { let mut m: HashMap<Str, Int> = HashMap::new(); m = HashMap::new(); }");
}

#[test]
fn generic_functions_and_types() {
    let src = "fn lookup<T>(m: HashMap<Str, T>, k: Str, d: T) -> T { m.get(k).unwrap_or(d) }
               fn size<T>(m: HashMap<Str, T>, d: T) -> Int { m.len() }
               struct Index<T> { by: HashMap<Int, T> }
               fn main() -> Str {
                 let mut m: HashMap<Str, Str> = HashMap::new();
                 m.insert(\"k\", \"v\");
                 let mut i: Index<Bool> = Index { by: HashMap::new() };
                 i.by.insert(7, true);
                 lookup(m, \"k\", \"d\") + size(HashMap::new(), true).to_str() + i.by.len().to_str()
               }";
    assert_eq!(shown(src), "v01");
    let (msg, at) = reject("fn f<T>(m: HashMap<T, Int>) {} fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("`T` cannot be a HashMap key", "T"));
    assert_eq!(
        hint("fn f<T>(m: HashMap<T, Int>) {} fn main() {}"),
        "HashMap keys must be Int or Str"
    );
    // A type may hold itself through a `HashMap`.
    ok("struct Tree { kids: HashMap<Str, Tree> } fn main() {}");
    ok("fn main() -> Bool { let a: HashMap<Str, List<Int>> = HashMap::new(); a == a }");
}

#[test]
fn types_and_new() {
    ok("fn f(m: HashMap<Str, List<Int>>) -> Int { m.len() } fn main() { let m: HashMap<Int, Int> = HashMap::new(); }");
    ok("fn main() -> HashMap<Str, Int> { HashMap::new() }");
    let src = "fn main() { let m = HashMap::new(); }";
    assert_eq!(
        reject(src),
        (
            "cannot tell the type of `HashMap::new()`".into(),
            "HashMap::new()".into()
        )
    );
    assert_eq!(hint(src), "add a type: `let m: HashMap<Str, Int> = HashMap::new();`");
    let src = "fn main() { let x: Int = HashMap::new(); }";
    assert_eq!(
        reject(src),
        ("expected Int, found a HashMap".into(), "HashMap::new()".into())
    );
    assert_eq!(hint(src), "`HashMap::new()` makes a `HashMap<K, V>`");
    let src = "fn main() { let m: HashMap<Str, Int> = HashMap::new(1); }";
    assert_eq!(reject(src).0, "`HashMap::new` takes 0 arguments, found 1");
    assert_eq!(hint(src), "write `HashMap::new()`");
    for src in ["fn main() { HashMap::get(1); }", "fn main() { let x = HashMap::new; }"] {
        assert_eq!(hint(src), "the only HashMap function is `HashMap::new()`", "{src}");
    }
    assert_eq!(hint("fn f(m: HashMap<Int>) {} fn main() {}"), "write `HashMap<K, V>`");
    assert!(hint("fn f(x: Foo) {} fn main() {}").contains("HashMap"));
    assert_eq!(
        reject("struct HashMap { a: Int } fn main() {}").0,
        "`HashMap` is a built-in type"
    );
}

/// `HashMap` and `Map` are different types: no conversion either way.
#[test]
fn hashmap_is_not_map() {
    let src = "fn main() { let m: Map<Str, Int> = HashMap::new(); }";
    assert_eq!(reject(src).0, "expected Map<Str, Int>, found a HashMap");
    let src = "fn main() { let m: HashMap<Str, Int> = Map::new(); }";
    assert_eq!(reject(src).0, "expected HashMap<Str, Int>, found a Map");
    let src = "fn f(m: Map<Str, Int>) {} fn main() { let m: HashMap<Str, Int> = HashMap::new(); f(m); }";
    assert_eq!(
        reject(src),
        ("expected Map<Str, Int>, found HashMap<Str, Int>".into(), "m".into())
    );
    let src =
        "fn main() -> Bool { let a: Map<Int, Int> = Map::new(); let b: HashMap<Int, Int> = HashMap::new(); a == b }";
    assert_eq!(
        reject(src).0,
        "`==` cannot be applied to Map<Int, Int> and HashMap<Int, Int>"
    );
}

#[test]
fn keys_must_be_int_or_str() {
    for src in [
        "fn f(m: HashMap<Bool, Int>) {} fn main() {}",
        "struct K { a: Int } fn f(m: HashMap<K, Int>) {} fn main() {}",
        "fn f(m: HashMap<List<Int>, Int>) {} fn main() {}",
        "fn f(m: HashMap<fn(Int), Int>) {} fn main() {}",
        "fn main() { let m: HashMap<Float, Int> = HashMap::new(); }",
    ] {
        assert!(reject(src).0.ends_with("cannot be a HashMap key"), "{src}");
        assert_eq!(hint(src), "HashMap keys must be Int or Str", "{src}");
    }
    let (msg, at) = reject("fn f(m: HashMap<Bool, Int>) {} fn main() {}");
    assert_eq!((msg.as_str(), at.as_str()), ("`Bool` cannot be a HashMap key", "Bool"));
    ok("fn f(m: HashMap<Int, HashMap<Str, Int>>, n: Map<Int, HashMap<Int, Int>>) {} fn main() {}");
}

#[test]
fn methods() {
    let m = "fn main() { let mut m: HashMap<Str, Int> = HashMap::new(); ";
    ok(&format!(
        "{m}m.insert(\"a\", 1); m.remove(\"a\"); let n: Int = m.len(); let k: List<Str> = m.keys(); let g: Option<Int> = m.get(\"a\"); }}"
    ));
    ok("struct S { m: HashMap<Int, Int> } fn main() { let mut s = S { m: HashMap::new() }; s.m.insert(1, 2); }");
    ok("fn main() { let xs: List<HashMap<Int, Int>> = [HashMap::new()]; }");
    let src = format!("{m}m.insert(1, 1); }}");
    assert_eq!(reject(&src).0, "expected Str, found Int");
    let src = format!("{m}m.insert(\"a\", \"b\"); }}");
    assert_eq!(reject(&src).0, "expected Int, found Str");
    let src = format!("{m}m.get(1); }}");
    assert_eq!(reject(&src).0, "expected Str, found Int");
    let src = format!("{m}m.insert(\"a\"); }}");
    assert_eq!(reject(&src).0, "`insert` takes 2 arguments, found 1");
    assert_eq!(
        hint(&src),
        "`HashMap<Str, Int>.insert` is declared as `insert(Str, Int) -> ()`"
    );
    assert_eq!(hint(&format!("{m}m.inser(\"a\", 1); }}")), "did you mean `insert`?");
    assert_eq!(
        hint(&format!("{m}m.push(1); }}")),
        "HashMap<Str, Int> has: get, insert, keys, len, remove"
    );
    let src = format!("{m}let x: Int = m.get(\"a\"); }}");
    assert_eq!(reject(&src).0, "expected Int, found Option<Int>");
    ok(&format!("{m}m == m; }}"));
}

#[test]
fn insert_and_remove_need_a_mut_place() {
    let f = "fn f() -> HashMap<Int, Int> { HashMap::new() } ";
    for (call, verb, to) in [("insert(1, 2)", "insert", "to"), ("remove(1)", "remove", "from")] {
        let src = format!("{f}fn main() {{ f().{call}; }}");
        let (msg, at) = reject(&src);
        assert_eq!(
            (msg, at.as_str()),
            (format!("cannot `{verb}` {to} this expression"), "f()")
        );
        assert_eq!(
            hint(&src),
            format!("`{verb}` changes a map; call it on a `let mut` variable")
        );
        let src = format!("fn main() {{ let m: HashMap<Int, Int> = HashMap::new(); m.{call}; }}");
        assert_eq!(reject(&src).0, format!("cannot `{verb}` {to} `m`, it is not `mut`"));
        assert_eq!(hint(&src), "declare it with `let mut`");
    }
}

#[test]
fn cannot_hold_capabilities() {
    let caps = common::caps();
    let src = "fn f(m: HashMap<Str, Dir>) {} fn main(d: Dir) {}";
    assert_eq!(
        reject_with(src, &caps),
        (
            "capability nested in `HashMap<Str, Dir>`".into(),
            "take `Dir` as a parameter".into(),
            "HashMap<Str, Dir>".into()
        )
    );
    let src = "fn main(d: Dir) { let m: HashMap<Str, Dir> = HashMap::new(); }";
    assert_eq!(reject_with(src, &caps).0, "capability nested in `HashMap<Str, Dir>`");
    let src = "fn f<T>(x: T) -> HashMap<Str, T> { let mut m: HashMap<Str, T> = HashMap::new(); m.insert(\"a\", x); m } fn main(d: Dir) { f(d); }";
    assert!(reject_with(src, &caps).0.contains("cannot be `Dir`"));
}

/// 100,000 `Str` keys with one hash (`common::collide`), inserted and looked up. Every key
/// lands in one bucket, which is sorted past 8 entries, so this takes about a second in a debug
/// build; with chains it did not finish in 100 s (checked by raising `TREEIFY`: 200,000
/// operations each walking up to 100,000 entries).
#[test]
fn colliding_keys_stay_fast() {
    let src = common::collide::colliding_program(317);
    let t = std::time::Instant::now();
    assert_eq!(int(&src), Value::Int(common::collide::colliding_result(317)));
    assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
}
