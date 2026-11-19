//! C19: a closure literal or named function passed to `map`, `filter`, `any`, `all`,
//! `sort_by_key`, `unwrap_or_else`, or `map`/`and_then`/`map_err` on `Option` and `Result` is
//! called directly (or inlined), its captures read from the caller's locals, with no function
//! object. Every program runs compiled and interpreted and must agree, errors at the closure's
//! own spans (and leave no object, which the test-heap runtime checks).

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

/// Captures of every representation (an `Int`, a `Str`, a list, an unboxed enum and struct, a
/// boxed struct), used and returned by the body; elements of the same kinds; a named function
/// beside the closures; each built-in.
#[test]
fn captures_and_elements_of_every_kind() {
    let src = r#"enum J { Null, N(Int), S(Str) }
        struct P { name: Str, n: Int }
        struct Big { a: Str, b: Str, c: Int, d: Int, e: Int }
        fn twice(x: Int) -> Int { x * 2 }
        fn long(s: Str) -> Bool { s.len() > 2 }
        fn main() -> Str {
            let k = 3;
            let sep = "-";
            let ws = ["ab", "cde", "f", "ghij"];
            let j = J::S("j");
            let p = P { name: "p", n: 4 };
            let big = Big { a: "a", b: "b", c: 1, d: 2, e: 3 };
            let xs = [1, 2, 3, 4, 5, 6];
            let a = xs.map(|x| x * k).filter(|x| x % 2 == 0);
            let b = ws.map(|w| w + sep + p.name);
            let c = ws.filter(|w| w.len() >= p.n - 1);
            let d = ws.map(|w| match j { J::S(s) => s + w, _ => w });
            let e = xs.map(|x| big.a + x.to_str() + big.b + (big.c + big.d + big.e).to_str());
            let f = xs.map(twice).filter(|x| x > k);
            let g = ws.filter(long);
            let h = ws.map(|w| ws.len() + w.len());
            let js = [J::N(1), J::Null, J::S("s")];
            let i = js.map(|v| match v { J::N(n) => n + k, J::S(s) => s.len(), J::Null => 0 });
            let ps = [P { name: "x", n: 1 }, P { name: "yy", n: 2 }];
            let l = ps.map(|q| q.name + sep).filter(|s| s != "x-");
            let m = ps.any(|q| q.n == k - 1);
            let n = ps.all(|q| q.name.len() < k);
            let o = ws.any(|w| w == sep);
            let q = xs.all(|x| x > 0);
            a.len().to_str() + b[2] + c.len().to_str() + d[0] + e[5] + f.len().to_str() + g[1]
                + h[3].to_str() + i[2].to_str() + l[0] + m.to_str() + n.to_str() + o.to_str() + q.to_str()
        }"#;
    assert_eq!(value(src), "3f-p2jaba6b65ghij81yy-truetruefalsetrue");
}

/// The `Option` and `Result` built-ins with closure literals, including `?` and `return` in the
/// body (leaving the closure, not the caller), a capture of every kind, a named function, and
/// a closure nested in a closure's body.
#[test]
fn option_and_result_bodies_with_early_exits() {
    let src = r#"fn half(x: Int) -> Option<Int> { if x % 2 == 0 { Some(x / 2) } else { None } }
        fn os(o: Option<Int>) -> Str { match o { Some(x) => "S" + x.to_str(), None => "N" } }
        fn rs(r: Result<Int, Str>) -> Str { match r { Ok(x) => "O" + x.to_str(), Err(e) => "E" + e } }
        fn main() -> Str {
            let k = 10;
            let s = "s";
            let o: Option<Int> = Some(4);
            let n: Option<Int> = None;
            let r: Result<Int, Str> = Ok(3);
            let e: Result<Int, Str> = Err("bad");
            let a = o.map(|x| x + k);
            let b = o.and_then(|x| { let h = half(x)?; Some(h + k) });
            let c = Some(3).and_then(|x| { let h = half(x)?; Some(h + k) });
            let d = o.and_then(half).and_then(half).and_then(half);
            let f = r.map(|x| { if x > 2 { return x * 100; } x });
            let g = e.map_err(|m| m + s);
            let h = e.and_then(|x| Ok(x + 1));
            let i = r.and_then(|x| if x > k { Ok(x) } else { Err(s + "small") });
            let j = n.unwrap_or_else(|| k * 7);
            let l = e.unwrap_or_else(|m| m.len() + k);
            let m = o.map(|x| [1, 2, 3].map(|y| y * x + k).filter(|y| y > k + 4).len());
            let q = r.map(|x| { let t = [x, x + 1].filter(|y| { if y == 3 { return false; } true }); t.len() });
            os(a) + os(b) + os(c) + os(d) + rs(f) + rs(g) + rs(h) + rs(i) + j.to_str() + l.to_str()
                + os(m) + rs(q)
        }"#;
    assert_eq!(value(src), "S14S12NNO300EbadsEbadEssmall7013S2O1");
}

