use crate::desc::{self, Desc, Name, Slice, Table, Variant};
use crate::obj::{self, from_slot, Header};
use crate::tests::lock;
use crate::text::{self, Err};
use alloc::string::String;
use alloc::vec::Vec;

// List<Str>, Option<Int> = None | Some(Int), Option<Str>
const LIST_STR: u32 = 6;
pub(crate) const OPT: u32 = 7;
const OPT_STR: u32 = 8;
pub(crate) const OPT_FLOAT: u32 = 9;

const DESCS: [Desc; 10] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
    Desc::Float,
    Desc::List { elem: desc::STR },
    Desc::Enum {
        name: Name { off: 0, len: 6 },
        variants: Slice { start: 0, len: 2 },
    },
    Desc::Enum {
        name: Name { off: 0, len: 6 },
        variants: Slice { start: 2, len: 2 },
    },
    Desc::Enum {
        name: Name { off: 0, len: 6 },
        variants: Slice { start: 4, len: 2 },
    },
];
const SLOTS: [u32; 3] = [desc::INT, desc::STR, desc::FLOAT];
const FIELDS: [Name; 3] = [Name { off: 0, len: 0 }; 3];
const VARIANTS: [Variant; 6] = [
    Variant {
        name: Name { off: 6, len: 4 },
        slots: Slice { start: 0, len: 0 },
    },
    Variant {
        name: Name { off: 10, len: 4 },
        slots: Slice { start: 0, len: 1 },
    },
    Variant {
        name: Name { off: 6, len: 4 },
        slots: Slice { start: 0, len: 0 },
    },
    Variant {
        name: Name { off: 10, len: 4 },
        slots: Slice { start: 1, len: 1 },
    },
    Variant {
        name: Name { off: 6, len: 4 },
        slots: Slice { start: 0, len: 0 },
    },
    Variant {
        name: Name { off: 10, len: 4 },
        slots: Slice { start: 2, len: 1 },
    },
];
pub(crate) const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    fields: &FIELDS,
    variants: &VARIANTS,
    names: b"OptionNoneSome",
    plans: &[],
};

fn s(x: &str) -> *mut Header {
    text::new_str(x)
}

fn dec(p: *mut Header) {
    // SAFETY: tests pass live objects they own.
    unsafe { obj::dec(&T, p) }
}

/// A `Str`'s contents, then frees it.
fn take(p: *mut Header) -> String {
    // SAFETY: `p` is a live `Str` owned here.
    let r = String::from_utf8(unsafe { obj::bytes(p) }.to_vec()).unwrap();
    dec(p);
    r
}

/// A `List<Str>`'s contents, then frees it.
fn take_list(p: *mut Header) -> Vec<String> {
    // SAFETY: `p` is a live `List<Str>` owned here.
    let v: Vec<*mut Header> = unsafe { obj::elems(p).iter().map(|&x| from_slot(x)).collect() };
    let out = v
        .iter()
        .map(|&e| {
            // SAFETY: a live element.
            String::from_utf8(unsafe { obj::bytes(e) }.to_vec()).unwrap()
        })
        .collect();
    dec(p);
    out
}

fn split(a: &str, sep: &str) -> Result<Vec<String>, Err> {
    let (a, sep) = (s(a), s(sep));
    // SAFETY: live `Str`s and the `List<Str>` descriptor.
    let r = unsafe { text::split(LIST_STR, a, sep) };
    dec(a);
    dec(sep);
    r.map(take_list)
}

/// `Some(n)` as `Some(Some(n))`, `None` as `Some(None)`.
fn parse(x: &str, none: *mut Header) -> Option<i64> {
    let a = s(x);
    // SAFETY: a live `Str`; `none` is 0 or a `None` of `OPT`.
    let p = unsafe { text::parse_int(&T, OPT, none, a) };
    dec(a);
    // SAFETY: `p` is a live `Option<Int>`.
    let r = unsafe {
        match *obj::field::<u32>(p, 8) {
            0 => None,
            _ => Some(*obj::slot(&T, p, 0) as i64),
        }
    };
    dec(p);
    r
}

