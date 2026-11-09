//! Free plans (C8b7): what `obj::plans` works out, and freeing through a table that has them,
//! as a run's table does (native test tables have none).

use crate::desc::{self, Desc, Kind, Name, Plan, Slice, Table, Variant};
use crate::obj::{self, to_slot, Header, Key, Map};
use crate::tests::lock;
use alloc::vec::Vec;

// struct Pair { s: Str, n: Int }    struct Nums { a: Int, b: Float }    struct Wide { 70 slots }
// enum Shape { Dot, Box(Int, Int), Named(Str, Shape) }
// List<Int>  List<Str>  List<Pair>  Map<Str, Pair>  HashMap<Str, Int>  HashMap<Int, Str>
// a closure capturing (Int, Str)
const PAIR: u32 = 6;
const NUMS: u32 = 7;
const WIDE: u32 = 8;
const SHAPE: u32 = 9;
const LIST_INT: u32 = 10;
const LIST_STR: u32 = 11;
const LIST_PAIR: u32 = 12;
const MAP: u32 = 13;
const HMAP_INT: u32 = 14;
const HMAP_STR: u32 = 15;
const CLOSURE: u32 = 16;
const N: u32 = 17;

const NO: Name = Name { off: 0, len: 0 };

const DESCS: [Desc; N as usize] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
    Desc::Float,
    Desc::Record {
        name: NO,
        slots: Slice { start: 0, len: 2 },
    },
    Desc::Record {
        name: NO,
        slots: Slice { start: 2, len: 2 },
    },
    Desc::Record {
        name: NO,
        slots: Slice { start: 4, len: 70 },
    },
    Desc::Enum {
        name: NO,
        variants: Slice { start: 0, len: 3 },
    },
    Desc::List { elem: desc::INT },
    Desc::List { elem: desc::STR },
    Desc::List { elem: PAIR },
    Desc::Map {
        key: desc::STR,
        value: PAIR,
    },
    Desc::HashMap {
        key: desc::STR,
        value: desc::INT,
    },
    Desc::HashMap {
        key: desc::INT,
        value: desc::STR,
    },
    Desc::Fn {
        name: NO,
        slots: Slice { start: 74, len: 2 },
    },
];

/// Slots: Pair, Nums, Wide (Str at 0 and 69, Int between), Box, Named, the closure's.
const SLOTS: [u32; 80] = {
    let mut s = [desc::INT; 80];
    s[0] = desc::STR;
    s[3] = desc::FLOAT;
    s[4] = desc::STR;
    s[4 + 69] = desc::STR;
    s[75] = desc::STR;
    s[78] = desc::STR;
    s[79] = SHAPE;
    s
};
const FIELDS: [Name; 80] = [NO; 80];
const VARIANTS: [Variant; 3] = [
    Variant {
        name: NO,
        slots: Slice { start: 0, len: 0 },
    },
    Variant {
        name: NO,
        slots: Slice { start: 76, len: 2 },
    },
    Variant {
        name: NO,
        slots: Slice { start: 78, len: 2 },
    },
];
const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    fields: &FIELDS,
    variants: &VARIANTS,
    names: b"",
    plans: &[],
};

fn plan(mask: u64, size: u32, kind: Kind) -> Plan {
    Plan { mask, size, kind }
}

#[test]
fn plans_are_worked_out_from_the_table() {
    let p = obj::plans(&T);
    let other = |kind| plan(0, 0, kind);
    let want = [
        other(Kind::Scalar),
        other(Kind::Scalar),
        other(Kind::Scalar),
        other(Kind::Bytes),
        other(Kind::Bytes),
        other(Kind::Scalar),
        plan(0b1, 24, Kind::Record),
        plan(0, 24, Kind::Record),
        plan(0, 8 + 8 * 70, Kind::Wide),
        plan(3, N, Kind::Enum),
        other(Kind::List),
        other(Kind::ListOfBytes),
        other(Kind::ListOfPtrs),
        other(Kind::MapOfPtrs),
        other(Kind::HashMap),
        other(Kind::HashMapOfPtrs),
        plan(0b10, 32, Kind::Variant),
        // Shape's variants: Dot, Box(Int, Int), Named(Str, Shape).
        plan(0, 16, Kind::Variant),
        plan(0, 32, Kind::Variant),
        plan(0b11, 32, Kind::Variant),
    ];
    assert_eq!(p, want);
}

/// A 64-slot struct still has a mask; the 65th slot makes it `Wide`.
#[test]
fn the_mask_holds_64_slots() {
    let descs = [
        Desc::Str,
        Desc::Record {
            name: NO,
            slots: Slice { start: 0, len: 64 },
        },
        Desc::Record {
            name: NO,
            slots: Slice { start: 0, len: 65 },
        },
    ];
    let slots = [0u32; 65];
    let t = Table {
        descs: &descs,
        slots: &slots,
        fields: &[NO; 65],
        variants: &[],
        names: b"",
        plans: &[],
    };
    assert_eq!(
        obj::plans(&t)[1..],
        [
            plan(u64::MAX, 8 + 8 * 64, Kind::Record),
            plan(0, 8 + 8 * 65, Kind::Wide)
        ]
    );
}

