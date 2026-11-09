use crate::coll::{self, Error};
use crate::desc::{self, Desc, Name, Slice, Table, Variant};
use crate::obj::{self, from_slot, to_slot, Header};
use alloc::vec;

const LIST_INT: u32 = 6;
const LIST_STR: u32 = 7;
const MAP_STR_INT: u32 = 8;
const MAP_INT_STR: u32 = 9;
const OPT_INT: u32 = 10;
const OPT_STR: u32 = 11;
const LIST_LIST: u32 = 12;
const CHAIN: u32 = 13;
const PAIR: u32 = 14;
const MAP_STR_LIST: u32 = 15;
const DEEP: u32 = 16; // List<DEEP>
const RES_STR: u32 = 17; // Result<Str, Str>
const LIST_FLOAT: u32 = 18;
const HASH_STR_INT: u32 = 19;
const HASH_INT_STR: u32 = 20;
const HASH_STR_LIST: u32 = 21;

const NAME: Name = Name { off: 0, len: 0 };
const DESCS: [Desc; 22] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
    Desc::Float,
    Desc::List { elem: desc::INT },
    Desc::List { elem: desc::STR },
    Desc::Map {
        key: desc::STR,
        value: desc::INT,
    },
    Desc::Map {
        key: desc::INT,
        value: desc::STR,
    },
    Desc::Enum {
        name: NAME,
        variants: Slice { start: 0, len: 2 },
    },
    Desc::Enum {
        name: NAME,
        variants: Slice { start: 2, len: 2 },
    },
    Desc::List { elem: LIST_INT },
    Desc::Enum {
        name: NAME,
        variants: Slice { start: 4, len: 2 },
    },
    Desc::Record {
        name: NAME,
        slots: Slice { start: 3, len: 2 },
    },
    Desc::Map {
        key: desc::STR,
        value: LIST_INT,
    },
    Desc::List { elem: DEEP },
    Desc::Enum {
        name: NAME,
        variants: Slice { start: 6, len: 2 },
    },
    Desc::List { elem: desc::FLOAT },
    Desc::HashMap {
        key: desc::STR,
        value: desc::INT,
    },
    Desc::HashMap {
        key: desc::INT,
        value: desc::STR,
    },
    Desc::HashMap {
        key: desc::STR,
        value: LIST_INT,
    },
];
const SLOTS: [u32; 6] = [desc::INT, desc::STR, CHAIN, desc::STR, LIST_INT, LIST_INT];
const FIELDS: [Name; 6] = [Name { off: 0, len: 0 }; 6];
const fn variant(start: u32, len: u32) -> Variant {
    Variant {
        name: NAME,
        slots: Slice { start, len },
    }
}
const VARIANTS: [Variant; 8] = [
    variant(0, 0),
    variant(0, 1),
    variant(0, 0),
    variant(1, 1),
    variant(0, 0),
    variant(2, 1),
    variant(1, 1),
    variant(1, 1),
];
const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    fields: &FIELDS,
    variants: &VARIANTS,
    names: b"",
    plans: &[],
};

use crate::tests::lock;

fn rc(p: *mut Header) -> u32 {
    // SAFETY: tests read only live objects.
    unsafe { (*p).rc }
}

fn dec(p: *mut Header) {
    // SAFETY: tests dec only references they own.
    unsafe { obj::dec(&T, p) }
}

fn s(x: &str) -> u64 {
    to_slot(obj::new_bytes(desc::STR, x.as_bytes()))
}

fn ints(xs: &[i64]) -> *mut Header {
    coll::list_new(LIST_INT, xs.iter().map(|&n| n as u64).collect())
}

fn strs(xs: &[&str]) -> *mut Header {
    coll::list_new(LIST_STR, xs.iter().map(|x| s(x)).collect())
}

/// The ints in list `p`.
fn contents(p: *mut Header) -> alloc::vec::Vec<i64> {
    // SAFETY: `p` is a live `List<Int>`.
    unsafe {
        (0..coll::list_len(p))
            .map(|i| coll::list_get(&T, p, i).unwrap() as i64)
            .collect()
    }
}

fn str_of(v: u64) -> alloc::vec::Vec<u8> {
    // SAFETY: `v` is a live `Str`.
    unsafe { obj::bytes(from_slot(v)) }.to_vec()
}

/// Element `i` of a `List<Str>`, as bytes; the reference `list_get` returns is released.
fn str_at(l: *mut Header, i: i64) -> alloc::vec::Vec<u8> {
    // SAFETY: `l` is a live `List<Str>`; the result is an owned `Str`.
    unsafe {
        let v = coll::list_get(&T, l, i).unwrap();
        let b = str_of(v);
        dec(from_slot(v));
        b
    }
}

/// A callee that takes a reference, makes it unique and returns it.
fn callee(p: *mut Header) -> *mut Header {
    // SAFETY: `p` is live and owned.
    unsafe { obj::inc(p) };
    // SAFETY: as above.
    unsafe { obj::unique(&T, p) }
}