/// Captures shared with the receiver and the result; a capture moved after the call; a
/// closure returning its captured `Str` (+1 per call); a list of strings filtered by a
/// closure that keeps a borrowed element; `sort_by_key` capturing a local, a named function,
/// and its own list (which stays a call through the object).
#[test]
fn sharing_and_sort_by_key() {
    let src = r#"fn neg(x: Int) -> Int { 0 - x }
        fn main() -> Str {
            let mut ws = ["pear", "fig", "apple", "kiwi"];
            let tag = "t";
            let tagged = ws.map(|w| tag);
            let keep = ws.filter(|w| w.len() > tag.len() + 2);
            let mine = ws.map(|w| ws.len());
            let moved = tag;
            ws.sort_by_key(|w| w.len());
            let mut xs = [3, 1, 2];
            let off = 10;
            xs.sort_by_key(|x| off - x);
            let mut ys = [1, 3, 2];
            ys.sort_by_key(neg);
            let mut zs = [2, 1, 3];
            zs.sort_by_key(|z| zs.len() - z);
            tagged[3] + keep[1] + mine[0].to_str() + moved + ws[0] + ws[3] + xs[0].to_str()
                + ys[0].to_str() + zs[0].to_str()
        }"#;
    assert_eq!(value(src), "tapple4tfigapple333");
}

/// Errors inside a closure body report the closure's own span, mid-loop, with the elements
/// and captures held so far released (the test-heap runtime checks).
#[test]
fn errors_at_the_closures_span() {
    for (src, needle) in [
        (
            r#"fn main() -> Int { let k = "k"; let xs = [1, 2, 0]; xs.map(|x| 10 / x + k.len()).len() }"#,
            "10 / x",
        ),
        (
            r#"fn main() -> Int { let ws = ["a", "bb"]; let i = 5; ws.filter(|w| w.slice(0, i) == "a").len() }"#,
            "w.slice(0, i)",
        ),
        (
            r#"fn main() -> Bool { let xs = [[1], []]; xs.any(|x| x[0] == 2) }"#,
            "x[0]",
        ),
        (
            r#"fn main() -> Int { let o = Some(9223372036854775807); o.map(|x| x + 1).unwrap_or(0) }"#,
            "x + 1",
        ),
    ] {
        let (msg, start, end) = both(src).expect_err(src);
        assert_eq!(&src[start..end], needle, "{msg}");
    }
}

/// Recursion through a built-in: a function passing itself (inlined into itself once), two
/// passing each other, and a closure calling the function that made it.
#[test]
fn recursion_through_built_ins() {
    let src = r#"enum J { N(Int), L(List<J>) }
        fn show(j: J) -> Str { match j { J::N(n) => n.to_str(), J::L(xs) => "[" + xs.map(show).join(",") + "]" } }
        fn even(n: Int) -> Bool { if n == 0 { true } else { [n - 1].all(odd) } }
        fn odd(n: Int) -> Bool { if n == 0 { false } else { [n - 1].any(even) } }
        fn depth(n: Int) -> Int { if n == 0 { 0 } else { [n - 1].map(|m| depth(m) + 1)[0] } }
        fn main() -> Str {
            let j = J::L([J::N(1), J::L([J::N(2), J::L([])]), J::N(3)]);
            show(j) + even(7).to_str() + odd(7).to_str() + depth(30).to_str()
        }"#;
    assert_eq!(value(src), "[1,[2,[]],3]falsetrue30");
}
