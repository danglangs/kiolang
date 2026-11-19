//! P9: compiled `for c in s.chars()` walks the string's UTF-8 without building the list. Every
//! program runs compiled and interpreted and must agree (and leave no object, which the
//! test-heap runtime checks: `KIO_TEST_HEAP=1`).

use kio_host::{Interp, Kio, Limits};

type Run = Result<String, (String, usize, usize)>;

fn limits(fuel: u64) -> Limits {
    Limits {
        fuel,
        ..Limits::default()
    }
}

fn compiled(src: &str, fuel: u64) -> Run {
    match Kio::new(limits(fuel)).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(limits(20_000_000_000)).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn value(src: &str) -> String {
    let c = compiled(src, 20_000_000_000);
    assert_eq!(c, interpreted(src), "compiled vs interpreted: {src}");
    c.unwrap_or_else(|e| panic!("{src}: {e:?}"))
}

/// Order and one-character `Str`s: empty, ASCII, two-, three- and four-byte characters, each
/// yielded whole, kept (pushed, returned, captured) and only borrowed.
#[test]
fn characters_in_order() {
    let src = r#"fn all(s: Str) -> Str {
            let mut out = "";
            let mut n = 0;
            for c in s.chars() {
                out += "[" + c + "]";
                n += c.len();
            }
            out + n.to_str()
        }
        fn main() -> Str {
            let mut kept: List<Str> = [];
            for c in "aé中😀".chars() { kept.push(c); }
            let mut fs: List<fn() -> Str> = [];
            for c in "x中".chars() { fs.push(|| c + c); }
            let mut m: Map<Str, Int> = Map::new();
            for c in "abcaé😀é".chars() { m.insert(c, m.get(c).unwrap_or(0) + 1); }
            all("") + "|" + all("abc") + "|" + all("aé中😀z") + "|" + kept.join(",") + "|"
                + fs[0]() + fs[1]() + "|" + m.keys().join("") + m.get("é").unwrap_or(0).to_str()
        }"#;
    assert_eq!(value(src), "0|[a][b][c]3|[a][é][中][😀][z]11|a,é,中,😀|xx中中|abcé😀2");
}

/// `break`, `continue`, `return`, `?` and an error inside the loop; a character from the
/// pass `break` left.
#[test]
fn exits_inside_the_loop() {
    let src = r#"fn first_wide(s: Str) -> Str {
            for c in s.chars() {
                if c.len() > 1 { return c; }
            }
            "none"
        }
        fn upto(s: Str, stop: Str) -> Str {
            let mut out = "";
            for c in s.chars() {
                if c == stop { break; }
                if c == "é" || c == "b" { continue; }
                out += c;
            }
            out
        }
        fn digits(s: Str) -> Option<Int> {
            let mut n = 0;
            for c in s.chars() { n = n * 10 + c.parse_int()?; }
            Some(n)
        }
        fn main() -> Str {
            let mut last = "";
            for c in "ab中cd".chars() {
                last = c;
                if c == "中" { break; }
            }
            first_wide("ab😀c") + first_wide("abc") + "|" + upto("abéc中d", "中") + upto("é😀", "x")
                + "|" + last + "|" + digits("123").unwrap_or(-1).to_str() + digits("1é3").unwrap_or(-1).to_str()
        }"#;
    assert_eq!(value(src), "😀none|ac😀|中|123-1");
    let src = r#"fn main() -> Int {
            let xs = [1, 2];
            let mut i = 0;
            for c in "aé中".chars() { i += xs[i]; }
            i
        }"#;
    let c = compiled(src, 20_000_000_000);
    assert_eq!(c, interpreted(src));
    assert!(c.unwrap_err().0.contains("out of bounds"));
}

/// The loop sees the string as it was when it started: reassigning, appending to and moving
/// the variable it came from inside the body changes nothing, and the old string stays alive.
#[test]
fn the_source_reassigned_inside() {
    let src = r#"fn main() -> Str {
            let mut s = "aé" + "中x";
            let mut out = "";
            for c in s.chars() {
                s = "zz" + c;
                out += c;
            }
            let mut t = "😀" + "b";
            for c in t.chars() {
                t += c + c;
                out += t;
            }
            let mut u = "ké" + "";
            let mut kept: List<Str> = [];
            for c in u.chars() {
                kept.push(u);
                u = c;
            }
            out + "|" + s + "|" + kept.join(",") + u
        }"#;
    assert_eq!(value(src), "aé中x😀b😀😀😀b😀😀bb|zzx|ké,ké");
}

/// Nested loops over the same string and over each character; `chars()` of a temporary (a
/// concatenation, a call, a literal, a field, a list element, an `Option` payload) and of a
/// borrowed parameter.
#[test]
fn nested_and_temporaries() {
    let src = r#"struct W { text: Str }
        fn word() -> Str { "é" + "a" }
        fn count(s: Str) -> Int { let mut n = 0; for c in s.chars() { n += 1; } n }
        fn main() -> Str {
            let s = "a中😀";
            let mut out = "";
            for a in s.chars() {
                for b in s.chars() {
                    if a != b { out += a + b + " "; }
                }
                for d in (a + "é").chars() { out += d; }
            }
            out += "|";
            for c in (s + "z").chars() { out += c; }
            for c in word().chars() { out += c; }
            for c in "lit中".chars() { out += c; }
            let w = W { text: "wé" + "" };
            for c in w.text.chars() { out += c; }
            let ws = ["p中", "q"];
            for x in ws { for c in x.chars() { out += c + "."; } }
            for c in ws[0].chars() { out += c; }
            match Some("ö" + "") {
                Some(t) => { for c in t.chars() { out += c; } }
                None => {}
            }
            out + count(s).to_str() + count(w.text).to_str()
        }"#;
    assert_eq!(
        value(src),
        "a中 a😀 aé中a 中😀 中é😀a 😀中 😀é|a中😀zéalit中wép.中.q.p中ö32"
    );
}

/// A long string's loop is charged per character: it runs out of fuel rather than free.
#[test]
fn charged_per_character() {
    let src = r#"fn main() -> Int {
            let mut s = "aé";
            let mut i = 0;
            while i < 16 { s = s + s; i += 1; }
            let mut n = 0;
            for c in s.chars() { if c == "a" { n += 1; } }
            n
        }"#;
    assert_eq!(value(src), "65536");
    let e = Kio::new(limits(3_000_000)).unwrap().run(src, vec![]).unwrap_err();
    assert!(e.to_string().starts_with("out of fuel"), "{e}");
}
