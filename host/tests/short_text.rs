//! C20: `s = s + c` with a short `c` appends inline when `s` is unique and has room; `contains`,
//! `find` and `split` with a needle of at most 8 bytes scan for it directly. Every program runs
//! compiled and interpreted and must agree (and leave no object, which the test-heap runtime
//! checks).

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

/// Appends of 0, 1, 2-3, 4-8, 9 and many bytes, ASCII and not, through every append path
/// (`x = x + ..`, `x += ..`, a field, a list element, an unboxed struct's field), growing past
/// the capacity many times; and appends whose left side is shared (another local, a list
/// element, a capture, the appended string itself), where copy on write must leave the other
/// holder unchanged.
#[test]
fn appends_of_every_length_and_sharing() {
    let src = r##"struct P { s: Str, n: Int }
        struct Big { a: Str, b: Str, c: Str, d: Str, e: Str }
        fn main() -> Str {
            let parts = ["", "a", "bc", "def", "ghij", "klmno", "pqrstuvw", "xyz12345!", "é", "日本", "😀", "long part of thirty-two bytes..."];
            let mut s = "";
            let mut t = "";
            let mut p = P { s: "", n: 0 };
            let mut b = Big { a: "", b: "", c: "", d: "", e: "" };
            let mut xs = ["", "x"];
            for i in 0..40 {
                let c = parts[i % 12];
                s = s + c;
                t += c;
                p.s = p.s + c;
                b.e += c;
                xs[1] = xs[1] + c;
            }
            let shared = s;
            s = s + "!";
            let mut u = t;
            u += "?";
            let mut v = xs[1];
            v = v + "#";
            let w = xs.map(|x| x + "|");
            let mut self_ = "ab";
            self_ = self_ + self_;
            self_ += self_;
            let mut chars = "";
            for c in "añ日😀z".chars() { chars = chars + c; }
            s.len().to_str() + "," + shared.len().to_str() + "," + t.len().to_str() + "," + u.len().to_str()
                + "," + p.s.len().to_str() + "," + b.e.len().to_str() + "," + xs[1].len().to_str() + ","
                + v.len().to_str() + "," + w[1].len().to_str() + "," + self_ + "," + chars + ","
                + (s == shared + "!").to_str() + (t == p.s).to_str() + (b.e == xs[1].slice(1, xs[1].len())).to_str()
        }"##;
    assert_eq!(
        value(src),
        "235,234,234,235,234,234,235,236,236,abababab,añ日😀z,truetruetrue"
    );
}

/// `contains`, `find` and `split` with needles of 0 (`split`: an error), 1, 2, 7, 8 and 9 bytes:
/// at the start, the end, overlapping their own prefix, absent, longer than the haystack,
/// matching only past a false start, and non-ASCII.
#[test]
fn short_needles() {
    let src = r##"fn f(o: Option<Int>) -> Str { match o { Some(i) => i.to_str(), None => "-" } }
        fn main() -> Str {
            let hs = ["", "a", "ab", "aab", "abcdefgh", "xabcdefgh", "abcdefghi", "aaaaaaaab", "a.b.c", "日本語の日本", "abababababx"];
            let ns = ["", "a", "b", ".", "ab", "aab", "abcdefg", "abcdefgh", "abcdefghi", "aaaab", "日本", "本", "ababx", "abababababx", "zz"];
            let mut out = "";
            for h in hs {
                for n in ns {
                    out = out + h.contains(n).to_str().slice(0, 1) + f(h.find(n));
                    if n != "" { out = out + h.split(n).len().to_str() + h.split(n).join("/"); }
                    out = out + ";";
                }
            }
            out
        }"##;
    let v = value(src);
    assert!(v.contains("t0;"), "{v}");
}

#[test]
fn split_with_an_empty_separator_is_an_error() {
    let src = r##"fn main() -> Int { let s = "abc"; let e = ""; s.split(e).len() }"##;
    let (msg, start, end) = both(src).expect_err("an error");
    assert_eq!(&src[start..end], "s.split(e)", "{msg}");
}