#[test]
fn list_basics() {
    let _l = lock();
    let before = obj::live();
    let p = ints(&[1, 2]);
    // SAFETY: `p` is a unique `List<Int>`.
    unsafe {
        coll::list_push(p, 3);
        assert_eq!(coll::list_len(p), 3);
        coll::list_set(&T, p, 0, 9).unwrap();
        *coll::list_slot(p, 2).unwrap() += 1;
    }
    assert_eq!(contents(p), [9, 2, 4]);
    dec(p);
    assert_eq!(obj::live(), before);
}

#[test]
fn index_bounds() {
    let _l = lock();
    let before = obj::live();
    let p = ints(&[1, 2]);
    for i in [2, 5, -1, i64::MIN, i64::MAX] {
        // SAFETY: `p` is a unique `List<Int>`.
        unsafe {
            assert_eq!(coll::list_get(&T, p, i), Err(Error::Index { index: i, len: 2 }));
            assert_eq!(coll::list_set(&T, p, i, 0), Err(Error::Index { index: i, len: 2 }));
            assert_eq!(coll::list_slot(p, i), Err(Error::Index { index: i, len: 2 }));
        }
    }
    assert_eq!(contents(p), [1, 2]);
    dec(p);
    assert_eq!(obj::live(), before);
}

#[test]
fn pointer_elements_are_counted() {
    let _l = lock();
    let before = obj::live();
    let p = strs(&["a", "b"]);
    // SAFETY: `p` is a unique `List<Str>`; each result is owned.
    unsafe {
        let a = from_slot(coll::list_get(&T, p, 0).unwrap());
        assert_eq!(rc(a), 2);
        coll::list_set(&T, p, 0, s("c")).unwrap();
        assert_eq!(rc(a), 1);
        assert_eq!(obj::bytes(a), b"a");
        dec(a);
        coll::list_push(p, s("d"));
    }
    assert_eq!(str_at(p, 2), b"d");
    dec(p);
    assert_eq!(obj::live(), before);
}

#[test]
fn set_with_value_aliasing_the_old_element() {
    let _l = lock();
    let before = obj::live();
    let p = strs(&["a"]);
    // SAFETY: `p` is a unique `List<Str>`; `v` is an owned reference to the element.
    unsafe {
        let v = coll::list_get(&T, p, 0).unwrap();
        coll::list_set(&T, p, 0, v).unwrap();
        assert_eq!(rc(from_slot(v)), 1);
    }
    assert_eq!(str_at(p, 0), b"a");
    dec(p);
    assert_eq!(obj::live(), before);
}

#[test]
fn copy_on_write_through_a_callee() {
    let _l = lock();
    let before = obj::live();
    let a = ints(&[1]);
    // bump(xs) { let mut ys = xs; ys[0] += 1; ys.push(0); ys }
    let b = callee(a);
    // SAFETY: `b` is unique, a fresh copy.
    unsafe {
        let v = coll::list_get(&T, b, 0).unwrap();
        coll::list_set(&T, b, 0, v + 1).unwrap();
        coll::list_push(b, 0);
    }
    assert_ne!(a, b);
    assert_eq!(contents(a), [1]);
    assert_eq!(contents(b), [2, 0]);
    dec(a);
    dec(b);

    // An unshared list is changed in place.
    let c = ints(&[1]);
    let d = callee(c);
    assert_ne!(c, d);
    dec(d);
    assert_eq!(rc(c), 1);
    dec(c);

    // Nested: sharing the outer list shares the inner; unique copies one level and inc's.
    let inner = ints(&[7]);
    let outer = coll::list_new(LIST_LIST, vec![to_slot(inner)]);
    let outer2 = callee(outer);
    assert_eq!(rc(inner), 2);
    // SAFETY: `outer2` is unique; its element is shared, so `unique` copies it first.
    unsafe {
        let i = from_slot(coll::list_get(&T, outer2, 0).unwrap());
        let i = obj::unique(&T, i);
        coll::list_push(i, 8);
        coll::list_set(&T, outer2, 0, to_slot(i)).unwrap();
    }
    assert_eq!(contents(inner), [7]);
    dec(outer);
    dec(outer2);
    assert_eq!(obj::live(), before);
}

/// A `Map`'s keys in key order, a `HashMap`'s in insertion order.
#[test]
fn map_keys_are_in_order() {
    let _l = lock();
    let before = obj::live();
    let cases = [
        (MAP_INT_STR, MAP_STR_INT, [-1, 2, 3], ["", "a", "ab", "b", "c"]),
        (HASH_INT_STR, HASH_STR_INT, [3, -1, 2], ["b", "a", "c", "", "ab"]),
    ];
    for (int_str, str_int, ints, strs) in cases {
        let m = coll::map_new(&T, int_str);
        // SAFETY: `m` is a unique map from `Int` to `Str`.
        unsafe {
            for (k, v) in [(3i64, "x"), (-1, "y"), (2, "z")] {
                coll::map_insert(&T, m, k as u64, s(v));
            }
            let ks = coll::map_keys(&T, m, LIST_INT);
            assert_eq!(contents(ks), ints);
            dec(ks);
        }
        dec(m);

        let m = coll::map_new(&T, str_int);
        // SAFETY: `m` is a unique map from `Str` to `Int`; each key is read, so the test still
        // owns it.
        unsafe {
            for (k, v) in [("b", 1), ("a", 2), ("c", 3), ("", 4), ("ab", 5)] {
                let k = s(k);
                coll::map_insert(&T, m, k, v);
                dec(from_slot(k));
            }
            let ks = coll::map_keys(&T, m, LIST_STR);
            let got: alloc::vec::Vec<_> = (0..5).map(|i| str_at(ks, i)).collect();
            assert_eq!(got, strs.map(str::as_bytes));
            dec(ks);
        }
        dec(m);
    }
    assert_eq!(obj::live(), before);
}