#[test]
fn split_cases() {
    let _g = lock();
    let base = obj::live();
    assert_eq!(split("a,b,,c", ","), Ok(["a", "b", "", "c"].map(String::from).to_vec()));
    assert_eq!(split("", ","), Ok(["".into()].to_vec()));
    assert_eq!(split("a--b--", "--"), Ok(["a", "b", ""].map(String::from).to_vec()));
    assert_eq!(split("abc", ""), Err(Err::EmptySeparator));
    assert_eq!(obj::live(), base);
}

/// The one-byte separator path (C8b6) against `str::split`: every separator position in and
/// around the 8-byte words, runs of separators, bytes that differ from the separator only in
/// the high bit or by a borrow (`!` and `\0` around ` `, 0xa0 inside `é`), and the tail.
#[test]
fn split_one_byte_matches_str_split() {
    let _g = lock();
    let base = obj::live();
    let alphabet = [" ", "a", "!", "\0", "\u{1f}", "é", "  ", "\u{a0}"];
    let mut x = 7u64;
    let n = if cfg!(miri) { 40 } else { 2000 };
    for len in 0..n {
        let mut a = String::new();
        for _ in 0..len % 40 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            a.push_str(alphabet[(x >> 33) as usize % alphabet.len()]);
        }
        for sep in [" ", "\0", "a"] {
            let want: Vec<String> = a.split(sep).map(String::from).collect();
            assert_eq!(split(&a, sep), Ok(want), "{a:?} at {sep:?}");
        }
    }
    assert_eq!(obj::live(), base);
}

#[test]
fn lines_trim_contains() {
    let _g = lock();
    let base = obj::live();
    let a = s("a\r\nb\n\nc\n");
    // SAFETY: live `Str`s.
    unsafe {
        assert_eq!(take_list(text::lines(LIST_STR, a)), ["a", "b", "", "c"]);
        dec(a);
        let e = s("");
        assert!(take_list(text::lines(LIST_STR, e)).is_empty());
        dec(e);
        let c = s("aé日本🦀");
        assert_eq!(take_list(text::chars(LIST_STR, c)), ["a", "é", "日", "本", "🦀"]);
        dec(c);
        let e = s("");
        assert!(take_list(text::chars(LIST_STR, e)).is_empty());
        dec(e);
        let t = s(" \t a b\n\u{a0}");
        assert_eq!(take(text::trim(t)), "a b");
        dec(t);
        let (h, el, x, em, he, elx) = (s("hello"), s("ell"), s("x"), s(""), s("he"), s("el"));
        assert!(text::contains(h, el) && !text::contains(h, x) && text::contains(h, em));
        assert!(text::starts_with(h, he) && !text::starts_with(h, elx));
        for p in [h, el, x, em, he, elx] {
            dec(p);
        }
    }
    assert_eq!(obj::live(), base);
}

#[test]
fn parse_int_cases() {
    let _g = lock();
    let base = obj::live();
    let pinned = obj::alloc(&T, OPT, 0);
    // SAFETY: `pinned` is live and ours.
    unsafe { (*pinned).rc = obj::PINNED };
    for none in [core::ptr::null_mut(), pinned] {
        assert_eq!(parse("42", none), Some(42));
        assert_eq!(parse("-7", none), Some(-7));
        assert_eq!(parse("+7", none), Some(7));
        assert_eq!(parse("-0", none), Some(0));
        assert_eq!(parse("-9223372036854775808", none), Some(i64::MIN));
        assert_eq!(parse("9223372036854775807", none), Some(i64::MAX));
        for bad in ["", "-", "+", "1.5", " 1", "1 ", "0x1", "9223372036854775808", "--1"] {
            assert_eq!(parse(bad, none), None, "{bad:?}");
        }
    }
    assert_eq!(obj::live(), base + 1);
    // Pinned: dec skips it, so free it by hand.
    // SAFETY: `pinned` is live; set a real count and drop it.
    unsafe {
        (*pinned).rc = 1;
        obj::dec(&T, pinned);
    }
    assert_eq!(obj::live(), base);
}

