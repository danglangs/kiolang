//! The runtime's JSON against the interpreter's (lang/src/json.rs): every input parses to the
//! same encoding or fails with the same message, and nothing leaks.

use crate::desc::{self, Desc, Name, Slice, Table, Variant};
use crate::json;
use crate::obj::{self, from_slot, to_slot, Header};
use crate::tests::lock;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

const JSON: u32 = 6;
const LIST_JSON: u32 = 7;
const MAP_JSON: u32 = 8;
const RESULT: u32 = 9; // Result<Json, Str>
/// `Option` of Json, Str, Int, Float, Bool, List<Json>, Map<Str, Json>, from here on.
const OPT: u32 = 10;

const NAMES: &[u8] = b"JsonNullBoolNumFloatStrArrObjResultOkErrOptionNoneSome";

const fn name(off: u32, len: u32) -> Name {
    Name { off, len }
}

const fn variant(n: Name, start: u32, len: u32) -> Variant {
    Variant {
        name: n,
        slots: Slice { start, len },
    }
}

const fn option(k: u32) -> Desc {
    Desc::Enum {
        name: name(40, 6),
        variants: Slice {
            start: 9 + 2 * k,
            len: 2,
        },
    }
}

const DESCS: [Desc; 17] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
    Desc::Float,
    Desc::Enum {
        name: name(0, 4),
        variants: Slice { start: 0, len: 7 },
    },
    Desc::List { elem: JSON },
    Desc::Map {
        key: desc::STR,
        value: JSON,
    },
    Desc::Enum {
        name: name(29, 6),
        variants: Slice { start: 7, len: 2 },
    },
    option(0),
    option(1),
    option(2),
    option(3),
    option(4),
    option(5),
    option(6),
];

const SLOTS: [u32; 15] = [
    desc::BOOL,
    desc::INT,
    desc::FLOAT,
    desc::STR,
    LIST_JSON,
    MAP_JSON,
    JSON,
    desc::STR,
    JSON,
    desc::STR,
    desc::INT,
    desc::FLOAT,
    desc::BOOL,
    LIST_JSON,
    MAP_JSON,
];

const NO: Name = name(0, 0);
const FIELDS: [Name; 15] = [NO; 15];

const fn none_some(k: u32) -> [Variant; 2] {
    [variant(name(46, 4), 0, 0), variant(name(50, 4), 8 + k, 1)]
}

const VARIANTS: [Variant; 23] = {
    let mut v = [variant(NO, 0, 0); 23];
    v[0] = variant(name(4, 4), 0, 0); // Null
    v[1] = variant(name(8, 4), 0, 1); // Bool
    v[2] = variant(name(12, 3), 1, 1); // Num
    v[3] = variant(name(15, 5), 2, 1); // Float
    v[4] = variant(name(20, 3), 3, 1); // Str
    v[5] = variant(name(23, 3), 4, 1); // Arr
    v[6] = variant(name(26, 3), 5, 1); // Obj
    v[7] = variant(name(35, 2), 6, 1); // Ok
    v[8] = variant(name(37, 3), 7, 1); // Err
    let mut k = 0;
    while k < 7 {
        let [n, s] = none_some(k);
        v[9 + 2 * k as usize] = n;
        v[10 + 2 * k as usize] = s;
        k += 1;
    }
    v
};

const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    fields: &FIELDS,
    variants: &VARIANTS,
    names: NAMES,
};

fn text(p: *mut Header) -> String {
    // SAFETY: `p` is a live `Str`.
    String::from_utf8(unsafe { obj::bytes(p) }.to_vec()).unwrap()
}

fn dec(p: *mut Header) {
    // SAFETY: `p` is owned here.
    unsafe { obj::dec(&T, p) }
}

/// The runtime's parse as `Ok(encoding)` or `Err(message)`, freeing what it made.
fn runtime(s: &str) -> Result<String, String> {
    let src = obj::new_bytes(desc::STR, s.as_bytes());
    // SAFETY: `src` is a `Str`; RESULT is `Result<Json, Str>` in `T`.
    let r = unsafe { json::parse(&T, RESULT, src) };
    // SAFETY: `r` is a `Result` with one slot.
    let (v, x) = unsafe { (*obj::field::<u32>(r, obj::HEADER), from_slot(*obj::slot(&T, r, 0))) };
    let out = match v {
        // SAFETY: `x` is a `Json`.
        0 => Ok(take(unsafe { json::encode(x) })),
        _ => Err(text(x)),
    };
    dec(r);
    dec(src);
    out
}

fn take(p: *mut Header) -> String {
    let s = text(p);
    dec(p);
    s
}

/// The interpreter's parse, as `runtime`'s.
fn interpreter(s: &str) -> Result<String, String> {
    lang::json::parse(s, lang::ir::EnumId(0)).map(|v| lang::json::encode(&v))
}

