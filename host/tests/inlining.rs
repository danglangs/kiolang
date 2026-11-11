//! C8c3: a small function is inlined at its direct calls, its arguments bound to fresh locals
//! (a borrowed parameter reads the argument in place), `return` and `?` leaving the inlined
//! block. Every program runs compiled and interpreted and must agree, errors at the callee's
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

/// Inlined helpers in every position: owned parameters moved, returned and dropped; borrowed
/// ones read in place from a local, a field and a temporary; the last use of an argument;
/// `return` from inside nested blocks with lets live; `?` on `Option` and `Result`, through
/// two levels of inlining; unboxed enum and struct parameters and results; a `match` binding
/// of a borrowed parameter; a callee with `let`s inlined in a loop of the caller; a `()`
/// result with an early `return`; the same function also called as a value; recursive,
/// looping and closure-making functions beside them, called.
#[test]
fn inlined_calls_in_every_position() {
    let src = r#"enum J { Null, N(Int), S(Str) }
        struct P { name: Str, n: Int }
        fn id(s: Str) -> Str { s }
        fn len(s: Str) -> Int { s.len() }
        fn drop_it(s: Str) -> Int { 1 }
        fn pick(a: Str, b: Str, c: Bool) -> Str {
            let t = a + "!";
            if c { let u = t + b; return u; }
            if b == "x" { return a; }
            b + t
        }
        fn half(n: Int) -> Result<Int, Str> {
            if n % 2 == 1 { return Err("odd " + n.to_str()); }
            Ok(n / 2)
        }
        fn quarter(n: Int) -> Result<Int, Str> { let h = half(n)?; half(h) }
        fn head(xs: List<Str>) -> Option<Str> { if xs.len() == 0 { None } else { Some(xs[0]) } }
        fn two(xs: List<Str>) -> Option<Str> { let a = head(xs)?; Some(a + a) }
        fn name_of(j: J) -> Str { match j { J::S(s) => s, J::N(n) => n.to_str(), J::Null => "null" } }
        fn mk(s: Str, n: Int) -> P { P { name: s, n: n } }
        fn bump(p: P) -> P { P { n: p.n + 1, ..p } }
        fn tag(s: Str, i: Int) -> Str { let t = s + ":"; t + i.to_str() }
        fn noop(n: Int) { if n > 0 { return; } }
        fn early(x: Int) -> Int { return x * 2; }
        fn even(n: Int) -> Bool { if n == 0 { true } else { odd(n - 1) } }
        fn odd(n: Int) -> Bool { if n == 0 { false } else { even(n - 1) } }
        fn looped(n: Int) -> Int { let mut s = 0; while s < n { s += 1; } s }
        fn adder(n: Int) -> fn(Int) -> Int { |x: Int| x + n }
        fn main() -> Str {
            let s = "abc" + "";
            let p = mk(s + "p", 4);
            let q = bump(p);
            let mut out = [id(s + "1"), id(s), pick("a" + "", "b" + "", true), pick("a", "x", false), pick("a", "y", false)];
            out.push(len(q.name).to_str() + len(out[0]).to_str() + len("lit").to_str() + drop_it(q.name + "").to_str());
            out.push(match quarter(8) { Ok(n) => n.to_str(), Err(e) => e });
            out.push(match quarter(6) { Ok(n) => n.to_str(), Err(e) => e });
            out.push(two(out).unwrap_or("none"));
            out.push(two([]).unwrap_or("none"));
            for j in [J::Null, J::N(3), J::S("s" + "")] { out.push(name_of(j)); }
            let mut i = 0;
            while i < 3 { out.push(tag(q.name, i)); noop(i); i += 1; }
            let f = len;
            out.push((f("four") + early(3) + q.n + looped(2) + adder(1)(2)).to_str());
            out.push(even(10).to_str() + odd(7).to_str());
            out.push(q.name);
            out.join(",")
        }"#;
    assert_eq!(
        value(src),
        "abc1,abc,a!b,a,ya!,4431,2,odd 3,abc1abc1,none,null,3,s,abcp:0,abcp:1,abcp:2,20,truetrue,abcp"
    );
}

/// An error in an inlined callee reports the callee's span and message, as the call would:
/// overflow, an index, division by zero, a slice, and an `Err` from `?` returned by `main`.
#[test]
fn inlined_errors_keep_the_callee_span() {
    let cases = [
        ("fn add(a: Int, b: Int) -> Int { a + b }\nfn main() -> Int { add(1, 2) + add(9223372036854775807, 1) }", "a + b", "Int overflow"),
        ("fn byte(b: Bytes, i: Int) -> Int { if i < 0 { -1 } else { b[i] } }\nfn main() -> Int { let b = \"ab\".to_bytes(); byte(b, 1) + byte(b, 2) }", "b[i]", "index 2 out of bounds for length 2"),
        ("fn d(a: Int, b: Int) -> Int { a / b }\nfn main() -> Int { let z = 0; d(4, 2) + d(1, z) }", "a / b", "division by zero"),
        ("fn cut(s: Str) -> Str { s.slice(1, 9) }\nfn main() -> Str { cut(\"abc\" + \"\") }", "s.slice(1, 9)", "slice 1..9 out of bounds for length 3"),
    ];
    for (src, at, msg) in cases {
        let start = src.find(at).unwrap();
        assert_eq!(both(src), Err((msg.into(), start, start + at.len())), "{src}");
    }
    let src = "fn half(n: Int) -> Result<Int, Str> { if n % 2 == 1 { return Err(\"odd\"); } Ok(n / 2) }\nfn main() -> Result<Int, Str> { let h = half(6)?; half(h) }";
    assert_eq!(both(src), Ok("odd".into()));
}