fn str(s: &str) -> *mut Header {
    obj::new_bytes(desc::STR, s.as_bytes())
}

fn rc(p: *mut Header) -> u32 {
    // SAFETY: tests read only live objects.
    unsafe { (*p).rc }
}

/// Every shape freed through a planned table, as `dec` and `free` see it in a run: each frees
/// what only it held and leaves shared children with one reference less.
#[test]
fn planned_frees_free_every_shape() {
    let _l = lock();
    let plans = obj::plans(&T);
    let t = Table { plans: &plans, ..T };
    let before = obj::live();
    let shared = str("shared");
    // SAFETY: every object is live and laid out as its descriptor says; each `dec` drops a
    // reference this test owns.
    unsafe {
        let pair = |s: *mut Header, n: i64| {
            let p = obj::alloc(&t, PAIR, 0);
            *obj::slot(&t, p, 0) = to_slot(s);
            *obj::slot(&t, p, 1) = n as u64;
            p
        };
        let mut roots = Vec::new();

        // Records: a pointer-free one (`dec`'s fast path), one with a shared and an owned child.
        let nums = obj::alloc(&t, NUMS, 0);
        *obj::slot(&t, nums, 0) = 5;
        roots.push(nums);
        obj::inc(shared);
        roots.push(pair(shared, 1));
        roots.push(pair(str("own"), 2));

        // Wide: pointers in slots 0 and 69 only (the Int slots hold numbers that are not
        // addresses).
        let wide = obj::alloc(&t, WIDE, 0);
        for i in 1..69 {
            *obj::slot(&t, wide, i) = 0xdead_beef;
        }
        *obj::slot(&t, wide, 0) = to_slot(str("first"));
        obj::inc(shared);
        *obj::slot(&t, wide, 69) = to_slot(shared);
        roots.push(wide);

        // Enum: Named("a", Named("b", Box(1, 2))), and a Dot (pointer-free, the fast path).
        let mut shape = obj::alloc(&t, SHAPE, 1);
        *obj::slot(&t, shape, 0) = 1;
        *obj::slot(&t, shape, 1) = 2;
        for name in ["b", "a"] {
            let named = obj::alloc(&t, SHAPE, 2);
            *obj::slot(&t, named, 0) = to_slot(str(name));
            *obj::slot(&t, named, 1) = to_slot(shape);
            shape = named;
        }
        roots.push(shape);
        roots.push(obj::alloc(&t, SHAPE, 0));

        // Lists of each element kind, one `Str` element shared.
        roots.push(obj::new_list(LIST_INT, (0..5).collect()));
        obj::inc(shared);
        roots.push(obj::new_list(LIST_STR, alloc::vec![to_slot(str("x")), to_slot(shared)]));
        roots.push(obj::new_list(LIST_PAIR, alloc::vec![to_slot(pair(str("y"), 3)), 0]));

        // Maps.
        let mut m = Map::new();
        m.insert(
            Key::Str(b"k".as_slice().into()),
            core::cell::Cell::new(to_slot(pair(str("v"), 4))),
        );
        roots.push(obj::new_map(MAP, m));
        let mut h = obj::HashMap::new();
        h.insert(Key::Str(b"k".as_slice().into()), 7);
        roots.push(obj::new_hash_map(HMAP_INT, h));
        let mut h = obj::HashMap::new();
        h.insert(Key::Int(1), to_slot(str("v")));
        obj::inc(shared);
        h.insert(Key::Int(2), to_slot(shared));
        roots.push(obj::new_hash_map(HMAP_STR, h));

        // A closure capturing (Int, Str), and a bare `Str` (the fast path).
        let c = obj::alloc(&t, CLOSURE, 0);
        *obj::slot(&t, c, 0) = 9;
        *obj::slot(&t, c, 1) = to_slot(str("cap"));
        roots.push(c);
        roots.push(str("alone"));

        assert_eq!(rc(shared), 5);
        for p in roots {
            obj::dec(&t, p);
        }
        assert_eq!(rc(shared), 1, "each holder dropped its reference once");
        obj::dec(&t, shared);
    }
    assert_eq!(obj::live(), before);
}

/// A variant number the enum does not have is refused, as `Table::variant` refuses it. (Not
/// under Miri: the object is left allocated.)
#[test]
#[cfg(not(miri))]
#[should_panic]
fn a_bad_variant_is_refused() {
    let _l = lock();
    let plans = obj::plans(&T);
    let t = Table { plans: &plans, ..T };
    let p = obj::alloc(&t, SHAPE, 0);
    // SAFETY: deliberately not: variant 3 does not exist; the plan lookup panics before any
    // slot is read.
    unsafe {
        *obj::field::<u32>(p, 8) = 3;
        obj::dec(&t, p);
    }
}
