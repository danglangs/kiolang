//! L11's built-ins: Rust's edge cases for each, and the checker's rules for them. The
//! differential test runs these programs compiled too.

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

/// `main`'s value as `kio run` shows it, or the error's message and the text at its span.
fn exec(src: &'static str) -> Result<String, (String, String)> {
    let p = check(src, &CapTable::v0()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || match run(&p, &mut NoCaps) {
        Ok(v) => Ok(p.display(&v)),
        Err(RunErr { span, msg }) => Err((msg, src[span.start..span.end].to_string())),
    })
}

fn shows(src: &'static str) -> String {
    exec(src).unwrap_or_else(|e| panic!("{e:?}"))
}

fn fails(src: &'static str) -> (String, String) {
    exec(src).unwrap_err()
}

/// The checker's message and hint for `src`.
fn rejects(src: &str) -> (String, Option<String>) {
    let d = check(src, &CapTable::v0()).unwrap_err();
    (d[0].msg.clone(), d[0].hint.clone())
}

#[test]
fn sort_by_is_stable_and_takes_less() {
    let src = r#"
struct P { name: Str, age: Int }
fn main() -> List<Str> {
    let mut ps = [P { name: "a", age: 3 }, P { name: "b", age: 1 }, P { name: "c", age: 3 },
                  P { name: "d", age: 2 }, P { name: "e", age: 1 }];
    ps.sort_by(|x, y| x.age < y.age);
    ps.map(|p| p.name)
}"#;
    assert_eq!(shows(src), r#"["b", "e", "d", "a", "c"]"#);
    let src = r#"
fn desc(a: Int, b: Int) -> Bool { a > b }
fn main() -> List<Int> {
    let mut xs = [5, -1, 3, 3, 9, 0, 2, 8, 7, 1, 4];
    xs.sort_by(desc);
    xs
}"#;
    assert_eq!(shows(src), "[9, 8, 7, 5, 4, 3, 3, 2, 1, 0, -1]");
}

#[test]
fn sort_by_empty_one_and_places() {
    let src = r#"
struct S { xs: List<List<Int>> }
fn main() -> Str {
    let mut e: List<Int> = [];
    e.sort_by(|a, b| a < b);
    let mut one = [7];
    one.sort_by(|a, b| a < b);
    let mut s = S { xs: [[3, 1, 2], [9, 8]] };
    let keep = s;
    s.xs[0].sort_by(|a, b| a < b);
    s.xs[1].sort_by_key(|x| x);
    e.len().to_str() + " " + one[0].to_str() + " " + s.xs[0][0].to_str() + s.xs[0][2].to_str()
        + s.xs[1][0].to_str() + " " + keep.xs[0][0].to_str()
}"#;
    assert_eq!(shows(src), "0 7 138 3");
}

/// A `less` that is no order still gives a permutation, the same compiled (the differential
/// test runs this program both ways) and pinned here.
#[test]
fn sort_by_inconsistent_comparator() {
    let src = r#"
fn main() -> List<List<Int>> {
    let mut a = [5, 1, 4, 2, 8, 0, 3, 9, 7, 6, 11, 10];
    a.sort_by(|x, y| (x + y) % 3 == 0);
    let mut b = [1, 2, 3, 4, 5, 6, 7];
    b.sort_by(|x, y| true);
    let mut c = [3, 1, 2, 5, 4];
    c.sort_by(|x, y| x % 2 == 1);
    [a, b, c]
}"#;
    assert_eq!(
        shows(src),
        "[[8, 1, 5, 2, 4, 9, 3, 0, 7, 6, 10, 11], [7, 6, 5, 4, 3, 2, 1], [5, 1, 3, 2, 4]]"
    );
}

#[test]
fn sort_by_comparator_sees_its_own_copy() {
    let src = r#"
fn main() -> List<Int> {
    let mut xs = [3, 1, 2];
    let n = |a: Int, b: Int| xs.len() > 0 && a < b;
    xs.sort_by(n);
    xs.push(xs.len());
    xs
}"#;
    assert_eq!(shows(src), "[1, 2, 3, 3]");
}

#[test]
fn sort_by_error_in_comparator() {
    let src = r#"
fn main() -> List<Int> {
    let mut xs = [3, 0, 2];
    xs.sort_by(|a, b| 6 / a < 6 / b);
    xs
}"#;
    assert_eq!(fails(src), ("division by zero".to_string(), "6 / a".to_string()));
}

#[test]
fn sort_by_key_int_and_str() {
    let src = r#"
fn main() -> List<List<Str>> {
    let mut a = ["pear", "fig", "apple", "kiwi", "plum", "b"];
    a.sort_by_key(|s| s.len());
    let mut b = ["b", "a", "C", "ä", "", "aa"];
    b.sort_by_key(|s| s);
    let mut c: List<Str> = [];
    c.sort_by_key(|s| s.len());
    [a, b, c]
}"#;
    assert_eq!(
        shows(src),
        r#"[["b", "fig", "pear", "kiwi", "plum", "apple"], ["", "C", "a", "aa", "b", "ä"], []]"#
    );
    let src = r#"
fn main() -> List<Int> {
    let mut xs = [10, -3, 7, -10, 3];
    xs.sort_by_key(|x| if x < 0 { 0 - x } else { x });
    xs
}"#;
    assert_eq!(shows(src), "[-3, 3, 7, 10, -10]");
}

#[test]
fn reverse() {
    let src = r#"
fn main() -> List<List<Int>> {
    let mut a = [1, 2, 3];
    let keep = a;
    a.reverse();
    let mut b = [1, 2, 3, 4];
    b.reverse();
    let mut e: List<Int> = [];
    e.reverse();
    let mut n = [[1, 2], [3]];
    n[0].reverse();
    [a, keep, b, e, n[0]]
}"#;
    assert_eq!(shows(src), "[[3, 2, 1], [1, 2, 3], [4, 3, 2, 1], [], [2, 1]]");
}

#[test]
fn list_slice() {
    let src = r#"
fn main() -> List<List<Str>> {
    let xs = ["a", "b", "c"];
    [xs.slice(0, 3), xs.slice(1, 2), xs.slice(3, 3), xs.slice(0, 0), xs]
}"#;
    assert_eq!(shows(src), r#"[["a", "b", "c"], ["b"], [], [], ["a", "b", "c"]]"#);
    let src = "fn main() -> List<Int> { [1, 2, 3].slice(2, 1) }";
    let msg = "slice 2..1 out of bounds for length 3";
    assert_eq!(fails(src), (msg.to_string(), "[1, 2, 3].slice(2, 1)".to_string()));
    let src = "fn main() -> List<Int> { [1, 2, 3].slice(0, 4) }";
    assert_eq!(fails(src).0, "slice 0..4 out of bounds for length 3");
    let src = "fn main() -> List<Int> { [1, 2, 3].slice(-1, 2) }";
    assert_eq!(fails(src).0, "slice -1..2 out of bounds for length 3");
}

#[test]
fn join() {
    let src = r#"
fn main() -> List<Str> {
    let e: List<Str> = [];
    [["a", "b", "c"].join(", "), ["x"].join("-"), e.join("-"), ["a", "b"].join(""), ["", ""].join("é")]
}"#;
    assert_eq!(shows(src), r#"["a, b, c", "x", "", "ab", "é"]"#);
}

#[test]
fn int_and_float_min_max() {
    let src = r#"
fn main() -> List<Int> {
    let m = 0 - 9223372036854775807 - 1;
    [3.min(5), 3.max(5), (0 - 2).min(2), m.min(0), m.max(9223372036854775807), 4.min(4)]
}"#;
    assert_eq!(shows(src), "[3, 5, -2, -9223372036854775808, 9223372036854775807, 4]");
    let src = r#"
fn main() -> List<Float> {
    let nan = 0.0 / 0.0;
    let inf = 1.0 / 0.0;
    let z = 0.0;
    let nz = -0.0;
    [1.5.min(2.0), 1.5.max(2.0), nan.min(1.0), 1.0.min(nan), nan.max(2.0), 2.0.max(nan),
     nan.min(nan), z.min(nz), nz.min(z), z.max(nz), nz.max(z), inf.min(-inf), inf.max(3.0)]
}"#;
    assert_eq!(shows(src), "[1.5, 2, 1, 1, 2, 2, NaN, -0, -0, 0, 0, -inf, inf]");
}

#[test]
fn case_mapping() {
    let src = r#"
fn main() -> List<Str> {
    ["Hello, World!".to_lowercase(), "Hello, World!".to_uppercase(), "".to_lowercase(),
     "straße".to_uppercase(), "ΟΔΟΣ".to_lowercase(), "İ".to_lowercase(), "ǅ".to_uppercase(),
     "ﬁ".to_uppercase(), "123 ✓".to_uppercase()]
}"#;
    let want: Vec<String> = [
        "Hello, World!".to_lowercase(),
        "Hello, World!".to_uppercase(),
        String::new(),
        "straße".to_uppercase(),
        "ΟΔΟΣ".to_lowercase(),
        "İ".to_lowercase(),
        "ǅ".to_uppercase(),
        "ﬁ".to_uppercase(),
        "123 ✓".to_string(),
    ]
    .iter()
    .map(|s| format!("{s:?}"))
    .collect();
    assert_eq!(shows(src), format!("[{}]", want.join(", ")));
    assert_eq!(want[3], "\"STRASSE\"");
    assert_eq!(want[4], "\"οδος\"");
}

#[test]
fn unwrap_or_else() {
    let src = r#"
fn len_or(e: Str) -> Int { e.len() }
fn main() -> List<Int> {
    let some: Option<Int> = Some(1);
    let none: Option<Int> = None;
    let ok: Result<Int, Str> = Ok(2);
    let err: Result<Int, Str> = Err("four");
    let k = 10;
    [some.unwrap_or_else(|| 1 / 0), none.unwrap_or_else(|| k + 1), ok.unwrap_or_else(|e| 1 / 0),
     err.unwrap_or_else(len_or), err.unwrap_or_else(|e| e.len() * k)]
}"#;
    assert_eq!(shows(src), "[1, 11, 2, 4, 40]");
    let src = r#"
fn main() -> List<Str> {
    let a: Option<List<Str>> = None;
    let b: Result<Str, Int> = Err(3);
    let c: Option<Str> = Some("s");
    let mut out = a.unwrap_or_else(|| []);
    out.push(b.unwrap_or_else(|n| n.to_str()));
    out.push(c.unwrap_or_else(|| "none"));
    out
}"#;
    assert_eq!(shows(src), r#"["3", "s"]"#);
    let src = "fn main() -> Int { let o: Option<Int> = None; o.unwrap_or_else(|| 1 / 0) }";
    assert_eq!(fails(src), ("division by zero".to_string(), "1 / 0".to_string()));
}

#[test]
fn bytes_extend() {
    let src = r#"
struct B { b: List<Bytes> }
fn main() -> List<Str> {
    let mut a = "ab".to_bytes();
    let keep = a;
    a.extend("cd".to_bytes());
    a.extend("".to_bytes());
    let mut e = "".to_bytes();
    e.extend(e);
    let mut s = "xy".to_bytes();
    s.extend(s);
    let mut n = B { b: ["1".to_bytes()] };
    n.b[0].extend("23".to_bytes());
    let mut big = "".to_bytes();
    let mut i = 0;
    while i < 100 { big.extend("0123456789".to_bytes()); i += 1; }
    [a.to_str(), keep.to_str(), e.to_str(), s.to_str(), n.b[0].to_str(), big.len().to_str()]
}"#;
    assert_eq!(shows(src), r#"["abcd", "ab", "", "xyxy", "123", "1000"]"#);
}

/// FIPS 180 test vectors (NIST's examples), and lengths around the padding's block edge.
#[test]
fn sha256_vectors() {
    let hex = |m: &[u8]| lang::digest::to_hex(&lang::digest::sha256(m));
    let vectors: [(&[u8], &str); 4] = [
        (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
    ];
    for (m, want) in vectors {
        assert_eq!(hex(m), want);
    }
    assert_eq!(
        hex(&vec![b'a'; 1_000_000]),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
    let edges = [
        (55, "d5e285683cd4efc02d021a5c62014694958901005d6f71e89e0989fac77e4072"),
        (56, "04c26261370ee7541549d16dee320c723e3fd14671e66a099afe0a377c16888e"),
        (63, "75220b47218278e656f2013bb8f0c455a25eaf01e86c64924e9d48d89776d6f2"),
        (64, "7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c"),
        (65, "9537c5fdf120482f7d58d25e9ed583f52c02b4e304ea814db1633ad565aed7e9"),
        (119, "000b48d4edf0fa7bee3c6236ecd2785baa5db4eeb8bb54341b029e0d9fa5fb0c"),
        (120, "13f05a0b594787f5ecd315edc96141bd3243203d1b7d4f0836f37308b276ba98"),
    ];
    for (n, want) in edges {
        assert_eq!(hex(&vec![b'x'; n]), want, "{n} bytes");
    }
}

#[test]
fn sha256_and_hex_in_kio() {
    let src = r#"
fn main() -> List<Str> {
    let mut a = "a";
    while a.len() < 1000000 { a = a + a; }
    let million = a.slice(0, 1000000);
    ["".sha256().to_hex(), "abc".sha256().to_hex(), "abc".to_bytes().sha256().to_hex(),
     "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".sha256().to_hex(),
     "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu".sha256().to_hex(),
     million.sha256().to_hex(), "héllo".sha256().to_hex(), "abc".sha256().len().to_str()]
}"#;
    assert_eq!(
        shows(src),
        r#"["e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1", "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1", "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0", "3c48591d8d098a4538f5e013dfcf406e948eac4d3277b10bf614e295d6068179", "32"]"#
    );
}

#[test]
fn to_hex() {
    let src = r#"
fn main() -> List<Str> {
    let mut b = "".to_bytes();
    b.push(0); b.push(255); b.push(16); b.push(9); b.push(171);
    ["".to_bytes().to_hex(), b.to_hex(), "Az".to_bytes().to_hex()]
}"#;
    assert_eq!(shows(src), r#"["", "00ff1009ab", "417a"]"#);
}

/// L26: `Str.from_hex()`, `to_hex`'s inverse: an even number of hex digits, either case.
#[test]
fn from_hex() {
    let src = r#"
fn main() -> List<Str> {
    let mut b = "".to_bytes();
    b.push(0); b.push(255); b.push(16); b.push(9); b.push(171);
    let show = |o: Option<Bytes>| match o { Some(x) => x.len().to_str() + ":" + x.to_hex(), None => "none" };
    [show("".from_hex()), show("00fF1a".from_hex()), show("ABCDEF".from_hex()), show("abc".from_hex()),
     show("0g".from_hex()), show("0x00".from_hex()), show(" 00".from_hex()), show("é0".from_hex()),
     show(b.to_hex().from_hex()), (b.to_hex().from_hex() == Some(b)).to_str()]
}"#;
    assert_eq!(
        shows(src),
        r#"["0:", "3:00ff1a", "3:abcdef", "none", "none", "none", "none", "none", "5:00ff1009ab", "true"]"#
    );
    let (msg, _) = rejects("fn main() { \"00\".to_bytes().from_hex(); }");
    assert_eq!(msg, "no method `from_hex` on Bytes");
}

/// L27: `Bytes.try_to_str()`: `Some` for UTF-8, `None` where `to_str()` stops the run.
#[test]
fn try_to_str() {
    let src = r#"
fn main() -> List<Str> {
    let show = |h: Str| match h.from_hex().unwrap_or("".to_bytes()).try_to_str() {
        Some(s) => "some:" + s,
        None => "none",
    };
    let t = "héllo €😀";
    [show(""), show("616263"), show(t.to_bytes().to_hex()),
     show("c0af"), show("e080af"), show("f08082ac"),
     show("80"), show("61bf"),
     show("eda080"), show("edbfbf"),
     show("c3"), show("e282"), show("f09f98"), show("e28241"),
     show("f4908080"), show("ff"),
     (t.to_bytes().try_to_str().unwrap_or("?") == t.to_bytes().to_str()).to_str()]
}"#;
    assert_eq!(
        shows(src),
        concat!(
            r#"["some:", "some:abc", "some:héllo €😀", "#,
            r#""none", "none", "none", "none", "none", "none", "none", "#,
            r#""none", "none", "none", "none", "none", "none", "true"]"#
        )
    );
    let (msg, _) = rejects("fn main() { \"a\".try_to_str(); }");
    assert_eq!(msg, "no method `try_to_str` on Str");
    let src = "fn main() -> Str { \"80\".from_hex().unwrap_or(\"\".to_bytes()).to_str() }";
    assert_eq!(fails(src).0, "invalid UTF-8");
}

#[test]
fn ct_eq() {
    let src = r#"
fn main() -> List<Bool> {
    ["abc".ct_eq("abc"), "abc".ct_eq("abd"), "abc".ct_eq("ab"), "".ct_eq(""), "".ct_eq("a"),
     "x".to_bytes().ct_eq("x".to_bytes()), "xy".to_bytes().ct_eq("yx".to_bytes()),
     "abc".sha256().ct_eq("abc".to_bytes().sha256())]
}"#;
    assert_eq!(shows(src), "[true, false, false, true, false, true, false, true]");
    assert!(lang::digest::ct_eq(b"", b""));
    assert!(!lang::digest::ct_eq(b"a", b"ab"));
    assert!(!lang::digest::ct_eq(b"ab", b"ac"));
}

#[test]
fn rejections() {
    let base = "fn main() { let mut xs = [1, 2]; let ys = [1]; let o: Option<Int> = None; let r: Result<Int, Str> = Ok(1); let b = \"\".to_bytes(); ";
    let cases: &[(&str, &str, &str)] = &[
        (
            "xs.sort_by(|a| true);",
            "`sort_by` takes a function of 2 parameters, found a closure of 1 parameters",
            "`List<Int>.sort_by` is declared as `sort_by(fn(Int, Int) -> Bool)`",
        ),
        ("xs.sort_by(|a, b| a - b);", "expected Bool, found Int", ""),
        (
            "xs.sort_by_key(|a| a > 0);",
            "`sort_by_key` needs a function returning Int or Str, found fn(Int) -> Bool",
            "`List<Int>.sort_by_key` is declared as `sort_by_key(fn(Int) -> K)`",
        ),
        (
            "ys.sort_by(|a, b| a < b);",
            "cannot `sort_by` `ys`, it is not `mut`",
            "declare it with `let mut`",
        ),
        (
            "ys.reverse();",
            "cannot `reverse` `ys`, it is not `mut`",
            "declare it with `let mut`",
        ),
        (
            "b.extend(b);",
            "cannot `extend` `b`, it is not `mut`",
            "declare it with `let mut`",
        ),
        (
            "xs.join(\",\");",
            "cannot join List<Int>",
            "join works on List<Str>; make one first: `xs.map(|x| x.to_str()).join(sep)`",
        ),
        (
            "o.unwrap_or_else(|x| 1);",
            "`unwrap_or_else` takes a function of 0 parameters, found a closure of 1 parameters",
            "`Option<Int>.unwrap_or_else` is declared as `unwrap_or_else(fn() -> Int) -> Int`",
        ),
        (
            "r.unwrap_or_else(|| 1);",
            "`unwrap_or_else` takes a function of 1 parameter, found a closure of 0 parameters",
            "`Result<Int, Str>.unwrap_or_else` is declared as `unwrap_or_else(fn(Str) -> Int) -> Int`",
        ),
        ("o.unwrap_or_else(|| \"s\");", "expected Int, found Str", ""),
        (
            "1.min(2.0);",
            "expected Int, found Float",
            "`Int.min` is declared as `min(Int) -> Int`",
        ),
        (
            "b.ct_eq(\"s\");",
            "expected Bytes, found Str",
            "`Bytes.ct_eq` is declared as `ct_eq(Bytes) -> Bool`",
        ),
        ("\"s\".to_hex();", "no method `to_hex` on Str", ""),
    ];
    for (body, msg, hint) in cases {
        let src = format!("{base}{body} }}");
        let (m, h) = rejects(&src);
        assert_eq!(m, *msg, "{body}");
        if !hint.is_empty() {
            assert_eq!(h.as_deref(), Some(*hint), "{body}");
        }
    }
    let src = "fn less(a: Str, b: Str) -> Bool { a < b }\nfn main() { let mut xs = [1]; xs.sort_by(less); }";
    let (m, h) = rejects(src);
    assert_eq!(
        m,
        "`sort_by` needs a function taking Int, Int, found fn(Str, Str) -> Bool"
    );
    assert_eq!(
        h.as_deref(),
        Some("`List<Int>.sort_by` is declared as `sort_by(fn(Int, Int) -> Bool)`")
    );
}

/// The shared merge sort against std's stable sort, on random lists with many ties, and the
/// questions it asks: each pair once at most per merge step, about n log n in all.
#[test]
fn merge_sort_matches_std() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for n in (0..70).chain([255, 256, 257, 1000]) {
        let keys: Vec<u64> = (0..n).map(|_| next() % (n as u64 / 3 + 1)).collect();
        let mut asked = 0;
        let order = lang::sort::sort_by(n, |a, b| {
            asked += 1;
            Ok::<_, ()>(keys[a] < keys[b])
        })
        .unwrap();
        let mut want: Vec<u32> = (0..n as u32).collect();
        want.sort_by_key(|&i| keys[i as usize]);
        assert_eq!(order, want, "{n} elements");
        let log = (usize::BITS - n.leading_zeros()) as usize;
        assert!(asked <= n * log + n, "{n} elements: {asked} questions");
    }
    // Sorted input costs one question per merge.
    let mut asked = 0;
    lang::sort::sort_by(1024, |a, b| {
        asked += 1;
        Ok::<_, ()>(a < b)
    })
    .unwrap();
    assert_eq!(asked, 1023);
}

/// Shapes the compiled path handles apart: a function parameter as `less`, captures, `()`
/// payloads, floats, a moved argument to `extend`.
#[test]
fn ownership_shapes() {
    let src = r#"
fn sorted(xs: List<Str>, less: fn(Str, Str) -> Bool) -> List<Str> {
    let mut ys = xs;
    ys.sort_by(less);
    ys
}
fn main() -> List<Str> {
    let suffix = "!";
    let xs = ["bb", "a", "ccc", "a"];
    let mut ys = sorted(xs, |a, b| a + suffix > b + suffix);
    ys.sort_by_key(|s| s.len() + suffix.len());
    let mut zs = xs;
    zs.sort_by_key(|s| s + suffix);
    ys.push(xs.join("+"));
    ys.push(zs.join("+"));
    ys
}"#;
    assert_eq!(shows(src), r#"["a", "a", "bb", "ccc", "bb+a+ccc+a", "a+a+bb+ccc"]"#);
    let src = r#"
fn main() -> List<Float> {
    let mut xs = [2.5, 0.0 / 0.0, -1.0, 1.0 / 0.0, 0.5];
    xs.sort_by(|a, b| a < b);
    let o: Option<()> = None;
    o.unwrap_or_else(|| ());
    let r: Result<Float, Str> = Err("x");
    xs.push(r.unwrap_or_else(|e| e.len().to_float()).max(0.5));
    xs
}"#;
    assert_eq!(shows(src), "[0.5, 2.5, NaN, -1, inf, 1]");
    let src = r#"
fn main() -> Str {
    let mut a = "ab".to_bytes();
    let more = "cd".to_bytes();
    a.extend(more);
    let mut parts = [a.to_hex(), "x"];
    parts.reverse();
    parts.slice(0, 1).join("") + parts[1]
}"#;
    assert_eq!(shows(src), "x61626364");
}

/// In a generic function, with `T` from the instance.
#[test]
fn generic_sort_by() {
    let src = r#"
fn sorted<T>(xs: List<T>, less: fn(T, T) -> Bool) -> List<T> {
    let mut ys = xs;
    ys.sort_by(less);
    ys.reverse();
    ys
}
fn main() -> Str {
    let a = sorted([3, 1, 2], |x, y| x < y);
    let b = sorted(["b", "c", "a"], |x, y| x < y);
    a[0].to_str() + b.join("")
}"#;
    assert_eq!(shows(src), "3cba");
}

/// L17: `Bool.to_str()`, as Rust's `to_string`.
#[test]
fn bool_to_str() {
    let src = r#"
fn flag(n: Int) -> Bool { n > 1 }
fn main() -> Str {
    let t = true;
    t.to_str() + "," + false.to_str() + "," + flag(1).to_str() + "," + (1 < 2 && flag(2)).to_str()
}"#;
    assert_eq!(shows(src), "true,false,false,true");
    let (msg, hint) = rejects("fn main() { true.to_str(1); }");
    assert_eq!(msg, "`to_str` takes 0 arguments, found 1");
    assert_eq!(hint.as_deref(), Some("`Bool.to_str` is declared as `to_str() -> Str`"));
}

/// L21: `Str.chars()`, one `Str` per Unicode scalar value, as Rust's `chars`.
#[test]
fn str_chars() {
    assert_eq!(
        shows(r#"fn main() -> List<Str> { "abc".chars() }"#),
        r#"["a", "b", "c"]"#
    );
    assert_eq!(shows(r#"fn main() -> List<Str> { "".chars() }"#), "[]");
    assert_eq!(shows(r#"fn main() -> List<Str> { "é".chars() }"#), r#"["é"]"#);
    assert_eq!(shows(r#"fn main() -> List<Str> { "日本".chars() }"#), r#"["日", "本"]"#);
    // A combining accent is its own scalar value (displayed escaped, as Rust's `{:?}`); an
    // emoji is one 4-byte character.
    let src = r#"fn main() -> List<Str> { "é🦀!".chars() }"#;
    assert_eq!(shows(src), r#"["e", "\u{301}", "🦀", "!"]"#);
    let src = r#"
fn main() -> Str {
    let s = "añ日🦀";
    let mut out = "";
    for c in s.chars() {
        out = out + c.len().to_str() + ",";
    }
    out + s.chars().len().to_str() + "," + s.chars().join("")
}"#;
    assert_eq!(shows(src), "1,2,3,4,4,añ日🦀");
    let (msg, hint) = rejects(r#"fn main() { "a".chars(1); }"#);
    assert_eq!(msg, "`chars` takes 0 arguments, found 1");
    assert_eq!(
        hint.as_deref(),
        Some("`Str.chars` is declared as `chars() -> List<Str>`")
    );
    let (msg, at) = fails(r#"fn main() -> Int { "abc".split("").len() }"#);
    assert_eq!(
        msg,
        "`split` needs a non-empty separator; to split into characters, use `s.chars()`"
    );
    assert_eq!(at, r#""abc".split("")"#);
}

/// L22: `for b in bytes`, each byte as an `Int` in order, over the value as the loop started.
#[test]
fn for_over_bytes() {
    let src = r#"
fn main() -> List<Int> {
    let mut out: List<Int> = [];
    for b in "".to_bytes() { out.push(b); }
    let mut bs = "".to_bytes();
    bs.push(0);
    bs.push(255);
    bs.push(7);
    for b in bs { out.push(b); }
    out
}"#;
    assert_eq!(shows(src), "[0, 255, 7]");
    // Other variables change inside the loop; the bytes themselves change too, but the loop
    // runs over the value it started with.
    let src = r#"
fn main() -> Str {
    let mut b = "aé".to_bytes();
    let mut sum = 0;
    let mut n = 0;
    for x in b {
        sum += x;
        n = n + 1;
        b.push(x);
    }
    sum.to_str() + "," + n.to_str() + "," + b.len().to_str()
}"#;
    assert_eq!(shows(src), "461,3,6");
    // `break`, `continue`, nesting, and an expression evaluated once.
    let src = r#"
fn bytes(n: Int) -> Bytes { let mut b = "".to_bytes(); for i in 0..n { b.push(i * 10); } b }
fn main() -> Str {
    let mut s = "";
    for x in bytes(6) {
        if x == 10 { continue; }
        if x == 40 { break; }
        for y in bytes(2) { s = s + (x + y).to_str() + " "; }
    }
    s
}"#;
    assert_eq!(shows(src), "0 10 20 30 30 40 ");
}
