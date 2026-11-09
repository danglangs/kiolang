//! L7c: bounds `T: Eq` and `T: Ord` on type parameters of functions, structs and enums.
mod common;

use lang::{check, run, CapTable, Caps, Value};

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
    let diags = check(src, &common::caps()).expect_err(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    let hint = d.hint.clone().unwrap_or_default();
    (d.msg.clone(), src[d.span.start..d.span.end].to_string(), hint)
}

const DEDUP: &str = "fn dedup<T: Eq>(xs: List<T>) -> List<T> {
                       let mut out: List<T> = [];
                       for x in xs { if !out.contains(x) { out.push(x); } }
                       out
                     }";
const MAX: &str = "fn max<T: Ord>(xs: List<T>) -> Option<T> {
                     if xs.len() == 0 { return None; }
                     let mut m = xs[0];
                     for x in xs { if m < x { m = x; } }
                     Some(m)
                   }";
const SET: &str = "struct Set<T: Ord> { items: List<T> }
                   fn insert<T: Ord>(s: Set<T>, x: T) -> Set<T> {
                     let mut out: List<T> = [];
                     let mut done = false;
                     for y in s.items {
                       if !done && x <= y { if x != y { out.push(x); } done = true; }
                       out.push(y);
                     }
                     if !done { out.push(x); }
                     Set { items: out }
                   }
                   fn has<T: Ord>(s: Set<T>, x: T) -> Bool { s.items.contains(x) }";