#[test]
fn map_get_insert_remove() {
    for d in [MAP_INT_STR, HASH_INT_STR] {
        map_get_insert_remove_of(d);
    }
}

fn map_get_insert_remove_of(d: u32) {
    let _l = lock();
    let before = obj::live();
    let m = coll::map_new(&T, d);
    // SAFETY: `m` is a unique map from `Int` to `Str`.
    unsafe {
        let none = coll::map_get(&T, m, 1, OPT_STR, core::ptr::null_mut());
        assert_eq!(*field_u32(none), 0);
        dec(none);

        coll::map_insert(&T, m, 1, s("old"));
        let v = s("new");
        coll::map_insert(&T, m, 1, s("ignored"));
        coll::map_insert(&T, m, 1, v);
        coll::map_insert(&T, m, 2, s("two"));
        coll::map_remove(&T, m, 2);
        coll::map_remove(&T, m, 9);
        assert_eq!(coll::map_len(&T, m), 1);

        let some = coll::map_get(&T, m, 1, OPT_STR, core::ptr::null_mut());
        assert_eq!(*field_u32(some), 1);
        assert_eq!(rc(from_slot(v)), 2);
        assert_eq!(str_of(*obj::slot(&T, some, 0)), b"new");
        dec(some);
        assert_eq!(rc(from_slot(v)), 1);
    }
    dec(m);
    assert_eq!(obj::live(), before);
}

fn field_u32(p: *mut Header) -> *mut u32 {
    // SAFETY: an enum holds its variant at 8.
    unsafe { obj::field(p, 8) }
}

#[test]
fn map_with_int_values_and_str_keys() {
    for d in [MAP_STR_INT, HASH_STR_INT] {
        map_with_int_values_and_str_keys_of(d);
    }
}

fn map_with_int_values_and_str_keys_of(d: u32) {
    let _l = lock();
    let before = obj::live();
    let m = coll::map_new(&T, d);
    // SAFETY: `m` is a unique map from `Str` to `Int`.
    unsafe {
        let k = s("a");
        coll::map_insert(&T, m, k, 10);
        coll::map_insert(&T, m, k, 20);
        let b = s("b");
        coll::map_insert(&T, m, b, 5);
        dec(from_slot(b));
        coll::map_remove(&T, m, k);
        let g = coll::map_get(&T, m, k, OPT_INT, core::ptr::null_mut());
        assert_eq!(*field_u32(g), 0);
        dec(g);
        dec(from_slot(k));
        let k = s("b");
        let g = coll::map_get(&T, m, k, OPT_INT, core::ptr::null_mut());
        assert_eq!(*obj::slot(&T, g, 0), 5);
        dec(g);
        dec(from_slot(k));
    }
    dec(m);
    assert_eq!(obj::live(), before);
}

/// `Str` keys are looked up borrowed (C8b4): `get`, `remove` and an `insert` of a key already
/// present keep nothing of the key passed, which is freed (poisoned in tests) right after; the
/// map's own copy, made by the first `insert`, is the one it keeps.
#[test]
fn str_keys_are_looked_up_borrowed() {
    for d in [MAP_STR_INT, HASH_STR_INT] {
        str_keys_are_looked_up_borrowed_of(d);
    }
}

fn str_keys_are_looked_up_borrowed_of(d: u32) {
    let _l = lock();
    let before = obj::live();
    let m = coll::map_new(&T, d);
    let with = |k: &str, f: &dyn Fn(u64)| {
        let k = s(k);
        f(k);
        dec(from_slot(k));
    };
    let get = |k: u64| {
        // SAFETY: `m` is a `Map<Str, Int>` and `k` a live `Str`.
        unsafe {
            let g = coll::map_get(&T, m, k, OPT_INT, core::ptr::null_mut());
            let v = (*field_u32(g) == 1).then(|| *obj::slot(&T, g, 0) as i64);
            dec(g);
            v
        }
    };
    // SAFETY: `m` is a unique `Map<Str, Int>`, each key a live `Str`.
    unsafe {
        with("a key longer than a word", &|k| coll::map_insert(&T, m, k, 1));
        with("a key longer than a word", &|k| coll::map_insert(&T, m, k, 2));
        with("a key longer than a wore", &|k| coll::map_insert(&T, m, k, 3));
        with("b", &|k| coll::map_insert(&T, m, k, 4));
        with("b", &|k| coll::map_remove(&T, m, k));
        with("b", &|k| coll::map_remove(&T, m, k));
        with("a key longer than a word", &|k| assert_eq!(get(k), Some(2)));
        with("a key longer than a wore", &|k| assert_eq!(get(k), Some(3)));
        with("b", &|k| assert_eq!(get(k), None));
        let ks = coll::map_keys(&T, m, LIST_STR);
        assert_eq!(str_at(ks, 0), b"a key longer than a word");
        assert_eq!((coll::map_len(&T, m), coll::list_len(ks)), (2, 2));
        dec(ks);
    }
    dec(m);
    assert_eq!(obj::live(), before);
}

