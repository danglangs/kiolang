use crate::coll::{self, Error};
use crate::desc::{self, Desc, Name, Slice, Table, Variant};
use crate::obj::{self, from_slot, to_slot, Header};
use alloc::vec;

const LIST_INT: u32 = 5;
const LIST_STR: u32 = 6;
const MAP_STR_INT: u32 = 7;
const MAP_INT_STR: u32 = 8;
const OPT_INT: u32 = 9;
const OPT_STR: u32 = 10;
const LIST_LIST: u32 = 11;
const CHAIN: u32 = 12;
const PAIR: u32 = 13;
const MAP_STR_LIST: u32 = 14;
const DEEP: u32 = 15; // List<DEEP>

const NAME: Name = Name { off: 0, len: 0 };
const DESCS: [Desc; 16] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
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
];
const SLOTS: [u32; 6] = [desc::INT, desc::STR, CHAIN, desc::STR, LIST_INT, LIST_INT];
const FIELDS: [Name; 6] = [Name { off: 0, len: 0 }; 6];
const fn variant(start: u32, len: u32) -> Variant {
    Variant {
        name: NAME,
        slots: Slice { start, len },
    }
}
const VARIANTS: [Variant; 6] = [
    variant(0, 0),
    variant(0, 1),
    variant(0, 0),
    variant(1, 1),
    variant(0, 0),
    variant(2, 1),
];
const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    fields: &FIELDS,
    variants: &VARIANTS,
    names: b"",
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
    }
    assert_eq!(contents(p), [9, 2, 3]);
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

#[test]
fn map_keys_are_in_order() {
    let _l = lock();
    let before = obj::live();
    let m = coll::map_new(MAP_INT_STR);
    // SAFETY: `m` is a unique `Map<Int, Str>`.
    unsafe {
        for (k, v) in [(3i64, "x"), (-1, "y"), (2, "z")] {
            coll::map_insert(&T, m, k as u64, s(v));
        }
        let ks = coll::map_keys(&T, m, LIST_INT);
        assert_eq!(contents(ks), [-1, 2, 3]);
        dec(ks);
    }
    dec(m);

    let m = coll::map_new(MAP_STR_INT);
    // SAFETY: `m` is a unique `Map<Str, Int>`; each key is read, so the test still owns it.
    unsafe {
        for (k, v) in [("b", 1), ("a", 2), ("c", 3), ("", 4), ("ab", 5)] {
            let k = s(k);
            coll::map_insert(&T, m, k, v);
            dec(from_slot(k));
        }
        let ks = coll::map_keys(&T, m, LIST_STR);
        let got: alloc::vec::Vec<_> = (0..5).map(|i| str_at(ks, i)).collect();
        assert_eq!(got, [&b""[..], b"a", b"ab", b"b", b"c"]);
        dec(ks);
    }
    dec(m);
    assert_eq!(obj::live(), before);
}

#[test]
fn map_get_insert_remove() {
    let _l = lock();
    let before = obj::live();
    let m = coll::map_new(MAP_INT_STR);
    // SAFETY: `m` is a unique `Map<Int, Str>`.
    unsafe {
        let none = coll::map_get(&T, m, 1, OPT_STR);
        assert_eq!(*field_u32(none), 0);
        dec(none);

        coll::map_insert(&T, m, 1, s("old"));
        let v = s("new");
        coll::map_insert(&T, m, 1, s("ignored"));
        coll::map_insert(&T, m, 1, v);
        coll::map_insert(&T, m, 2, s("two"));
        coll::map_remove(&T, m, 2);
        coll::map_remove(&T, m, 9);
        assert_eq!(coll::map_len(m), 1);

        let some = coll::map_get(&T, m, 1, OPT_STR);
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
    let _l = lock();
    let before = obj::live();
    let m = coll::map_new(MAP_STR_INT);
    // SAFETY: `m` is a unique `Map<Str, Int>`.
    unsafe {
        let k = s("a");
        coll::map_insert(&T, m, k, 10);
        coll::map_insert(&T, m, k, 20);
        let b = s("b");
        coll::map_insert(&T, m, b, 5);
        dec(from_slot(b));
        coll::map_remove(&T, m, k);
        let g = coll::map_get(&T, m, k, OPT_INT);
        assert_eq!(*field_u32(g), 0);
        dec(g);
        dec(from_slot(k));
        let k = s("b");
        let g = coll::map_get(&T, m, k, OPT_INT);
        assert_eq!(*obj::slot(&T, g, 0), 5);
        dec(g);
        dec(from_slot(k));
    }
    dec(m);
    assert_eq!(obj::live(), before);
}

#[test]
fn map_copy_on_write_through_a_callee() {
    let _l = lock();
    let before = obj::live();
    let l = ints(&[1]);
    let m = coll::map_new(MAP_STR_LIST);
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
    assert_eq!(unsafe { coll::map_len(m) }, 1);
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

    let mk = |ks: &[(&str, i64)]| {
        let m = coll::map_new(MAP_STR_INT);
        for &(k, v) in ks {
            let k = s(k);
            // SAFETY: `m` is a unique `Map<Str, Int>`.
            unsafe { coll::map_insert(&T, m, k, v as u64) };
            dec(from_slot(k));
        }
        m
    };
    let (m1, m2) = (mk(&[("a", 1), ("b", 2)]), mk(&[("b", 2), ("a", 1)]));
    assert!(eq(MAP_STR_INT, m1, m2));
    let others = [mk(&[("a", 1), ("b", 3)]), mk(&[("a", 1), ("c", 2)]), mk(&[("a", 1)])];
    for &o in &others {
        assert!(!eq(MAP_STR_INT, m1, o));
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

    for p in [
        a, b, c, d, x, y, m1, m2, p, q, r, r2, s4, s4b, s5, none, none2, l1, l2, l3,
    ] {
        dec(p);
    }
    others.into_iter().for_each(dec);
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
