//! C8c4: `to_bytes` and `to_str` take an operand that holds its own reference (a last use, a
//! temporary) and reuse the object when that reference is its only one; a shared, pinned or
//! borrowed operand is copied. Every program runs compiled and interpreted and must agree (and
//! leave no object, which the test-heap runtime checks).

use kio_host::{Interp, Kio, Limits};

type Run = Result<String, (String, usize, usize)>;

fn compiled(src: &str) -> Run {
    match Kio::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn both(src: &str) -> Run {
    let c = compiled(src);
    assert_eq!(c, interpreted(src), "compiled vs interpreted: {src}");
    c
}

/// Each operand shape: a moved local (adopted), a local read again later, an element, a field,
/// a captured value and a map value (shared: copied, the original unchanged after the copy is
/// pushed to), a literal (pinned), a temporary, a borrowed and an owned parameter, a
/// reassignment through both conversions, conversions in a loop, and a `Bytes` built from
/// `"".to_bytes()` by `push`.
#[test]
fn conversions_adopt_or_copy_in_every_position() {
    let src = r#"struct W { s: Str, n: Int }
        fn lent(s: Str) -> Bytes { s.to_bytes() }
        fn owned(s: Str) -> Bytes { let t = s; t.to_bytes() }
        fn build(n: Int) -> Str {
            let mut out = "".to_bytes();
            for i in 0..n { out.push(97 + i % 26); }
            out.to_str()
        }
        fn bang(b: Bytes) -> Str { let mut c = b; c.push(33); c.to_str() }
        fn main() -> Str {
            let mut r = "";
            let a = "x" + "y";
            let mut b = a.to_bytes();
            b.push(65);
            r += b.to_str() + ",";
            let s = "p" + "q";
            let mut c = s.to_bytes();
            c.push(66);
            r += s + c.to_str() + ",";
            let xs = ["e" + "l"];
            let e = xs[0];
            r += bang(e.to_bytes()) + xs[0] + ",";
            let w = W { s: "f" + "d", n: 1 };
            r += bang(w.s.to_bytes()) + w.s + w.n.to_str() + ",";
            let k = "c" + "p";
            let f = || k.len();
            r += bang(k.to_bytes()) + f().to_str() + ",";
            let mut m: Map<Str, Str> = Map::new();
            let v = "m" + "v";
            m.insert("k", v);
            r += bang(v.to_bytes()) + m.get("k").unwrap_or("") + ",";
            for i in 0..2 {
                r += bang("lit".to_bytes()) + bang(("t" + i.to_str()).to_bytes());
            }
            let p = "par" + "am";
            r += bang(lent(p)) + p + bang(owned(p)) + p + ",";
            let mut z = "z" + "z";
            z = z.to_bytes().to_str();
            z += "!";
            r += z + ",";
            let mut y = "q".to_bytes();
            let y2 = y;
            y.push(67);
            r += y.to_str() + y2.to_str() + ",";
            for i in 0..3 {
                let t = "n" + i.to_str();
                let mut u = t.to_bytes();
                u.push(48 + i);
                r += u.to_str();
            }
            r + "," + build(3) + build(20).len().to_str() + "".to_bytes().to_str() + build(0)
        }"#;
    assert_eq!(
        both(src).unwrap(),
        "xyA,pqpqB,el!el,fd!fd1,cp!2,mv!mv,lit!t0!lit!t1!param!paramparam!param,zz!,qCq,n00n11n22,abc20"
    );
}

/// Invalid UTF-8 still errors at the conversion's span, whether the operand is a moved local,
/// a temporary or a shared local.
#[test]
fn invalid_utf8_errors_at_the_site() {
    for (body, at) in [
        ("let mut b = \"\".to_bytes(); b.push(255); b.to_str()", "b.to_str()"),
        (
            "\"\u{e9}\".to_bytes().slice(0, 1).to_str()",
            "\"\u{e9}\".to_bytes().slice(0, 1).to_str()",
        ),
        (
            "let mut b = \"\".to_bytes(); b.push(200); let s = b.to_str(); b.len().to_str() + s",
            "b.to_str()",
        ),
    ] {
        let src = format!("fn main() -> Str {{ {body} }}");
        let (msg, start, end) = both(&src).unwrap_err();
        assert!(msg.contains("invalid UTF-8"), "{msg}");
        assert_eq!(&src[start..end], at, "{src}");
    }
}