fn same(s: &str) {
    assert_eq!(runtime(s), interpreter(s), "input {s:?}");
}

fn nest(open: &str, inner: &str, close: &str, n: usize) -> String {
    format!("{}{inner}{}", open.repeat(n), close.repeat(n))
}

#[test]
fn agrees_with_the_interpreter() {
    let _l = lock();
    let before = obj::live();
    let cases = [
        " \t\r\n null ",
        "true",
        "false",
        "nul",
        "[1, tru]",
        "",
        "0",
        "-12",
        "9223372036854775807",
        "-9223372036854775808",
        "9223372036854775808",
        "-9223372036854775809",
        "18446744073709551616",
        "18446744073709551615",
        "18446744073709551619",
        "-18446744073709551626",
        "184467440737095516150",
        "36893488147419103242",
        "1.5",
        "2e3",
        "-",
        "01",
        "\"a\\\"\\\\\\/\\b\\f\\n\\r\\t\"",
        "\"\\u00e9\\u20ac\"",
        "\"\\ud83d\\ude00\"",
        "\"\\ud83d\"",
        "\"\\ud83dx\"",
        "\"\\ude00\"",
        "\"\\ud83d\\u0041\"",
        "\"\\ud83d\\uzz00\"",
        "\"\\uzz00\"",
        "\"\\q\"",
        "\"abc",
        "\"abc\\",
        "\"a\nb\"",
        "\"0123456789abcdef\u{1}\"",
        "\"0123456789abcdé\\\"xyz\"",
        "[]",
        "{}",
        " [ 1 , [ 2 ] , { \"a\" : null } ] ",
        "{\"b\":1,\"a\":2}",
        "{\"a\":1,\"a\":2}",
        "{\"a\":[1],\"a\":{\"b\":\"c\"}}",
        "[1 2]",
        "[1,",
        "{\"a\" 1}",
        "{1:2}",
        "{\"a\":1 \"b\":2}",
        "{\"a\":1,}",
        "{\"a\":[1,{\"b\":2,}]}",
        "[1] x",
        "1.",
        ".5",
        "-.5",
        "1e",
        "1e+",
        "0.5.5",
        "1e999",
        "-1e999",
        "1e-999",
        "[2.50,-0.0,1e2]",
        "1e300",
        "5e-324",
        "1.7976931348623157e308",
        "123456789.123456789",
        "\"\u{7f}é€😀\"",
    ];
    for s in cases {
        same(s);
    }
    for n in [127, 128, 129] {
        same(&nest("[", "", "]", n));
        same(&nest("{\"k\":", "1", "}", n));
        same(&nest("[", "1", "", n));
    }
    assert_eq!(obj::live(), before);
}

#[test]
fn messages_name_the_offset() {
    let _l = lock();
    assert_eq!(runtime("[1 2]"), Err("expected ',' or ']' at byte 3".into()));
    assert_eq!(
        runtime(&nest("[", "", "]", 129)),
        Err("nesting deeper than 128 at byte 128".into())
    );
    assert_eq!(runtime("{\"a\":1,\"a\":[true]}"), Ok("{\"a\":[true]}".into()));
}

/// A tiny xorshift, so the cases are the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn mutated_documents_agree() {
    let _l = lock();
    let before = obj::live();
    let seed = "{\"a\":[1,-2.5e3,true,null,\"x\\n\\u00e9\\ud83d\\ude00\"],\"b\":{\"c\":\"€\",\"d\":[]}}";
    let alphabet = b"{}[]\":,.-+eE0123456789\\untrfals \t\n";
    let mut rng = Rng(0x6b69_6f4a);
    let rounds = if cfg!(miri) { 40 } else { 3000 };
    for _ in 0..rounds {
        let mut b = seed.as_bytes().to_vec();
        for _ in 0..1 + rng.below(3) {
            let i = rng.below(b.len());
            match rng.below(3) {
                0 => b[i] = alphabet[rng.below(alphabet.len())],
                1 => drop(b.remove(i)),
                _ => b.insert(i, alphabet[rng.below(alphabet.len())]),
            }
        }
        if let Ok(s) = core::str::from_utf8(&b) {
            same(s);
        }
    }
    assert_eq!(obj::live(), before);
}

/// `Json` of variant `v` with payload `x`.
fn make(v: u32, x: Option<u64>) -> *mut Header {
    let p = obj::alloc(&T, JSON, v);
    if let Some(x) = x {
        // SAFETY: a fresh variant with one slot.
        unsafe { *obj::slot(&T, p, 0) = x };
    }
    p
}