#[test]
fn bytes_and_str_conversions() {
    let _g = lock();
    let base = obj::live();
    // SAFETY: live objects of the right kinds.
    unsafe {
        let a = s("h\u{e9}llo \u{4e16}");
        let b = text::to_bytes(a);
        assert_eq!(text::len(a), 10);
        assert_eq!(text::len(b), 10);
        let back = text::to_str(b).unwrap();
        assert!(text::eq(a, back));
        for p in [a, b, back] {
            dec(p);
        }
        let bad = text::new_bytes(&[0x68, 0xff]);
        assert_eq!(text::to_str(bad), Err(Err::InvalidUtf8));
        dec(bad);
        let trunc = text::new_bytes(&"\u{e9}".as_bytes()[..1]);
        assert_eq!(text::to_str(trunc), Err(Err::InvalidUtf8));
        dec(trunc);
        let e = text::new_bytes(&[]);
        dec(text::to_str(e).unwrap());
        dec(e);
    }
    assert_eq!(obj::live(), base);
}

#[test]
fn concat_eq_int_to_str() {
    let _g = lock();
    let base = obj::live();
    let (a, b) = (s("ab"), s("cd"));
    // SAFETY: live `Str`s and `Bytes`.
    unsafe {
        let c = text::concat(a, b);
        assert_eq!(obj::bytes(c), b"abcd");
        assert_eq!(obj::bytes(a), b"ab");
        let (e1, e2) = (s(""), s(""));
        let e = text::concat(e1, e2);
        dec(e1);
        dec(e2);
        assert_eq!(text::len(e), 0);
        assert!(!text::eq(a, b) && text::eq(a, a) && text::eq(e, e) && !text::eq(c, a));
        let (x, y, z) = (text::new_bytes(b"a"), text::new_bytes(b"a"), text::new_bytes(b"b"));
        assert!(text::eq(x, y) && !text::eq(x, z));
        for p in [x, y, z, c, e] {
            dec(p);
        }
    }
    for (n, w) in [
        (0, "0"),
        (-7, "-7"),
        (i64::MAX, "9223372036854775807"),
        (i64::MIN, "-9223372036854775808"),
    ] {
        assert_eq!(take(text::int_to_str(n)), w);
    }
    // `int_text` (C8b6) against `{}` around every power of ten and at the ends.
    let mut ns = alloc::vec![0, 1, -1, i64::MAX, i64::MIN, i64::MIN + 1];
    let mut p = 1i64;
    while let Some(q) = p.checked_mul(10) {
        ns.extend([p - 1, p, p + 1, -p + 1, -p, -p - 1]);
        p = q;
    }
    ns.extend([p - 1, p, -p]);
    for n in ns {
        assert_eq!(text::int_text(n, &mut [0; 20]), alloc::format!("{n}").as_bytes());
    }
    dec(a);
    dec(b);
    assert_eq!(obj::live(), base);
}

#[test]
fn bytes_index_and_push() {
    let _g = lock();
    let base = obj::live();
    // SAFETY: `b` is a live, unshared `Bytes`, always the latest returned.
    unsafe {
        let mut b = text::new_bytes(b"");
        let mut caps = Vec::new();
        for i in 0..1000i64 {
            b = text::push(&T, b, i % 256).unwrap();
            let cap = *obj::field::<u32>(b, 12);
            if caps.last() != Some(&cap) {
                caps.push(cap);
            }
            assert!(cap as i64 > i);
        }
        assert!(caps.len() > 3 && caps.len() < 10, "{caps:?}");
        assert_eq!(text::len(b), 1000);
        for i in [0, 1, 255, 256, 999] {
            assert_eq!(text::index(b, i), Ok(i % 256));
        }
        assert_eq!(text::index(b, 1000), Err(Err::OutOfBounds));
        assert_eq!(text::index(b, -1), Err(Err::OutOfBounds));
        assert_eq!(text::push(&T, b, 256), Err(Err::ByteRange));
        assert_eq!(text::push(&T, b, -1), Err(Err::ByteRange));
        assert_eq!(text::len(b), 1000);
        // A copy-on-write push: the caller's `unique` copied a shared buffer.
        obj::inc(b);
        let u = obj::unique(&T, b);
        assert_ne!(u, b);
        let u = text::push(&T, u, 7).unwrap();
        assert_eq!((text::len(u), text::len(b)), (1001, 1000));
        dec(u);
        dec(b);
        // A tight buffer from `new_bytes` grows too, and a literal-sized Str-to-bytes does.
        let ab = s("ab");
        let t = text::to_bytes(ab);
        dec(ab);
        let t = text::push(&T, t, 99).unwrap();
        assert_eq!(obj::bytes(t), b"abc");
        dec(t);
    }
    assert_eq!(obj::live(), base);
}

