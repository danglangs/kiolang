//! C8c2: a struct of at most 4 fields is held as one Wasm value per field outside slots, boxed
//! on entry to a slot, a capture, `==`, display or a runtime operation. Every program runs
//! compiled and interpreted and must agree.

use kio_host::{Interp, Kio, Limits};

type Run = Result<String, (String, usize, usize)>;

fn limits() -> Limits {
    Limits {
        fuel: 20_000_000_000,
        ..Limits::default()
    }
}

fn compiled(src: &str) -> Run {
    match Kio::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn both(src: &str) -> Run {
    let c = compiled(src);
    assert_eq!(c, interpreted(src), "compiled vs interpreted: {src}");
    c
}

fn value(src: &str) -> String {
    both(src).unwrap_or_else(|e| panic!("{src}: {e:?}"))
}

fn error(src: &str) -> (String, usize, usize) {
    both(src).unwrap_err()
}

/// Every position of an unboxed struct: returned, passed owned and lent, stored in a list, a
/// map and a field, read back from each, captured, compared, assigned whole and by field
/// (`=`, `+=` on `Int`, `Float` and `Str`), nested places through a boxed field, matched,
/// iterated, popped, through `Option`/`Result`, passed to a closure and through `map`.
#[test]
fn unboxed_structs_in_every_position() {
    let src = r#"struct P { x: Int, y: Int }
        struct Q { name: Str, n: Int, ok: Bool, f: Float }
        struct U { u: (), s: Str }
        struct Big { a: Int, b: Int, c: Int, d: Int, e: Int }
        struct Nest { p: P, q: Q, o: Option<Str>, big: Big }
        fn mk(i: Int) -> P { P { x: i, y: i * 2 } }
        fn sum(p: P) -> Int { p.x + p.y }
        fn take(q: Q) -> Str { q.name + q.n.to_str() }
        fn pass(q: Q) -> Q { q }
        fn bump(q: Q) -> Q { let mut r = q; r.n += 1; r.name += "!"; r.f += 0.25; r }
        fn main() -> Str {
            let p = mk(3);
            let q = Q { name: "a" + "b", n: 1, ok: true, f: 1.5 };
            let q2 = bump(pass(q));
            let xs = [p, mk(1)];
            let mut m: Map<Str, P> = Map::new();
            m.insert("k", p);
            let got = m.get("k").unwrap_or(P { x: 0, y: 0 });
            let f = |d: Int| p.x + q2.n + d;
            let eq = if p == xs[0] && p != xs[1] && q != q2 && (Q { name: "ab", n: 1, ok: true, f: 1.5 } == q) { "eq" } else { "ne" };
            let mut n = Nest { p: p, q: q, o: Some("o" + ""), big: Big { a: 1, b: 2, c: 3, d: 4, e: 5 } };
            n.p.x = 10;
            n.q.name += "c";
            n.big.e += 1;
            let np = n.p;
            let total = sum(p) + sum(xs[1]) + got.y + f(1) + np.x + n.q.n + n.big.e;
            let mut s = P { x: 1, y: 2 };
            s.x += 5;
            s.y = s.x * 2;
            s = P { x: s.y, y: 0 };
            let u = U { u: (), s: "u" + "" };
            let matched = match Some(q2) { Some(v) => v.name + v.f.to_str(), None => "" };
            let mut acc = 0;
            for e in xs { acc += e.x; }
            let mut ys = [mk(7)];
            let popped = ys.pop().unwrap_or(P { x: -1, y: -1 });
            let r: Result<P, Str> = Ok(mk(4));
            let ok = r.unwrap_or(P { x: 0, y: 0 }).x;
            let mapped = xs.map(|e: P| mk(e.x + 1)).filter(|e: P| e.y > 2).len();
            let unit = if u.u == () { "()" } else { "" };
            take(q2) + eq + total.to_str() + "/" + s.x.to_str() + u.s + matched + acc.to_str() + popped.y.to_str()
                + ok.to_str() + n.o.unwrap_or("-") + n.q.name + mapped.to_str() + unit
        }"#;
    assert_eq!(value(src), "ab!2eq41/12uab!1.754144oabc2()");
}

/// A struct with pointer fields: moved into an owned parameter, lent to one read in place,
/// captured, moved by a last use, returned, and released on every exit (scope end, `return`,
/// `break`, `continue`, `?`); the live count is 0 after each program.
#[test]
fn pointer_fields_moved_borrowed_and_captured() {
    let src = r#"struct R { name: Str, xs: List<Int>, n: Int }
        fn own(r: R) -> List<Str> { [r.name] }
        fn lent(r: R) -> Int { r.name.len() + r.xs.len() + r.n }
        fn ret(r: R) -> R { r }
        fn early(r: R, i: Int) -> Int {
            let keep = ret(r);
            if i == 0 { return keep.n; }
            let mut total = 0;
            for k in 0..3 {
                let copy = R { name: keep.name + k.to_str(), xs: keep.xs, n: k };
                if k == 1 { continue; }
                if k == 2 { break; }
                total += copy.name.len() + lent(copy);
            }
            total + keep.xs.len()
        }
        fn q(r: R) -> Option<Int> { let first: Option<Int> = if r.xs.len() > 0 { Some(r.xs[0]) } else { None }; let v = first?; Some(v + r.n) }
        fn main() -> Str {
            let a = R { name: "a" + "", xs: [1, 2], n: 3 };
            let b = a;
            let c = ret(b);
            let f = |s: Str| c.name + s + c.xs.len().to_str();
            let owned = own(c).len();
            let l = lent(c);
            let e = early(c, 0) + early(c, 1);
            let mut d = c;
            d.name = d.name + "d";
            d.xs.push(9);
            d.n = 0;
            let g = R { name: "g" + "", xs: [], n: 1 };
            f("f") + owned.to_str() + l.to_str() + e.to_str() + d.name + d.xs.len().to_str() + q(c).unwrap_or(-1).to_str() + q(g).unwrap_or(-1).to_str() + q(d).unwrap_or(-1).to_str()
        }"#;
    assert_eq!(value(src), "af21611ad34-11");
}

/// A struct result of `main`, and structs inside values, display as the interpreter does; a
/// `()` field and a boxed enum field included.
#[test]
fn structs_display_boxed() {
    assert_eq!(
        value("struct P { x: Int, y: Int } fn main() -> P { P { x: 1, y: 2 } }"),
        "P { x: 1, y: 2 }"
    );
    assert_eq!(
        value(
            r#"struct U { u: (), s: Str, o: Option<Int>, f: Float } fn main() -> U { U { u: (), s: "s" + "", o: Some(1), f: 0.5 } }"#
        ),
        r#"U { u: (), s: "s", o: Some(1), f: 0.5 }"#
    );
    assert_eq!(
        value(r#"struct P { x: Int, y: Int } fn main() -> List<P> { let p = P { x: 1, y: 2 }; [p, P { x: 3, ..p }] }"#),
        "[P { x: 1, y: 2 }, P { x: 3, y: 2 }]"
    );
    assert_eq!(
        value(r#"struct P { x: Int, y: Int } fn main() -> Option<P> { let p = P { x: 1, y: 2 }; Some(p) }"#),
        "Some(P { x: 1, y: 2 })"
    );
    assert_eq!(
        value(r#"struct E {} fn main() -> List<E> { let e = E {}; [e, E {}] }"#),
        "[E, E]"
    );
}

/// An overflow in `s.f += e` reports the statement, as the interpreter does; an index through a
/// struct's list field reports the index.
#[test]
fn field_write_errors_match() {
    let src = "struct P { x: Int, xs: List<Int> } fn main() -> Int { let mut s = P { x: 9223372036854775807, xs: [1] }; s.x += 1; s.x }";
    let (m, start, end) = error(src);
    assert_eq!(m, "Int overflow");
    assert_eq!(&src[start..end], "s.x += 1;");
    let src =
        "struct P { x: Int, xs: List<Int> } fn main() -> Int { let mut s = P { x: 1, xs: [1] }; s.xs[3] = 1; s.x }";
    let (m, start, end) = error(src);
    assert_eq!(m, "index 3 out of bounds for length 1");
    assert_eq!(&src[start..end], "s.xs[3]");
}

/// `Result<Str, ()>`: a `()` payload is always 0, so a count change on the payload needs no tag
/// test (C8c1 review); the value still boxes, matches, displays and passes `?` as before.
#[test]
fn unit_payloads_are_zero() {
    let src = r#"fn f(i: Int) -> Result<Str, ()> { if i == 0 { Err(()) } else { Ok("s" + i.to_str()) } }
        fn g(i: Int) -> Result<Int, ()> { let s = f(i)?; Ok(s.len()) }
        fn main() -> Str {
            let xs = [f(0), f(1), f(2)];
            let mut out = "";
            for x in xs { out = out + match x { Ok(s) => s, Err(u) => "-" }; }
            let a = f(0);
            let b = a;
            let eq = if b == f(0) && f(1) != f(2) { "eq" } else { "ne" };
            out + eq + g(0).unwrap_or(-1).to_str() + g(5).unwrap_or(-1).to_str() + b.is_err().to_str()
        }"#;
    assert_eq!(value(src), "-s1s2eq-12true");
    assert_eq!(value("fn main() -> Result<Str, ()> { Err(()) }"), "()");
    assert_eq!(
        value("enum E { A(()), B(Str) } fn main() -> List<E> { let a = E::A(()); [a, E::B(\"b\" + \"\"), E::A(())] }"),
        r#"[E::A(()), E::B("b"), E::A(())]"#
    );
}

/// Generic instances, struct update, a recursive fold returning a struct (bench's `tree`), a
/// struct through a `fn` value and `sort_by`, and a 5-field struct beside a small one.
#[test]
fn generics_update_recursion_and_function_values() {
    let src = r#"struct Pair<T> { a: T, b: T }
        struct Stats { sum: Int, leaves: Int, depth: Int }
        enum Tree { Leaf(Int), Node(List<Tree>) }
        fn swap<T>(p: Pair<T>) -> Pair<T> { Pair { a: p.b, b: p.a } }
        fn fold(t: Tree) -> Stats {
            match t {
                Tree::Leaf(v) => Stats { sum: v, leaves: 1, depth: 0 },
                Tree::Node(kids) => {
                    let mut s = Stats { sum: 0, leaves: 0, depth: 0 };
                    for k in kids {
                        let c = fold(k);
                        s.sum += c.sum;
                        s.leaves += c.leaves;
                        if c.depth + 1 > s.depth { s.depth = c.depth + 1; }
                    }
                    s
                }
            }
        }
        fn apply(f: fn(Pair<Int>) -> Pair<Int>, p: Pair<Int>) -> Pair<Int> { f(p) }
        fn main() -> Str {
            let p = swap(Pair { a: "x" + "", b: "y" + "" });
            let q = swap(Pair { a: 1, b: 2 });
            let r = Pair { a: 7, ..q };
            let t = Tree::Node([Tree::Leaf(1), Tree::Node([Tree::Leaf(2), Tree::Leaf(3)]), Tree::Leaf(4)]);
            let s = fold(t);
            let g = apply(swap, r);
            let h = apply(|x: Pair<Int>| Pair { a: x.a * 10, b: x.b }, g);
            let mut ps = [Pair { a: 3, b: 0 }, Pair { a: 1, b: 0 }, Pair { a: 2, b: 0 }];
            ps.sort_by(|x: Pair<Int>, y: Pair<Int>| x.a < y.a);
            let order = ps.map(|x: Pair<Int>| x.a.to_str()).join("");
            p.a + p.b + q.a.to_str() + r.a.to_str() + r.b.to_str() + s.sum.to_str() + s.leaves.to_str() + s.depth.to_str() + g.a.to_str() + h.a.to_str() + order
        }"#;
    assert_eq!(value(src), "yx2711042110123");
}