#[test]
fn map_copy_on_write_through_a_callee() {
    for d in [MAP_STR_LIST, HASH_STR_LIST] {
        map_copy_on_write_through_a_callee_of(d);
    }
}

fn map_copy_on_write_through_a_callee_of(d: u32) {
    let _l = lock();
    let before = obj::live();
    let l = ints(&[1]);
    let m = coll::map_new(&T, d);
    let k = s("k");
    // SAFETY: `m` is a unique `Map<Str, List<Int>>`.
    unsafe { coll::map_insert(&T, m, k, to_slot(l)) };
    let m2 = callee(m);
    assert_ne!(m, m2);
    assert_eq!(rc(l), 2);
    // SAFETY: `m2` is unique.
    unsafe { coll::map_remove(&T, m2, k) };
    assert_eq!(rc(l), 1);
    // SAFETY: `m` is live.
    assert_eq!(unsafe { coll::map_len(&T, m) }, 1);
    dec(from_slot(k));
    dec(m);
    dec(m2);
    assert_eq!(obj::live(), before);
}

fn eq(d: u32, a: *mut Header, b: *mut Header) -> bool {
    // SAFETY: both are live values of `d`.
    unsafe { coll::eq(&T, d, to_slot(a), to_slot(b)) }
}

#[test]
fn equality() {
    let _l = lock();
    let before = obj::live();
    let (a, b, c, d) = (ints(&[1, 2]), ints(&[1, 2]), ints(&[1, 3]), ints(&[1]));
    assert!(eq(LIST_INT, a, a) && eq(LIST_INT, a, b) && !eq(LIST_INT, a, c) && !eq(LIST_INT, a, d));
    // SAFETY: scalars.
    unsafe {
        assert!(coll::eq(&T, desc::INT, 3, 3) && !coll::eq(&T, desc::INT, 3, 4));
        assert!(coll::eq(&T, desc::BOOL, 1, 1) && coll::eq(&T, desc::UNIT, 0, 0));
    }
    let (x, y) = (strs(&["a", "b"]), strs(&["a", "b"]));
    assert!(eq(LIST_STR, x, y));
    // SAFETY: `y` is a unique `List<Str>`.
    unsafe { coll::list_set(&T, y, 1, s("c")).unwrap() };
    assert!(!eq(LIST_STR, x, y));

    // Either kind of map: the same keys and values, inserted in any order.
    let mut maps = alloc::vec::Vec::new();
    for d in [MAP_STR_INT, HASH_STR_INT] {
        let mk = |ks: &[(&str, i64)]| {
            let m = coll::map_new(&T, d);
            for &(k, v) in ks {
                let k = s(k);
                // SAFETY: `m` is a unique map from `Str` to `Int`.
                unsafe { coll::map_insert(&T, m, k, v as u64) };
                dec(from_slot(k));
            }
            m
        };
        let (m1, m2) = (mk(&[("a", 1), ("b", 2)]), mk(&[("b", 2), ("a", 1)]));
        assert!(eq(d, m1, m2) && eq(d, m1, m1));
        let others = [mk(&[("a", 1), ("b", 3)]), mk(&[("a", 1), ("c", 2)]), mk(&[("a", 1)])];
        for &o in &others {
            assert!(!eq(d, m1, o) && !eq(d, o, m1));
        }
        maps.extend([m1, m2]);
        maps.extend(others);
    }

    // A record holding a Str and a list; an enum with and without payload.
    let pair = |name: &str, xs: &[i64]| {
        let p = obj::alloc(&T, PAIR, 0);
        // SAFETY: a fresh `Pair` with two slots.
        unsafe {
            *obj::slot(&T, p, 0) = s(name);
            *obj::slot(&T, p, 1) = to_slot(ints(xs));
        }
        p
    };
    let (p, q, r, r2) = (pair("n", &[1]), pair("n", &[1]), pair("n", &[2]), pair("m", &[1]));
    assert!(eq(PAIR, p, q) && !eq(PAIR, p, r) && !eq(PAIR, p, r2));
    let some = |n: i64| {
        let o = obj::alloc(&T, OPT_INT, 1);
        // SAFETY: a fresh `Some` with one slot.
        unsafe { *obj::slot(&T, o, 0) = n as u64 };
        o
    };
    let (s4, s4b, s5, none, none2) = (
        some(4),
        some(4),
        some(5),
        obj::alloc(&T, OPT_INT, 0),
        obj::alloc(&T, OPT_INT, 0),
    );
    assert!(eq(OPT_INT, s4, s4b) && !eq(OPT_INT, s4, s5) && !eq(OPT_INT, s4, none) && eq(OPT_INT, none, none2));

    let ll = |xs: &[&[i64]]| coll::list_new(LIST_LIST, xs.iter().map(|x| to_slot(ints(x))).collect());
    let (l1, l2, l3) = (ll(&[&[1], &[2, 3]]), ll(&[&[1], &[2, 3]]), ll(&[&[1], &[2, 4]]));
    assert!(eq(LIST_LIST, l1, l2) && !eq(LIST_LIST, l1, l3));

    for p in [a, b, c, d, x, y, p, q, r, r2, s4, s4b, s5, none, none2, l1, l2, l3] {
        dec(p);
    }
    maps.into_iter().for_each(dec);
    assert_eq!(obj::live(), before);
}