#[test]
fn append_in_place_only_when_unique() {
    let _g = lock();
    let base = obj::live();
    // SAFETY: every pointer is a live `Str` owned here; `append` takes its first argument.
    unsafe {
        // Unique: grows in place, capacity doubling, so few moves.
        let x = s("xy");
        let mut a = s("");
        let mut moves = 0;
        for i in 0..1000 {
            let before = a;
            a = text::append(&T, a, x);
            moves += (a != before) as u32;
            assert_eq!(text::len(a), 2 * (i + 1));
        }
        assert!(moves < 12, "{moves} moves");
        assert!(obj::bytes(a).chunks(2).all(|c| c == b"xy"));
        dec(a);
        // Shared: `t` keeps its value; the result is a new object.
        let a = s("ab");
        obj::inc(a);
        let t = a;
        let r = text::append(&T, a, x);
        assert_ne!(r, t);
        assert_eq!((obj::bytes(t), obj::bytes(r)), (&b"ab"[..], &b"abxy"[..]));
        assert_eq!((*t).rc, 1);
        // `a = a + a`: two references to one object, so it is copied.
        obj::inc(r);
        let rr = text::append(&T, r, r);
        assert_eq!(obj::bytes(rr), b"abxyabxy");
        assert_eq!(obj::bytes(r), b"abxy");
        // `a += a` with `a` borrowed: one reference, which `append` takes; the copy is made
        // before it is dropped (a test build poisons freed bytes).
        let one = s("cd");
        let cc = text::append(&T, one, one);
        assert_eq!(obj::bytes(cc), b"cdcd");
        dec(cc);
        // A pinned literal is copied and never written.
        let lit = s("lit");
        (*lit).rc = obj::PINNED;
        let l2 = text::append(&T, lit, x);
        assert_eq!((obj::bytes(lit), obj::bytes(l2)), (&b"lit"[..], &b"litxy"[..]));
        (*lit).rc = 1;
        for p in [t, r, rr, l2, lit, x] {
            dec(p);
        }
    }
    assert_eq!(obj::live(), base);
}

/// A pinned `None` of `opt`, as codegen emits; `unpin` frees it.
fn pinned_none(opt: u32) -> *mut Header {
    let p = obj::alloc(&T, opt, 0);
    // SAFETY: `p` is live and ours.
    unsafe { (*p).rc = obj::PINNED };
    p
}

fn unpin(p: *mut Header) {
    // SAFETY: `p` is live; give it a real count and drop it.
    unsafe { (*p).rc = 1 };
    dec(p);
}

/// An `Option`'s payload as `f` reads it, then frees the option.
fn opt<R>(p: *mut Header, f: impl Fn(u64) -> R) -> Option<R> {
    // SAFETY: `p` is a live `Option`, owned here.
    let r = unsafe {
        match *obj::field::<u32>(p, 8) {
            0 => None,
            _ => Some(f(*obj::slot(&T, p, 0))),
        }
    };
    dec(p);
    r
}

fn str_slot(v: u64) -> String {
    // SAFETY: a live `Str` slot.
    String::from_utf8(unsafe { obj::bytes(from_slot(v)) }.to_vec()).unwrap()
}

/// Runs `f` on new `Str`s `a` and `b`, then frees them.
fn with2<R>(a: &str, b: &str, f: impl FnOnce(*mut Header, *mut Header) -> R) -> R {
    let (a, b) = (s(a), s(b));
    let r = f(a, b);
    dec(a);
    dec(b);
    r
}

#[test]
fn ends_with_cases() {
    let _g = lock();
    let base = obj::live();
    // SAFETY: live `Str`s.
    let e = |a, b| with2(a, b, |a, b| unsafe { text::ends_with(a, b) });
    assert!(e("abc", "bc") && e("abc", "") && e("", "") && e("abc", "abc"));
    assert!(!e("", "a") && !e("abc", "abcd") && !e("abc", "b"));
    assert_eq!(obj::live(), base);
}

