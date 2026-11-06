use crate::desc::{self, Desc, Name, Slice, Table, Variant};
use crate::obj::{self, from_slot, Header};
use crate::tests::lock;
use crate::text::{self, Err};
use alloc::string::String;
use alloc::vec::Vec;

// List<Str>, Option<Int> = None | Some(Int)
const LIST_STR: u32 = 5;
const OPT: u32 = 6;

const DESCS: [Desc; 7] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
    Desc::List { elem: desc::STR },
    Desc::Enum {
        name: Name { off: 0, len: 6 },
        variants: Slice { start: 0, len: 2 },
    },
];
const SLOTS: [u32; 1] = [desc::INT];
const VARIANTS: [Variant; 2] = [
    Variant {
        name: Name { off: 6, len: 4 },
        slots: Slice { start: 0, len: 0 },
    },
    Variant {
        name: Name { off: 10, len: 4 },
        slots: Slice { start: 0, len: 1 },
    },
];
const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    variants: &VARIANTS,
    names: b"OptionNoneSome",
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
    let v: Vec<*mut Header> = unsafe { (*obj::field::<obj::List>(p, 8)).iter().map(|&x| from_slot(x)).collect() };
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