#[test]
fn float_equality_is_ieee() {
    let _l = lock();
    let before = obj::live();
    let floats = |xs: &[f64]| coll::list_new(LIST_FLOAT, xs.iter().map(|x| x.to_bits()).collect());
    let (nan, z, nz, one) = (floats(&[f64::NAN]), floats(&[0.0]), floats(&[-0.0]), floats(&[1.0]));
    // The same object holding a NaN is not equal to itself, as in the interpreter.
    assert!(!eq(LIST_FLOAT, nan, nan) && eq(LIST_FLOAT, z, nz) && eq(LIST_FLOAT, one, one));
    assert!(!eq(LIST_FLOAT, z, one));
    // SAFETY: scalars.
    unsafe {
        let f = |a: f64, b: f64| coll::eq(&T, desc::FLOAT, a.to_bits(), b.to_bits());
        assert!(!f(f64::NAN, f64::NAN) && f(0.0, -0.0) && f(1.5, 1.5) && !f(1.0, 2.0));
        assert!(coll::list_contains(&T, z, (-0.0f64).to_bits()) && !coll::list_contains(&T, nan, f64::NAN.to_bits()));
    }
    [nan, z, nz, one].into_iter().for_each(dec);
    assert_eq!(obj::live(), before);
}

#[test]
fn deep_equality_does_not_recurse() {
    let _l = lock();
    let depth = if cfg!(miri) { 1_000 } else { 1_000_000 };
    let before = obj::live();
    let chain = || {
        let mut p = obj::alloc(&T, CHAIN, 0);
        for _ in 0..depth {
            let q = obj::alloc(&T, CHAIN, 1);
            // SAFETY: a fresh `Link`, one slot.
            unsafe { *obj::slot(&T, q, 0) = to_slot(p) };
            p = q;
        }
        p
    };
    let (a, b) = (chain(), chain());
    assert!(eq(CHAIN, a, b));
    // Differ at the very bottom: the end of `b` becomes a Link to an End.
    let mut bottom = b;
    // SAFETY: walking `Link` slots down to the `End`.
    unsafe {
        loop {
            let next = from_slot(*obj::slot(&T, bottom, 0));
            if *field_u32(next) == 0 {
                break;
            }
            bottom = next;
        }
        let end = obj::alloc(&T, CHAIN, 0);
        let link = obj::alloc(&T, CHAIN, 1);
        *obj::slot(&T, link, 0) = to_slot(end);
        let old = core::mem::replace(&mut *obj::slot(&T, bottom, 0), to_slot(link));
        obj::dec(&T, from_slot(old));
    }
    assert!(!eq(CHAIN, a, b));
    dec(a);
    dec(b);
    assert_eq!(obj::live(), before);
}

#[test]
fn deep_lists_are_equal_and_freed() {
    let _l = lock();
    let depth = if cfg!(miri) { 1_000 } else { 1_000_000 };
    let before = obj::live();
    let nest = || {
        let mut p = coll::list_new(DEEP, vec![]);
        for _ in 0..depth {
            p = coll::list_new(DEEP, vec![to_slot(p)]);
        }
        p
    };
    let (a, b) = (nest(), nest());
    assert!(eq(DEEP, a, b));
    dec(a);
    dec(b);
    let wide = |n: i64| coll::list_new(LIST_INT, (0..depth as i64).map(|i| (i + n) as u64).collect());
    let (a, b, c) = (wide(0), wide(0), wide(1));
    assert!(eq(LIST_INT, a, b) && !eq(LIST_INT, a, c));
    dec(a);
    dec(b);
    dec(c);
    assert_eq!(obj::live(), before);
}

/// A pinned `None` of `opt`, as codegen emits.
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

/// The variant of an `Option` or `Result`, and its payload slot if it has one.
fn parts(p: *mut Header) -> (u32, Option<u64>) {
    // SAFETY: `p` is a live `Option` or `Result` of `T`.
    unsafe {
        let v = *field_u32(p);
        let (_, ds) = obj::record(&T, p);
        (v, (!ds.is_empty()).then(|| *obj::slot(&T, p, 0)))
    }
}