#[test]
fn strip_prefix_and_suffix_cases() {
    let _g = lock();
    let base = obj::live();
    let pinned = pinned_none(OPT_STR);
    for none in [core::ptr::null_mut(), pinned] {
        // SAFETY: live `Str`s; `none` is null or a `None` of `OPT_STR`.
        let pre = |a, b| {
            with2(a, b, |a, b| {
                opt(unsafe { text::strip_prefix(&T, OPT_STR, none, a, b) }, str_slot)
            })
        };
        // SAFETY: as above.
        let suf = |a, b| {
            with2(a, b, |a, b| {
                opt(unsafe { text::strip_suffix(&T, OPT_STR, none, a, b) }, str_slot)
            })
        };
        assert_eq!(pre("abc", "ab"), Some("c".into()));
        assert_eq!(pre("abc", "abc"), Some("".into()));
        assert_eq!(pre("abc", ""), Some("abc".into()));
        assert_eq!(pre("abc", "b"), None);
        assert_eq!(pre("", "a"), None);
        assert_eq!(pre("éa", "é"), Some("a".into()));
        assert_eq!(suf("abc", "bc"), Some("a".into()));
        assert_eq!(suf("abc", "abc"), Some("".into()));
        assert_eq!(suf("abc", ""), Some("abc".into()));
        assert_eq!(suf("abc", "b"), None);
        assert_eq!(suf("", "a"), None);
    }
    // SAFETY: `pinned` is live.
    assert_eq!(unsafe { (*pinned).rc }, obj::PINNED);
    unpin(pinned);
    assert_eq!(obj::live(), base);
}

#[test]
fn find_cases() {
    let _g = lock();
    let base = obj::live();
    let pinned = pinned_none(OPT);
    for none in [core::ptr::null_mut(), pinned] {
        // SAFETY: live `Str`s; `none` is null or a `None` of `OPT`.
        let f = |a, b| {
            with2(a, b, |a, b| {
                opt(unsafe { text::find(&T, OPT, none, a, b) }, |v| v as i64)
            })
        };
        assert_eq!(f("abcb", "b"), Some(1));
        assert_eq!(f("abc", ""), Some(0));
        assert_eq!(f("", ""), Some(0));
        assert_eq!(f("abc", "z"), None);
        assert_eq!(f("", "a"), None);
        assert_eq!(f("éa", "a"), Some(2));
        assert_eq!(f("aaa", "aa"), Some(0));
    }
    unpin(pinned);
    assert_eq!(obj::live(), base);
}

#[test]
fn replace_cases() {
    let _g = lock();
    let base = obj::live();
    let r = |x: &str, from, to| {
        let x = s(x);
        // SAFETY: live `Str`s.
        let out = with2(from, to, |a, b| take(unsafe { text::replace(x, a, b) }));
        dec(x);
        out
    };
    assert_eq!(r("abab", "b", "xy"), "axyaxy");
    assert_eq!(r("aaa", "aa", "b"), "ba");
    assert_eq!(r("abc", "z", "y"), "abc");
    assert_eq!(r("abc", "", "-"), "-a-b-c-");
    assert_eq!(r("", "", "-"), "-");
    assert_eq!(r("abc", "b", ""), "ac");
    assert_eq!(r("héé", "é", "e"), "hee");
    assert_eq!(r("", "a", "b"), "");
    assert_eq!(obj::live(), base);
}

/// `Some(bits)` or `None` from an `Option` of `Int` or `Float`, then frees it.
pub(crate) fn take_option(p: *mut Header) -> Option<u64> {
    // SAFETY: `p` is a live `Option` with a scalar payload.
    let r = unsafe {
        match *obj::field::<u32>(p, 8) {
            0 => None,
            _ => Some(*obj::slot(&T, p, 0)),
        }
    };
    dec(p);
    r
}

#[test]
fn parse_float_and_to_str_cases() {
    let _g = lock();
    let base = obj::live();
    let parse = |x: &str| {
        let a = s(x);
        // SAFETY: a live `Str`; null for a new `None`.
        let p = unsafe { text::parse_float(&T, OPT_FLOAT, core::ptr::null_mut(), a) };
        dec(a);
        take_option(p).map(f64::from_bits)
    };
    // As Rust's `f64::from_str`, which the interpreter calls.
    for x in [
        "1",
        "-0",
        "1e21",
        "inf",
        "-infinity",
        "NaN",
        "1e400",
        ".5",
        "5.",
        "0x10",
        " 1",
        "",
        "+",
        "1_0",
        "e5",
    ] {
        let want = x.parse::<f64>().ok();
        let got = parse(x);
        assert_eq!(got.map(f64::to_bits), want.map(f64::to_bits), "{x:?}");
    }
    for x in [
        1.0,
        0.1 + 0.2,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.0,
        1e21,
        1e-7,
        5e-324,
        f64::MAX,
    ] {
        assert_eq!(take(text::float_to_str(x)), alloc::string::ToString::to_string(&x));
    }
    assert_eq!(obj::live(), base);
}

