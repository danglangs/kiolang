use crate::desc::{self, Desc, Name, Slice, Table, Variant};
use crate::obj::{self, from_slot, to_slot, Header, Key, Map, PINNED};
use alloc::vec;
use alloc::vec::Vec;
use std::sync::{Mutex, MutexGuard};

// struct Inner { name: Str, n: Int }   struct Outer { items: List<Inner> }
// enum Chain { End, Link(Chain) }      Map<Str, Inner>
const LIST_INNER: u32 = 5;
const INNER: u32 = 6;
const OUTER: u32 = 7;
const CHAIN: u32 = 8;
const MAP: u32 = 9;

const DESCS: [Desc; 10] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
    Desc::List { elem: INNER },
    Desc::Record {
        name: Name { off: 0, len: 5 },
        slots: Slice { start: 0, len: 2 },
    },
    Desc::Record {
        name: Name { off: 5, len: 5 },
        slots: Slice { start: 2, len: 1 },
    },
    Desc::Enum {
        name: Name { off: 10, len: 5 },
        variants: Slice { start: 0, len: 2 },
    },
    Desc::Map {
        key: desc::STR,
        value: INNER,
    },
];
const SLOTS: [u32; 4] = [desc::STR, desc::INT, LIST_INNER, CHAIN];
const VARIANTS: [Variant; 2] = [
    Variant {
        name: Name { off: 15, len: 3 },
        slots: Slice { start: 0, len: 0 },
    },
    Variant {
        name: Name { off: 18, len: 4 },
        slots: Slice { start: 3, len: 1 },
    },
];
const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    variants: &VARIANTS,
    names: b"InnerOuterChainEndLink",
};

/// The live count is global, so tests that check it run one at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn inner(name: &str, n: i64) -> *mut Header {
    let p = obj::alloc(&T, INNER, 0);
    // SAFETY: `p` is a fresh `Inner`, with two slots.
    unsafe {
        *obj::slot(&T, p, 0) = to_slot(obj::new_bytes(desc::STR, name.as_bytes()));
        *obj::slot(&T, p, 1) = n as u64;
    }
    p
}

fn rc(p: *mut Header) -> u32 {
    // SAFETY: tests read only live objects.
    unsafe { (*p).rc }
}

#[test]
fn struct_of_list_of_structs_frees_everything() {
    let _l = lock();
    let before = obj::live();
    let list = obj::new_list(LIST_INNER, (0..3).map(|i| to_slot(inner("x", i))).collect());
    let outer = obj::alloc(&T, OUTER, 0);
    // SAFETY: `outer` has one slot; it takes `list`'s reference.
    unsafe { *obj::slot(&T, outer, 0) = to_slot(list) };
    assert_eq!(obj::live() - before, 1 + 1 + 3 * 2);
    // SAFETY: `outer` is live, one reference.
    unsafe { obj::dec(&T, outer) };
    assert_eq!(obj::live(), before);
}

#[test]
fn map_values_are_freed() {
    let _l = lock();
    let before = obj::live();
    let mut m = Map::new();
    m.insert(Key::Str(b"a".as_slice().into()), to_slot(inner("a", 1)));
    m.insert(Key::Str(b"b".as_slice().into()), to_slot(inner("b", 2)));
    let p = obj::new_map(MAP, m);
    // SAFETY: `p` is live.
    unsafe { obj::dec(&T, p) };
    assert_eq!(obj::live(), before);
}

#[test]
fn deep_nest_frees_without_recursion() {
    let _l = lock();
    let depth = if cfg!(miri) { 1_000 } else { 1_000_000 };
    let before = obj::live();
    let mut p = obj::alloc(&T, CHAIN, 0);
    for _ in 0..depth {
        let q = obj::alloc(&T, CHAIN, 1);
        // SAFETY: `q` is a fresh `Link`, one slot; it takes `p`'s reference.
        unsafe { *obj::slot(&T, q, 0) = to_slot(p) };
        p = q;
    }
    assert_eq!(obj::live() - before, depth + 1);
    // A recursive free would overflow the test thread's 2 MB stack at this depth.
    // SAFETY: `p` is live.
    unsafe { obj::dec(&T, p) };
    assert_eq!(obj::live(), before);
}

#[test]
fn unique_copies_only_when_shared() {
    let _l = lock();
    let before = obj::live();
    let a = inner("a", 1);
    let b = inner("b", 2);
    let list = obj::new_list(LIST_INNER, vec![to_slot(a), to_slot(b)]);
    // SAFETY: every pointer is live; each step's reference counts are asserted.
    unsafe {
        assert_eq!(obj::unique(&T, list), list, "sole reference: no copy");

        obj::inc(list);
        let copy = obj::unique(&T, list);
        assert_ne!(copy, list);
        assert_eq!((rc(list), rc(copy)), (1, 1));
        let elems: Vec<_> = (*obj::field::<obj::List>(copy, 8))
            .iter()
            .map(|&v| from_slot(v))
            .collect();
        assert_eq!(elems, [a, b], "elements shared, not copied");
        assert_eq!((rc(a), rc(b)), (2, 2));

        // A struct copies its slots: the copy shares `a`'s name.
        obj::inc(a);
        let a2 = obj::unique(&T, a);
        assert_ne!(a2, a);
        let name = from_slot(*obj::slot(&T, a, 0));
        assert_eq!(from_slot(*obj::slot(&T, a2, 0)), name);
        assert_eq!((rc(name), *obj::slot(&T, a2, 1)), (2, 1));
        assert_eq!(obj::bytes(name), b"a");

        obj::dec(&T, a2);
        obj::dec(&T, copy);
        obj::dec(&T, list);
    }
    assert_eq!(obj::live(), before);
}

/// A string literal as codegen emits it: a data segment, pinned.
#[repr(C, align(8))]
struct Static {
    header: Header,
    len: u32,
    cap: u32,
    bytes: [u8; 2],
}
static HI: Static = Static {
    header: Header {
        rc: PINNED,
        desc: desc::STR,
    },
    len: 2,
    cap: 2,
    bytes: *b"hi",
};

#[test]
fn pinned_survives_any_decs() {
    let _l = lock();
    let before = obj::live();
    let p = (&raw const HI).cast::<Header>().cast_mut();
    // SAFETY: `p` is pinned, so `inc` and `dec` never write it, and `unique` only reads it.
    unsafe {
        for _ in 0..1_000 {
            obj::dec(&T, p);
        }
        obj::inc(p);
        assert_eq!(rc(p), PINNED);
        assert_eq!(obj::bytes(p), b"hi");

        let q = obj::unique(&T, p);
        assert_ne!(q, p, "a static is always copied");
        assert_eq!((rc(q), obj::bytes(q)), (1, b"hi".as_slice()));
        obj::dec(&T, q);
    }
    assert_eq!(obj::live(), before);
}

#[test]
fn zero_is_no_object() {
    // SAFETY: 0 is "no object"; `inc` and `dec` ignore it.
    unsafe {
        obj::inc(core::ptr::null_mut());
        obj::dec(&T, core::ptr::null_mut());
    }
}

#[test]
#[cfg(not(miri))]
#[should_panic(expected = "use after free")]
fn use_after_free_is_caught() {
    let _l = lock();
    let p = obj::new_bytes(desc::STR, b"x");
    // SAFETY: deliberately not: the second `dec` reads poisoned memory, which test builds keep
    // allocated so the read is defined.
    unsafe {
        obj::dec(&T, p);
        obj::dec(&T, p);
    }
}