#[test]
fn list_contains_compares_by_value() {
    let _l = lock();
    let before = obj::live();
    let (xs, empty, ss) = (ints(&[1, 2, -3]), ints(&[]), strs(&["a", "bc"]));
    let (bc, b) = (s("bc"), s("b"));
    // SAFETY: live lists and slot values of their element types.
    unsafe {
        assert!(coll::list_contains(&T, xs, 2) && coll::list_contains(&T, xs, -3i64 as u64));
        assert!(!coll::list_contains(&T, xs, 3) && !coll::list_contains(&T, empty, 1));
        assert!(coll::list_contains(&T, ss, bc) && !coll::list_contains(&T, ss, b));
    }
    for p in [xs, empty, ss, from_slot(bc), from_slot(b)] {
        dec(p);
    }
    assert_eq!(obj::live(), before);
}

#[test]
fn list_sort_ints_signed_and_strs_by_bytes() {
    let _l = lock();
    let before = obj::live();
    let xs = ints(&[3, -1, 2, i64::MIN, 2, i64::MAX, 0]);
    let empty = ints(&[]);
    let ss = strs(&["b", "B", "a", "é", "", "ab"]);
    // SAFETY: unique lists of `Int` and `Str`.
    unsafe {
        coll::list_sort(&T, xs);
        coll::list_sort(&T, empty);
        coll::list_sort(&T, ss);
    }
    assert_eq!(contents(xs), [i64::MIN, -1, 0, 2, 2, 3, i64::MAX]);
    assert_eq!(contents(empty), []);
    let got: alloc::vec::Vec<_> = (0..6).map(|i| str_at(ss, i)).collect();
    assert_eq!(got, [&b""[..], b"B", b"a", b"ab", b"b", "é".as_bytes()]);
    for p in [xs, empty, ss] {
        dec(p);
    }
    assert_eq!(obj::live(), before);
}

/// `pop` moves the element into the `Some`: its count is unchanged, and freeing the `Some`
/// frees it.
#[test]
fn list_pop_moves_the_element() {
    let _l = lock();
    let before = obj::live();
    let none = pinned_none(OPT_STR);
    let ss = strs(&["a", "b"]);
    // SAFETY: a unique `List<Str>`; `none` is a `None` of `OPT_STR`.
    unsafe {
        let b = coll::list_get(&T, ss, 1).unwrap();
        dec(from_slot(b));
        let p = coll::list_pop(&T, ss, OPT_STR, none);
        assert_eq!(parts(p), (1, Some(b)));
        assert_eq!(rc(from_slot(b)), 1);
        dec(p);
        let a = coll::list_pop(&T, ss, OPT_STR, none);
        assert_eq!(str_of(parts(a).1.unwrap()), b"a");
        dec(a);
        assert_eq!(coll::list_pop(&T, ss, OPT_STR, none), none);
        assert_eq!(coll::list_len(ss), 0);
        let fresh = coll::list_pop(&T, ss, OPT_STR, core::ptr::null_mut());
        assert_eq!(parts(fresh), (0, None));
        dec(fresh);
        let xs = ints(&[7]);
        let p = coll::list_pop(&T, xs, OPT_INT, core::ptr::null_mut());
        assert_eq!(parts(p), (1, Some(7)));
        dec(p);
        dec(xs);
    }
    dec(ss);
    unpin(none);
    assert_eq!(obj::live(), before);
}

/// `ok`, `err` and `ok_or` borrow their arguments and hold new references.
#[test]
fn result_ok_err_and_option_ok_or() {
    let _l = lock();
    let before = obj::live();
    let none = pinned_none(OPT_STR);
    let (x, e) = (s("x"), s("e"));
    let ok = obj::alloc(&T, RES_STR, 0);
    let err = obj::alloc(&T, RES_STR, 1);
    let some = obj::alloc(&T, OPT_STR, 1);
    // SAFETY: fresh objects with one slot each, filled with owned references; then live
    // values of the types each function takes.
    unsafe {
        obj::inc(from_slot(x));
        obj::inc(from_slot(x));
        obj::inc(from_slot(e));
        *obj::slot(&T, ok, 0) = x;
        *obj::slot(&T, some, 0) = x;
        *obj::slot(&T, err, 0) = e;
        assert_eq!(rc(from_slot(x)), 3);
        let o = coll::result_variant(&T, ok, 0, OPT_STR, none);
        assert_eq!((parts(o), rc(from_slot(x))), ((1, Some(x)), 4));
        dec(o);
        assert_eq!(coll::result_variant(&T, ok, 1, OPT_STR, none), none);
        assert_eq!(coll::result_variant(&T, err, 0, OPT_STR, none), none);
        let o = coll::result_variant(&T, err, 1, OPT_STR, none);
        assert_eq!((parts(o), rc(from_slot(e))), ((1, Some(e)), 3));
        dec(o);
        let r = coll::option_ok_or(&T, some, e, RES_STR);
        assert_eq!((parts(r), rc(from_slot(x)), rc(from_slot(e))), ((0, Some(x)), 4, 2));
        dec(r);
        let r = coll::option_ok_or(&T, none, e, RES_STR);
        assert_eq!((parts(r), rc(from_slot(e))), ((1, Some(e)), 3));
        dec(r);
    }
    for p in [ok, err, some, from_slot(x), from_slot(e)] {
        dec(p);
    }
    unpin(none);
    assert_eq!(obj::live(), before);
}