#[test]
fn slice_cases() {
    let _g = lock();
    let base = obj::live();
    let str_slice = |x: &str, a: i64, b: i64| {
        let p = s(x);
        // SAFETY: a live `Str`.
        let r = unsafe { text::slice(p, a, b) }.map(take);
        dec(p);
        r
    };
    assert_eq!(str_slice("hello", 1, 3), Ok("el".into()));
    assert_eq!(str_slice("hello", 0, 5), Ok("hello".into()));
    assert_eq!(str_slice("hello", 5, 5), Ok("".into()));
    assert_eq!(str_slice("", 0, 0), Ok("".into()));
    assert_eq!(str_slice("héllo", 1, 3), Ok("é".into()));
    for (a, b) in [
        (3, 2),
        (-1, 2),
        (0, 6),
        (6, 6),
        (0, -1),
        (i64::MIN, i64::MAX),
        (0, 1 << 40),
    ] {
        assert_eq!(str_slice("hello", a, b), Err(Err::SliceBounds), "{a}..{b}");
    }
    assert_eq!(str_slice("héllo", 1, 2), Err(Err::CharBoundary));
    assert_eq!(str_slice("héllo", 2, 4), Err(Err::CharBoundary));
    // Bounds come first, as in the interpreter.
    assert_eq!(str_slice("é", 1, 3), Err(Err::SliceBounds));

    let bytes_slice = |x: &[u8], a: i64, b: i64| {
        let p = text::new_bytes(x);
        // SAFETY: a live `Bytes`.
        let r = unsafe { text::bytes_slice(p, a, b) }.map(|q| {
            // SAFETY: a fresh `Bytes`.
            let v = unsafe { obj::bytes(q) }.to_vec();
            dec(q);
            v
        });
        dec(p);
        r
    };
    assert_eq!(bytes_slice(&[0xc3, 0xa9, 1], 1, 3), Ok(alloc::vec![0xa9, 1]));
    assert_eq!(bytes_slice(&[], 0, 0), Ok(alloc::vec![]));
    assert_eq!(bytes_slice(&[1, 2], 2, 1), Err(Err::SliceBounds));
    assert_eq!(bytes_slice(&[1, 2], 0, 3), Err(Err::SliceBounds));
    assert_eq!(obj::live(), base);
}

/// The word-at-a-time comparisons agree with `[u8]`'s own on pairs of short strings (bytes
/// above 0x7f included, which a signed compare would misorder), at lengths on both sides of a
/// word and differing at every position.
#[test]
fn word_compare_matches_bytes() {
    let mut xs: Vec<Vec<u8>> = Vec::new();
    for len in 0..19 {
        for fill in [0u8, b'a', 0xff] {
            let base = alloc::vec![fill; len];
            xs.push(base.clone());
            for i in 0..len {
                for b in [0u8, b'b', 0x80] {
                    let mut x = base.clone();
                    x[i] = b;
                    xs.push(x);
                }
            }
        }
    }
    // Miri is slow: every 41st string against every 13th.
    let (sa, sb) = if cfg!(miri) { (41, 13) } else { (1, 1) };
    for a in xs.iter().step_by(sa) {
        for b in xs.iter().step_by(sb) {
            assert_eq!(text::bytes_eq(a, b), a == b, "{a:?} {b:?}");
            assert_eq!(text::bytes_cmp(a, b), a.cmp(b), "{a:?} {b:?}");
            // Map keys compare their prefixes first (C8b5), with the same result.
            let (ka, kb) = (
                crate::obj::Key::Str(a.as_slice().into()),
                crate::obj::Key::Str(b.as_slice().into()),
            );
            assert_eq!(ka.cmp(&kb), a.cmp(b), "{a:?} {b:?}");
            assert_eq!(ka == kb, a == b, "{a:?} {b:?}");
        }
    }
}