#[test]
fn encodes_what_parse_cannot_make() {
    let _l = lock();
    let before = obj::live();
    let floats = [f64::NAN, f64::INFINITY, -0.0, 1.0, 1e21, 0.1];
    let items: Vec<u64> = floats
        .iter()
        .map(|x| to_slot(make(json::FLOAT, Some(x.to_bits()))))
        .collect();
    let s = obj::new_bytes(desc::STR, b"q\"b\\n\nt\tc\x01\x1f");
    let mut items = items;
    items.push(to_slot(make(json::STR, Some(to_slot(s)))));
    let arr = make(json::ARR, Some(to_slot(obj::new_list(LIST_JSON, items))));
    // SAFETY: `arr` is a `Json`.
    let got = take(unsafe { json::encode(arr) });
    assert_eq!(
        got,
        "[null,null,-0.0,1.0,1000000000000000000000.0,0.1,\"q\\\"b\\\\n\\nt\\tc\\u0001\\u001f\"]"
    );
    dec(arr);
    // Deeper than parse allows: the encoder keeps its own stack.
    let mut j = make(json::NULL, None);
    for _ in 0..10_000 {
        j = make(json::ARR, Some(to_slot(obj::new_list(LIST_JSON, vec![to_slot(j)]))));
    }
    // SAFETY: `j` is a `Json`.
    let got = take(unsafe { json::encode(j) });
    assert_eq!(got.len(), 20_004);
    dec(j);
    assert_eq!(obj::live(), before);
}

/// The payload of `Option` `o` (one of `OPT`..) if it is `Some`, as a reference of the
/// caller's; `o` is freed.
fn some(o: *mut Header) -> Option<u64> {
    // SAFETY: `o` is an `Option` owned here; a pointer payload gains the caller's reference
    // before the `Some`'s goes.
    unsafe {
        let r = (*obj::field::<u32>(o, obj::HEADER) == 1).then(|| *obj::slot(&T, o, 0));
        if let Some(x) = r.filter(|_| T.is_ptr(SLOTS[(8 + (*o).desc - OPT) as usize])) {
            obj::inc(from_slot(x));
        }
        dec(o);
        r
    }
}

#[test]
fn accessors() {
    let _l = lock();
    let before = obj::live();
    let src = obj::new_bytes(
        desc::STR,
        b"{\"n\":7,\"f\":2.5,\"s\":\"hi\",\"t\":true,\"l\":[10,20],\"o\":{}}",
    );
    // SAFETY: as in `runtime`.
    let r = unsafe { json::parse(&T, RESULT, src) };
    // SAFETY: `r` is `Ok`.
    let j = unsafe { from_slot(*obj::slot(&T, r, 0)) };
    let key = |k: &str| obj::new_bytes(desc::STR, k.as_bytes());
    let opt = |k: u32| OPT + k;
    // SAFETY: every receiver is a `Json`, every `opt` the accessor's `Option`.
    unsafe {
        let get = |k: &str| {
            let k = key(k);
            let v = some(json::get(&T, opt(0), core::ptr::null_mut(), j, k));
            dec(k);
            v.map(from_slot)
        };
        let as_ =
            |x: *mut Header, want: u32, k: u32| some(json::as_variant(&T, opt(k), core::ptr::null_mut(), x, want));
        let n = get("n").unwrap();
        assert_eq!(as_(n, json::NUM, 2), Some(7));
        assert_eq!(as_(n, json::FLOAT, 3), Some(7f64.to_bits()));
        assert_eq!(as_(n, json::STR, 1), None);
        let f = get("f").unwrap();
        assert_eq!(as_(f, json::FLOAT, 3), Some(2.5f64.to_bits()));
        assert_eq!(as_(f, json::NUM, 2), None);
        let s = get("s").unwrap();
        let hi = from_slot(as_(s, json::STR, 1).unwrap());
        assert_eq!(text(hi), "hi");
        dec(hi);
        let t = get("t").unwrap();
        assert_eq!(as_(t, json::BOOL, 4), Some(1));
        let l = get("l").unwrap();
        let at = |i: i64| some(json::at(&T, opt(0), core::ptr::null_mut(), l, i)).map(from_slot);
        let second = at(1).unwrap();
        assert_eq!(as_(second, json::NUM, 2), Some(20));
        assert_eq!(at(2), None);
        assert_eq!(at(-1), None);
        let list = from_slot(as_(l, json::ARR, 5).unwrap());
        assert_eq!(obj::elems(list).len(), 2);
        let o = get("o").unwrap();
        let map = from_slot(as_(o, json::OBJ, 6).unwrap());
        assert_eq!(get("zz"), None);
        let k = key("n");
        assert_eq!(some(json::get(&T, opt(0), core::ptr::null_mut(), l, k)), None);
        dec(k);
        for p in [n, f, s, t, l, second, list, o, map] {
            dec(p);
        }
    }
    dec(r);
    dec(src);
    assert_eq!(obj::live(), before);
}