#[test]
fn map_get_returns_the_pinned_none() {
    for d in [MAP_STR_INT, HASH_STR_INT] {
        map_get_returns_the_pinned_none_of(d);
    }
}

fn map_get_returns_the_pinned_none_of(d: u32) {
    let _l = lock();
    let before = obj::live();
    let none = pinned_none(OPT_INT);
    let m = coll::map_new(&T, d);
    let k = s("k");
    // SAFETY: a `Map<Str, Int>` and a `Str` key.
    unsafe {
        assert_eq!(coll::map_get(&T, m, k, OPT_INT, none), none);
        coll::map_insert(&T, m, k, 5);
        let g = coll::map_get(&T, m, k, OPT_INT, none);
        assert_eq!(parts(g), (1, Some(5)));
        dec(g);
    }
    dec(from_slot(k));
    dec(m);
    unpin(none);
    assert_eq!(obj::live(), before);
}

/// A `HashMap` owns its `Str` keys (copies) and its values' references through growth,
/// compaction after removes, sorted buckets and copy-on-write; freeing it frees them all.
#[test]
fn hash_map_frees_what_it_holds() {
    let _l = lock();
    let before = obj::live();
    let n = if cfg!(miri) { 60 } else { 2_000 };
    // Keys built to share one hash, so their bucket is sorted, beside ordinary ones.
    let parts = crate::hmap::colliding_parts(2, 8);
    let mut keys: alloc::vec::Vec<alloc::vec::Vec<u8>> = parts[0]
        .iter()
        .flat_map(|a| parts[1].iter().map(move |b| [&a[..], &b[..]].concat()))
        .collect();
    keys.extend((0..n).map(|i| alloc::format!("key {i}").into_bytes()));
    let m = coll::map_new(&T, HASH_STR_LIST);
    let key = |k: &[u8]| to_slot(obj::new_bytes(desc::STR, k));
    // SAFETY: `m` is a unique `HashMap<Str, List<Int>>` (and `c`, a copy, once made unique);
    // each key is a live `Str` the test owns.
    unsafe {
        for (i, k) in keys.iter().enumerate() {
            let k = key(k);
            coll::map_insert(&T, m, k, to_slot(ints(&[i as i64])));
            dec(from_slot(k));
        }
        let c = callee(m);
        for k in keys.iter().step_by(2) {
            let k = key(k);
            coll::map_remove(&T, c, k);
            coll::map_insert(&T, c, k, to_slot(ints(&[])));
            coll::map_remove(&T, c, k);
            dec(from_slot(k));
        }
        assert_eq!(coll::map_len(&T, m), keys.len() as i64);
        assert_eq!(coll::map_len(&T, c), (keys.len() / 2) as i64);
        let ks = coll::map_keys(&T, c, LIST_STR);
        assert_eq!(str_at(ks, 0), keys[1]);
        assert_eq!(str_at(ks, 1), keys[3]);
        dec(ks);
        assert!(!eq(HASH_STR_LIST, m, c));
        dec(c);
    }
    dec(m);
    assert_eq!(obj::live(), before);
}

/// `m.insert(k, ..)` after `m.get(k)` reuses the entry `get` found (C8b6), and only then:
/// random gets, inserts, removes and copies on `Map<Str, Int>` and `HashMap<Str, Int>` against
/// a model, half of them on the key last got. 40 keys besides the odd ones (`""`, longer than a
/// word, equal 8-byte prefixes), so inserts and removes split and merge nodes (a `HashMap` grows
/// and compacts). Miri checks that every remembered entry used is still valid.
#[test]
fn map_insert_after_get_matches_a_model() {
    for d in [MAP_STR_INT, HASH_STR_INT] {
        map_insert_after_get_matches_a_model_of(d);
    }
}