/// Map keys order as a derived `Ord` would: `Int` before `Str`, `Str`s by bytes.
#[test]
fn keys_order_as_bytes() {
    use crate::obj::Key;
    let k = |s: &[u8]| Key::Str(s.into());
    assert!(Key::Int(5) < k(b""));
    assert!(Key::Int(-1) < Key::Int(0));
    assert!(k(b"abcdefgh") < k(b"abcdefgh\0"));
    assert!(k(b"abcdefgz") > k(b"abcdefgh\xff"));
    assert!(k(b"\x80") > k(b"\x7f\xff\xff\xff\xff\xff\xff\xff\xff"));
    assert_eq!(k(b"abcdefghij"), k(b"abcdefghij"));
    assert_ne!(k(b"abcdefghij"), k(b"abcdefghik"));
    assert_ne!(Key::Int(0), k(b""));
    // Equal zero-padded prefixes: the bytes decide.
    assert!(k(b"") < k(b"\0"));
    assert!(k(b"a") < k(b"a\0"));
    assert!(k(b"a\0") < k(b"a\0\0"));
    assert!(k(b"abcdefgh") < k(b"abcdefgh\0"));
    assert_ne!(k(b"a"), k(b"a\0"));
    // A probe compares as an owned key.
    // SAFETY: the bytes outlive the probe, which is only compared.
    assert_eq!(*unsafe { Key::probe(b"abcdefghij") }, k(b"abcdefghij"));
}

/// L11 (C14): case mapping, SHA-256, hex and `ct_eq` match lang's, and free what they make.
#[test]
fn l11_text() {
    let _g = lock();
    let base = obj::live();
    // SAFETY: live objects of the right kinds.
    unsafe {
        for x in [
            "Hello, World!",
            "",
            "stra\u{df}e",
            "\u{39f}\u{394}\u{39f}\u{3a3}",
            "\u{130}",
        ] {
            let a = s(x);
            assert_eq!(take(text::to_lowercase(a)), x.to_lowercase());
            assert_eq!(take(text::to_uppercase(a)), x.to_uppercase());
            dec(a);
        }
        let abc = s("abc");
        let h = text::sha256(abc);
        let hex = text::to_hex(h);
        assert_eq!(
            take(hex),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let b = text::to_bytes(abc);
        let h2 = text::sha256(b);
        assert!(text::ct_eq(h, h2));
        assert!(!text::ct_eq(h, b));
        assert!(text::ct_eq(abc, abc));
        let abd = s("abd");
        assert!(!text::ct_eq(abc, abd));
        for p in [abc, h, b, h2, abd] {
            dec(p);
        }
        let e = text::new_bytes(&[]);
        assert_eq!(take(text::to_hex(e)), "");
        dec(e);
        // L26: `from_hex`, the inverse.
        for (x, want) in [
            ("", Some(&[][..])),
            ("00fF1a", Some(&[0, 255, 26][..])),
            ("abc", None),
            ("0g", None),
        ] {
            let p = s(x);
            let got = text::from_hex_opt(p).map(|v| {
                let b = from_slot(v);
                let r = obj::bytes(b).to_vec();
                dec(b);
                r
            });
            assert_eq!(got.as_deref(), want);
            dec(p);
        }
    }
    assert_eq!(obj::live(), base);
}

/// `extend` appends in place when there is room, else moves to a larger buffer; the argument
/// is only read.
#[test]
fn extend_grows_and_borrows() {
    let _g = lock();
    let base = obj::live();
    // SAFETY: a unique `Bytes`, and another live one.
    unsafe {
        let mut b = text::new_bytes(b"ab");
        let more = text::new_bytes(b"0123456789");
        for _ in 0..10 {
            b = text::extend(&T, b, more);
        }
        let empty = text::new_bytes(&[]);
        b = text::extend(&T, b, empty);
        assert_eq!(text::len(b), 102);
        assert_eq!(&obj::bytes(b)[..12], b"ab0123456789");
        assert_eq!(&obj::bytes(b)[92..], b"0123456789");
        assert_eq!((*more).rc, 1);
        let room = obj::new_bytes_cap(desc::BYTES, b"x", 8);
        let same = text::extend(&T, room, more);
        assert_eq!(obj::bytes(same), b"x0123456789");
        for p in [b, more, empty, same] {
            dec(p);
        }
    }
    assert_eq!(obj::live(), base);
}