#[test]
fn eq_bound_allows_equality_and_contains() {
    let src = format!(
        "{DEDUP}
         struct P {{ x: Int }}
         fn same<T: Eq>(a: List<T>, b: List<T>) -> Bool {{ a == b && !(a != b) }}
         fn yes(x: Bool) -> Str {{ if x {{ \"true\" }} else {{ \"false\" }} }}
         fn main() -> Str {{
           let a = dedup([1, 2, 1, 3, 2]);
           let b = dedup([\"x\", \"y\", \"x\"]);
           let c = dedup([P {{ x: 1 }}, P {{ x: 1 }}, P {{ x: 2 }}]);
           let d = dedup([Some(1), None, Some(1), None]);
           let e = dedup([0.5, 0.5, 1.5]);
           a.len().to_str() + b.len().to_str() + c.len().to_str() + d.len().to_str()
             + e.len().to_str() + yes(same(a, [1, 2, 3])) + yes(same(b, [\"y\"]))
         }}"
    );
    assert_eq!(shown(&src), "32222truefalse");
}

#[test]
fn ord_bound_allows_order_sort_and_equality() {
    let src = format!(
        "{MAX}
         fn sorted<T: Ord>(xs: List<T>) -> List<T> {{ let mut ys = xs; ys.sort(); ys }}
         fn le<T: Ord>(a: T, b: T) -> Bool {{ a <= b && !(a > b) && (a < b || a == b) && (b >= a) }}
         fn yes(x: Bool) -> Str {{ if x {{ \"true\" }} else {{ \"false\" }} }}
         fn main() -> Str {{
           let a = max([3, 9, 2]).unwrap_or(0);
           let b = max([\"pear\", \"apple\", \"zoo\", \"Zebra\"]).unwrap_or(\"\");
           let e: Option<Int> = max([]);
           let s = sorted([\"b\", \"a\", \"B\"]);
           a.to_str() + b + yes(e.is_none()) + s[0] + s[2] + yes(le(1, 2)) + yes(le(\"b\", \"a\"))
         }}"
    );
    assert_eq!(shown(&src), "9zootrueBbtruefalse");
}

#[test]
fn bounds_propagate_through_callers() {
    let src = format!(
        "{DEDUP}
         {MAX}
         fn uniq_sorted<U: Ord>(xs: List<U>) -> List<U> {{ let mut ys = dedup(xs); ys.sort(); ys }}
         fn top<U: Ord>(xs: List<U>, d: U) -> U {{ max(uniq_sorted(xs)).unwrap_or(d) }}
         fn pairs<U: Eq>(xs: List<U>) -> List<List<U>> {{ dedup([xs, xs]) }}
         fn yes(x: Bool) -> Str {{ if x {{ \"true\" }} else {{ \"false\" }} }}
         fn main() -> Str {{
           let a = uniq_sorted([3, 1, 3, 2, 1]);
           let f: fn(List<Str>, Str) -> Str = top;
           a.len().to_str() + a[0].to_str() + top([4, 8, 8], 0).to_str() + f([\"a\", \"c\"], \"\")
             + pairs([1.5]).len().to_str()
         }}"
    );
    assert_eq!(shown(&src), "318c1");
}

#[test]
fn sorted_set_type() {
    let src = format!(
        "{SET}
         enum Heap<T: Ord> {{ Empty, One(T) }}
         fn peek<T: Ord>(h: Heap<T>, d: T) -> T {{ match h {{ Heap::One(x) => x, Heap::Empty => d }} }}
         fn yes(x: Bool) -> Str {{ if x {{ \"true\" }} else {{ \"false\" }} }}
         fn main() -> Str {{
           let mut s: Set<Int> = Set {{ items: [] }};
           for x in [5, 1, 4, 1, 5, 9, 2, 6] {{ s = insert(s, x); }}
           let mut w: Set<Str> = Set {{ items: [] }};
           for x in [\"b\", \"a\", \"b\"] {{ w = insert(w, x); }}
           let h: Heap<Str> = Heap::Empty;
           s.items.len().to_str() + s.items[0].to_str() + s.items[5].to_str() + yes(has(s, 4))
             + yes(has(s, 3)) + w.items[0] + w.items.len().to_str() + peek(Heap::One(7), 0).to_str()
             + peek(h, \"-\") + yes(s == Set {{ items: [1, 2, 4, 5, 6, 9] }})
         }}"
    );
    assert_eq!(shown(&src), "619truefalsea27-true");
}

#[test]
fn type_arguments_must_satisfy_bounds() {
    let only = "only Int and Str are Ord";
    let cases: Vec<(String, &str, &str, &str)> = vec![
        (
            format!("{MAX} fn main() {{ max([1.5]); }}"),
            "`Float` is not `Ord`: `max` needs `T: Ord`",
            "[1.5]",
            only,
        ),
        (
            format!("{MAX} struct Point {{ x: Int }} fn main() {{ max([Point {{ x: 1 }}]); }}"),
            "`Point` is not `Ord`: `max` needs `T: Ord`",
            "[Point { x: 1 }]",
            only,
        ),
        (
            format!("{MAX} fn main() {{ max([[1]]); }}"),
            "`List<Int>` is not `Ord`: `max` needs `T: Ord`",
            "[[1]]",
            only,
        ),
        (
            format!("{MAX} fn main() {{ max::<Float>([]); }}"),
            "`Float` is not `Ord`: `max` needs `T: Ord`",
            "Float",
            only,
        ),
        (
            format!("{MAX} fn main() {{ let f: fn(List<Bool>) -> Option<Bool> = max; }}"),
            "`Bool` is not `Ord`: `max` needs `T: Ord`",
            "max",
            only,
        ),
        (
            format!("{DEDUP} fn inc(x: Int) -> Int {{ x }} fn main() {{ dedup([inc]); }}"),
            "`fn(Int) -> Int` is not `Eq`: `dedup` needs `T: Eq`",
            "[inc]",
            "functions have no equality",
        ),
        (
            format!("{DEDUP} struct H {{ f: fn(Int) -> Int }} fn inc(x: Int) -> Int {{ x }} fn main() {{ dedup([H {{ f: inc }}]); }}"),
            "`H` is not `Eq`: `dedup` needs `T: Eq`",
            "[H { f: inc }]",
            "functions have no equality",
        ),
        (
            format!("{MAX} fn g<U>(xs: List<U>) -> Option<U> {{ max(xs) }} fn main() {{}}"),
            "`U` is not `Ord`: `max` needs `T: Ord`",
            "xs",
            "add the bound: `U: Ord`",
        ),
        (
            format!("{MAX} fn g<U: Eq>(xs: List<U>) -> Option<U> {{ max(xs) }} fn main() {{}}"),
            "`U` is not `Ord`: `max` needs `T: Ord`",
            "xs",
            "add the bound: `U: Ord`",
        ),
        (
            format!("{DEDUP} fn g<U>(xs: List<U>) -> List<Option<U>> {{ dedup([]) }} fn main() {{}}"),
            "`Option<U>` is not `Eq`: `dedup` needs `T: Eq`",
            "dedup([])",
            "add the bound: `U: Eq`",
        ),
    ];
    for (src, msg, at, hint) in cases {
        let (m, a, h) = reject(&src);
        assert_eq!((m.as_str(), a.as_str(), h.as_str()), (msg, at, hint), "{src}");
    }
}

#[test]
fn type_bounds_are_checked_at_every_use() {
    let set = "struct Set<T: Ord> { items: List<T> }";
    let bag = "struct Bag<T: Eq> { items: List<T> }";
    let float = "`Float` is not `Ord`: `Set` needs `T: Ord`";
    let only = "only Int and Str are Ord";
    let cases: Vec<(String, &str, &str, &str)> = vec![
        (
            format!("{set} fn main() {{ let s: Set<Float> = Set {{ items: [] }}; }}"),
            float,
            "Float",
            only,
        ),
        (
            format!("{set} fn main() {{ let s = Set {{ items: [1.5] }}; }}"),
            float,
            "[1.5]",
            only,
        ),
        (
            format!("{set} fn f(s: Set<Bool>) {{}} fn main() {{}}"),
            "`Bool` is not `Ord`: `Set` needs `T: Ord`",
            "Bool",
            only,
        ),
        (
            format!("{set} fn f<U>(s: Set<U>) {{}} fn main() {{}}"),
            "`U` is not `Ord`: `Set` needs `T: Ord`",
            "U",
            "add the bound: `U: Ord`",
        ),
        (
            format!("{set} struct W<V> {{ s: Set<V> }} fn main() {{}}"),
            "`V` is not `Ord`: `Set` needs `T: Ord`",
            "V",
            "add the bound: `V: Ord`",
        ),
        (
            format!("{set} enum E {{ A(List<Set<Float>>) }} fn main() {{}}"),
            float,
            "Float",
            only,
        ),
        (
            format!("{bag} fn main() {{ let b: Bag<fn(Int) -> Int> = Bag {{ items: [] }}; }}"),
            "`fn(Int) -> Int` is not `Eq`: `Bag` needs `T: Eq`",
            "fn(Int) -> Int",
            "functions have no equality",
        ),
        (
            format!("{bag} fn f<U>(b: Bag<U>) {{}} fn main() {{}}"),
            "`U` is not `Eq`: `Bag` needs `T: Eq`",
            "U",
            "add the bound: `U: Eq`",
        ),
        (
            "enum Heap<T: Ord> { Empty, One(T) } fn main() { let h = Heap::One(1.5); }".to_string(),
            "`Float` is not `Ord`: `Heap` needs `T: Ord`",
            "1.5",
            only,
        ),
        (
            format!("{set} fn main() {{ let s = Set {{ items: [] }}; }}"),
            "cannot infer `T` for `Set { .. }`",
            "Set { items: [] }",
            "add a type: `let s: Set<Int> = Set { .. };`",
        ),
    ];
    for (src, msg, at, hint) in cases {
        let (m, a, h) = reject(&src);
        assert_eq!((m.as_str(), a.as_str(), h.as_str()), (msg, at, hint), "{src}");
    }
    // `Ord` satisfies `Eq`; a bounded parameter satisfies its own bound.
    let ok = [
        format!("{bag} fn f<U: Ord>(b: Bag<U>) -> Bag<U> {{ b }} fn main() {{}}"),
        format!("{set} {bag} struct W<V: Ord> {{ s: Set<V>, b: Bag<List<V>> }} fn main() {{}}"),
        format!("{bag} fn main() {{ let b: Bag<Option<Float>> = Bag {{ items: [] }}; }}"),
    ];
    for src in ok {
        check(&src, &CapTable::default()).unwrap_or_else(|d| panic!("{src}: {d:?}"));
    }
}

#[test]
fn bounds_inside_bodies() {
    let cases = [
        (
            "fn f<T: Eq>(x: T) -> Bool { x < x }",
            "`<` cannot be applied to T and T",
            "add the bound: `T: Ord`",
        ),
        (
            "fn f<T>(x: T) -> Bool { x >= x }",
            "`>=` cannot be applied to T and T",
            "add the bound: `T: Ord`",
        ),
        (
            "fn f<T: Eq>(xs: List<T>) { let mut ys = xs; ys.sort(); }",
            "cannot sort List<T>",
            "add the bound: `T: Ord`",
        ),
        (
            "fn f<T: Ord, U>(x: T, y: U) -> Bool { [Some(y)] == [Some(y)] }",
            "cannot compare `List<Option<U>>` values",
            "add the bound: `U: Eq`",
        ),
        (
            "fn f<T: Ord>(x: T, y: T) -> T { x + y }",
            "`+` cannot be applied to T and T",
            "`T` is a type parameter, which has no operators; take a function for it, like `add: fn(T, T) -> T`",
        ),
        (
            "fn f<T: Ord, U: Ord>(x: T, y: U) -> Bool { x < y }",
            "`<` cannot be applied to T and U",
            "`T` is a type parameter, which has no operators; take a function for it, like `add: fn(T, T) -> T`",
        ),
        (
            "fn f<T: Ord>(xs: List<T>) -> Int { xs.len() } fn main() { f(); }",
            "`f` takes 1 arguments, found 0",
            "`f` is declared as `fn f<T: Ord>(xs: List<T>)`",
        ),
    ];
    for (src, msg, hint) in cases {
        let src = if src.contains("fn main") {
            src.to_string()
        } else {
            format!("{src} fn main() {{}}")
        };
        let (m, _, h) = reject(&src);
        assert_eq!((m.as_str(), h.as_str()), (msg, hint), "{src}");
    }
}