fn map_insert_after_get_matches_a_model_of(d: u32) {
    let _l = lock();
    let before = obj::live();
    let mut names: alloc::vec::Vec<alloc::string::String> =
        ["a", "", "a key longer than a word", "a key longer than a wore", "b\0"]
            .map(alloc::string::String::from)
            .to_vec();
    names.extend((0..40).map(|i| alloc::format!("k{i}")));
    let n = if cfg!(miri) { 1_500 } else { 50_000 };
    let m = coll::map_new(&T, d);
    let mut model = alloc::collections::BTreeMap::new();
    let mut x = 3u64;
    let mut next = |k: usize| {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (x >> 33) as usize % k
    };
    let with = |k: &str, f: &dyn Fn(u64)| {
        let k = s(k);
        f(k);
        dec(from_slot(k));
    };
    // SAFETY: `m` is a map from `Str` to `Int`, unique when changed; each key is a live `Str`.
    unsafe {
        let get = |m: *mut Header, k: &str| {
            let k = s(k);
            let g = coll::map_get(&T, m, k, OPT_INT, core::ptr::null_mut());
            let v = (*field_u32(g) == 1).then(|| *obj::slot(&T, g, 0) as i64);
            dec(g);
            dec(from_slot(k));
            v
        };
        let mut got = names[0].as_str();
        for i in 0..n {
            let k = if next(2) == 0 {
                got
            } else {
                names[next(names.len())].as_str()
            };
            match next(8) {
                0..=2 => {
                    assert_eq!(get(m, k), model.get(k).copied(), "step {i}");
                    got = k;
                }
                3..=5 => {
                    with(k, &|q| coll::map_insert(&T, m, q, i as u64));
                    model.insert(k, i as i64);
                }
                6 => {
                    with(k, &|q| coll::map_remove(&T, m, q));
                    model.remove(k);
                }
                _ => {
                    // A copy remembers nothing, and the original keeps what it remembered.
                    obj::inc(m);
                    let c = obj::unique(&T, m);
                    assert_eq!(get(c, k), model.get(k).copied(), "step {i}");
                    with(k, &|q| coll::map_insert(&T, c, q, 0));
                    with(got, &|q| coll::map_insert(&T, m, q, i as u64));
                    model.insert(got, i as i64);
                    dec(c);
                }
            }
            assert_eq!(coll::map_len(&T, m), model.len() as i64);
        }
        for k in &names {
            assert_eq!(get(m, k), model.get(k.as_str()).copied());
        }
    }
    dec(m);
    assert_eq!(obj::live(), before);
}

/// An insert that reuses the entry `get` found releases the value it replaces.
#[test]
fn map_insert_after_get_releases_the_old_value() {
    for d in [MAP_INT_STR, HASH_INT_STR] {
        map_insert_after_get_releases_the_old_value_of(d);
    }
}

fn map_insert_after_get_releases_the_old_value_of(d: u32) {
    let _l = lock();
    let before = obj::live();
    let m = coll::map_new(&T, d);
    let (a, b) = (from_slot(s("a")), from_slot(s("b")));
    // SAFETY: `m` is a unique map from `Int` to `Str`.
    unsafe {
        obj::inc(a);
        coll::map_insert(&T, m, 7, to_slot(a));
        let g = coll::map_get(&T, m, 7, OPT_STR, core::ptr::null_mut());
        assert_eq!(rc(a), 3);
        dec(g);
        coll::map_insert(&T, m, 7, to_slot(b));
        assert_eq!((rc(a), rc(b)), (1, 1));
    }
    dec(a);
    dec(m);
    assert_eq!(obj::live(), before);
}

/// L11 (C14): `reverse`, `slice` (sharing its elements), `join`, `sort_by_key` and the
/// permutation `sort_by` ends with.
#[test]
fn reverse_slice_join_sort_by_key() {
    let _l = lock();
    let before = obj::live();
    // SAFETY: live, unique lists of the descriptors their names say.
    unsafe {
        let xs = ints(&[1, 2, 3, 4]);
        coll::list_reverse(xs);
        assert_eq!(contents(xs), [4, 3, 2, 1]);
        let part = coll::list_slice(&T, xs, 1, 3).unwrap();
        assert_eq!(contents(part), [3, 2]);
        assert!(coll::list_slice(&T, xs, 3, 2).is_none());
        assert!(coll::list_slice(&T, xs, 0, 5).is_none());
        assert!(coll::list_slice(&T, xs, -1, 1).is_none());
        let none = coll::list_slice(&T, xs, 4, 4).unwrap();
        assert_eq!(contents(none), []);
        let ss = strs(&["b", "a", "c"]);
        let mid = coll::list_slice(&T, ss, 1, 2).unwrap();
        let a = coll::list_get(&T, ss, 1).unwrap();
        assert_eq!(rc(from_slot(a)), 3);
        dec(from_slot(a));
        let sep = from_slot(s(", "));
        let joined = coll::list_join(ss, sep);
        assert_eq!(obj::bytes(joined), b"b, a, c");
        let empty = strs(&[]);
        let nothing = coll::list_join(empty, sep);
        assert_eq!(obj::bytes(nothing), b"");
        let keys = ints(&[2, 1, 2]);
        coll::list_sort_by_key(&T, ss, keys);
        assert_eq!(
            (0..3).map(|i| str_at(ss, i)).collect::<alloc::vec::Vec<_>>(),
            [b"a", b"b", b"c"]
        );
        let skeys = strs(&["z", "y", "y", "x"]);
        coll::list_sort_by_key(&T, xs, skeys);
        assert_eq!(contents(xs), [1, 3, 2, 4]);
        coll::list_permute(xs, &[3, 0, 2, 1]);
        assert_eq!(contents(xs), [4, 1, 2, 3]);
        for p in [xs, part, none, ss, mid, sep, joined, empty, nothing, keys, skeys] {
            dec(p);
        }
    }
    assert_eq!(obj::live(), before);
}
