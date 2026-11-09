//! Heap objects and their reference counts.
//!
//! Layout, in bytes; every object is 8-aligned and a slot is 8 bytes:
//!
//! ```text
//! Record      [rc u32][desc u32][slot 0]..[slot n-1]
//! Enum        [rc][desc][variant u32][0 u32][slot 0]..     the slots of that variant
//! Str, Bytes  [rc][desc][len u32][cap u32][bytes; cap]
//! List        [rc][desc][List]
//! Map         [rc][desc][Map]
//! ```
//!
//! A slot or element holds an `Int` as its bits, a `Bool` as 0 or 1, `()` as 0, and a pointer
//! as its address, zero-extended.
//!
//! Invariants, which every caller keeps:
//! - A pointer is 0 (no object, such as a cleared local) or the address of a live object made
//!   here or a pinned static.
//! - `rc` is the number of references. `PINNED` marks a static: never counted, never freed. A
//!   count cannot reach `PINNED` on wasm32: that takes 2^32 references in 4 GiB.
//! - `desc` indexes the `Table` every function is given and matches the object's layout.
//! - Every pointer slot and element holds 0 or an owned reference.
//!
//! Each `unsafe fn` here has the one contract: its pointers keep these invariants for `t`.

use crate::desc::{self, Desc, Table};
use alloc::alloc::{alloc_zeroed, handle_alloc_error, Layout};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::mem::{align_of, size_of};
use core::ptr;

pub const PINNED: u32 = u32::MAX;

#[repr(C)]
pub struct Header {
    pub rc: u32,
    pub desc: u32,
}

/// A `Map` key, as the interpreter's. `Str` keys are copied out of their objects, so they
/// order by bytes and are not counted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Int(i64),
    Str(Box<[u8]>),
}

pub type List = Vec<u64>;
pub type Map = BTreeMap<Key, u64>;

pub(crate) const HEADER: usize = 8;
const ENUM_SLOTS: usize = 16;
pub(crate) const BYTES_DATA: usize = 16;
const _: () = assert!(HEADER.is_multiple_of(align_of::<List>()) && HEADER.is_multiple_of(align_of::<Map>()));

/// A slot's value for pointer `p`.
pub fn to_slot(p: *mut Header) -> u64 {
    p.expose_provenance() as u64
}

/// The pointer a slot holds.
pub fn from_slot(v: u64) -> *mut Header {
    ptr::with_exposed_provenance_mut(v as usize)
}

/// The field at byte `off` of `p`.
///
/// # Safety
/// `off` is inside `p`'s object and the field there is a `T`.
pub unsafe fn field<T>(p: *mut Header, off: usize) -> *mut T {
    // SAFETY: the caller's guarantee keeps the result inside the object.
    unsafe { p.cast::<u8>().add(off).cast() }
}

/// Slot `i` of a struct or of an enum variant.
///
/// # Safety
/// `p` is a struct or enum with more than `i` slots.
pub unsafe fn slot(t: &Table, p: *mut Header, i: usize) -> *mut u64 {
    // SAFETY: the caller's guarantee; `record` reads only the header and variant.
    unsafe {
        let (off, slots) = record(t, p);
        assert!(i < slots.len());
        field(p, off + 8 * i)
    }
}

fn layout(size: usize) -> Layout {
    Layout::from_size_align(size, 8).unwrap()
}

/// A fresh object of `size` zeroed bytes, `rc` 1.
fn new(size: usize, desc: u32) -> *mut Header {
    let layout = layout(size);
    // SAFETY: `size` is at least HEADER, so not zero.
    let p = unsafe { alloc_zeroed(layout) }.cast::<Header>();
    if p.is_null() {
        handle_alloc_error(layout);
    }
    // SAFETY: a fresh allocation of at least HEADER bytes, 8-aligned.
    unsafe { p.write(Header { rc: 1, desc }) };
    live_add(1);
    p
}

/// A struct, or variant `variant` of an enum, every slot 0. Fill the slots with `slot`.
pub fn alloc(t: &Table, desc: u32, variant: u32) -> *mut Header {
    let (off, n, is_enum) = match t.desc(desc) {
        Desc::Record { slots, .. } => (HEADER, slots.len, false),
        Desc::Enum { variants, .. } => (ENUM_SLOTS, t.variant(variants, variant).slots.len, true),
        d => panic!("rt_alloc of {d:?}"),
    };
    let p = new(off + 8 * n as usize, desc);
    if is_enum {
        // SAFETY: an enum object has its variant at HEADER.
        unsafe { field::<u32>(p, HEADER).write(variant) };
    }
    p
}

/// An `Option` of descriptor `opt` (variant 0 `None`, 1 `Some` with one slot): `Some(v)`
/// taking `v`'s reference, or `none`, a pinned `None` of `opt` (no allocation), or a new `None`
/// if `none` is null.
///
/// # Safety
/// `none` is null or a `None` of `opt`; `v` is a slot value of `Some`'s payload type.
pub unsafe fn option(t: &Table, opt: u32, none: *mut Header, v: Option<u64>) -> *mut Header {
    match v {
        Some(v) => {
            let p = alloc(t, opt, 1);
            // SAFETY: `p` is a fresh `Some`, with one slot.
            unsafe { *slot(t, p, 0) = v };
            p
        }
        None if none.is_null() => alloc(t, opt, 0),
        None => {
            // SAFETY: the caller's guarantee.
            unsafe { inc(none) };
            none
        }
    }
}

/// A `Str` or `Bytes` (by `desc`) holding a copy of `b`.
pub fn new_bytes(desc: u32, b: &[u8]) -> *mut Header {
    new_bytes_cap(desc, b, b.len())
}

/// Like `new_bytes`, with room for `cap` bytes (at least `b.len()`) before it must grow.
pub fn new_bytes_cap(desc: u32, b: &[u8], cap: usize) -> *mut Header {
    assert!(desc == desc::STR || desc == desc::BYTES);
    assert!(cap >= b.len());
    let len = u32::try_from(b.len()).expect("over 4 GiB");
    let cap = u32::try_from(cap).expect("over 4 GiB");
    let p = new(BYTES_DATA + cap as usize, desc);
    // SAFETY: `p` has `len` and `cap` at 8 and 12, then room for `cap` >= `len` bytes.
    unsafe {
        field::<u32>(p, 8).write(len);
        field::<u32>(p, 12).write(cap);
        ptr::copy_nonoverlapping(b.as_ptr(), field(p, BYTES_DATA), b.len());
    }
    p
}

/// Room for the `len` bytes of a `Str` or `Bytes` that `adopt` turns into one: the address
/// of its data, after space for the header. For `cabi_realloc` (runtime/src/caps.rs), so a
/// string the host returns lands in its object with no copy.
pub fn bytes_room(len: usize) -> *mut u8 {
    let layout = layout(BYTES_DATA + len);
    // SAFETY: the size is at least BYTES_DATA, so not zero.
    let p = unsafe { alloc::alloc::alloc(layout) };
    if p.is_null() {
        handle_alloc_error(layout);
    }
    // SAFETY: inside the allocation.
    unsafe { p.add(BYTES_DATA) }
}

/// The `Str` or `Bytes` (by `desc`) holding the `len` bytes at `data`, count 1.
///
/// # Safety
/// `data` came from `bytes_room(len)`, its bytes are initialized (UTF-8 for a `Str`), and it
/// is adopted once.
pub unsafe fn adopt(desc: u32, data: *mut u8, len: usize) -> *mut Header {
    assert!(desc == desc::STR || desc == desc::BYTES);
    let n = u32::try_from(len).expect("over 4 GiB");
    // SAFETY: `bytes_room` left BYTES_DATA bytes before `data`, 8-aligned.
    unsafe {
        let p = data.sub(BYTES_DATA).cast::<Header>();
        p.write(Header { rc: 1, desc });
        field::<u32>(p, 8).write(n);
        field::<u32>(p, 12).write(n);
        live_add(1);
        p
    }
}

/// A `Str`'s or `Bytes`' contents.
///
/// # Safety
/// `p` is a `Str` or `Bytes`, and outlives `'a` unchanged.
pub unsafe fn bytes<'a>(p: *mut Header) -> &'a [u8] {
    // SAFETY: the caller's guarantee and the layout.
    unsafe { core::slice::from_raw_parts(field(p, BYTES_DATA), *field::<u32>(p, 8) as usize) }
}

/// A `List` with descriptor `desc`, owning its elements' references.
pub fn new_list(desc: u32, v: List) -> *mut Header {
    let p = new(HEADER + size_of::<List>(), desc);
    // SAFETY: room for a `List` at HEADER, aligned (asserted above).
    unsafe { field::<List>(p, HEADER).write(v) };
    p
}

/// A `Map` with descriptor `desc`, owning its values' references.
pub fn new_map(desc: u32, m: Map) -> *mut Header {
    let p = new(HEADER + size_of::<Map>(), desc);
    // SAFETY: room for a `Map` at HEADER, aligned (asserted above).
    unsafe { field::<Map>(p, HEADER).write(m) };
    p
}

/// Adds a reference.
///
/// # Safety
/// The module's contract.
pub unsafe fn inc(p: *mut Header) {
    if p.is_null() {
        return;
    }
    // SAFETY: a live object, so its header is readable and, unless pinned, writable.
    unsafe {
        let rc = (*p).rc;
        check_live(rc);
        if rc != PINNED {
            (*p).rc = rc + 1;
        }
    }
}

/// Drops a reference, freeing what only it kept alive.
///
/// # Safety
/// The module's contract.
pub unsafe fn dec(t: &Table, p: *mut Header) {
    // SAFETY: the caller's guarantee.
    unsafe {
        if release(p) {
            free(t, p);
        }
    }
}

/// Drops a reference without freeing; true if it was the last, so the caller must `free`.
///
/// # Safety
/// The module's contract.
unsafe fn release(p: *mut Header) -> bool {
    if p.is_null() {
        return false;
    }
    // SAFETY: as in `inc`.
    unsafe {
        let rc = (*p).rc;
        check_live(rc);
        if rc == PINNED {
            return false;
        }
        (*p).rc = rc - 1;
        rc == 1
    }
}

/// Frees `p`, whose count `release` took to 0, and every object only it kept alive. A
/// worklist, not recursion, so a deep value costs heap, not stack.
///
/// # Safety
/// The module's contract, and `p` has no references left.
unsafe fn free(t: &Table, mut p: *mut Header) {
    let mut work = Vec::new(); // allocates only if a child dies too
    loop {
        // SAFETY: `p` is unreferenced, so its children and memory are ours to release.
        unsafe {
            let size = size(t, p);
            ptrs(t, p, |c| {
                if release(c) {
                    work.push(c)
                }
            });
            match t.desc((*p).desc) {
                Desc::List { .. } => ptr::drop_in_place(field::<List>(p, HEADER)),
                Desc::Map { .. } => ptr::drop_in_place(field::<Map>(p, HEADER)),
                _ => {}
            }
            free_memory(p, size);
        }
        match work.pop() {
            Some(c) => p = c,
            None => return,
        }
    }
}

/// `p` if this is its only reference, else a copy with count 1 sharing (inc'ing) its
/// children. Takes `p`'s reference either way. A pinned static is always copied.
///
/// # Safety
/// The module's contract, and `p` is not 0.
pub unsafe fn unique(t: &Table, p: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee; `p` is shared, so the `dec` frees nothing.
    unsafe {
        let rc = (*p).rc;
        check_live(rc);
        if rc == 1 {
            return p;
        }
        let q = copy(t, p);
        dec(t, p);
        q
    }
}

/// A shallow copy of `p` with count 1; each child gains a reference.
unsafe fn copy(t: &Table, p: *mut Header) -> *mut Header {
    // SAFETY: `p` is live; `q` is fresh with `p`'s size and descriptor.
    unsafe {
        let size = size(t, p);
        let desc = (*p).desc;
        let q = new(size, desc);
        match t.desc(desc) {
            Desc::List { .. } => field::<List>(q, HEADER).write((*field::<List>(p, HEADER)).clone()),
            Desc::Map { .. } => field::<Map>(q, HEADER).write((*field::<Map>(p, HEADER)).clone()),
            _ => ptr::copy_nonoverlapping(field::<u8>(p, HEADER), field(q, HEADER), size - HEADER),
        }
        ptrs(t, q, |c| inc(c));
        q
    }
}

/// Calls `f` with each pointer `p` holds: slots, elements and map values.
unsafe fn ptrs(t: &Table, p: *mut Header, mut f: impl FnMut(*mut Header)) {
    // SAFETY: `p` is live and laid out as its descriptor says.
    unsafe {
        match t.desc((*p).desc) {
            Desc::List { elem } if t.is_ptr(elem) => (*field::<List>(p, HEADER)).iter().for_each(|&v| f(from_slot(v))),
            Desc::Map { value, .. } if t.is_ptr(value) => {
                (*field::<Map>(p, HEADER)).values().for_each(|&v| f(from_slot(v)))
            }
            Desc::Record { .. } | Desc::Enum { .. } => {
                let (off, slots) = record(t, p);
                for (i, &d) in slots.iter().enumerate() {
                    if t.is_ptr(d) {
                        f(from_slot(*field::<u64>(p, off + 8 * i)))
                    }
                }
            }
            _ => {}
        }
    }
}

/// A struct's or enum variant's first slot offset and slot descriptors.
pub(crate) unsafe fn record<'t>(t: &Table<'t>, p: *mut Header) -> (usize, &'t [u32]) {
    // SAFETY: `p` is live; an enum has its variant at HEADER.
    unsafe {
        match t.desc((*p).desc) {
            Desc::Record { slots, .. } => (HEADER, t.slots(slots)),
            Desc::Enum { variants, .. } => (ENUM_SLOTS, t.slots(t.variant(variants, *field(p, HEADER)).slots)),
            d => panic!("{d:?} has no slots"),
        }
    }
}

/// The object's size in bytes, as allocated.
unsafe fn size(t: &Table, p: *mut Header) -> usize {
    // SAFETY: `p` is live and laid out as its descriptor says.
    unsafe {
        match t.desc((*p).desc) {
            Desc::Str | Desc::Bytes => BYTES_DATA + *field::<u32>(p, 12) as usize,
            Desc::List { .. } => HEADER + size_of::<List>(),
            Desc::Map { .. } => HEADER + size_of::<Map>(),
            Desc::Record { .. } | Desc::Enum { .. } => {
                let (off, slots) = record(t, p);
                off + 8 * slots.len()
            }
            d @ (Desc::Unit | Desc::Int | Desc::Bool | Desc::Float) => panic!("{d:?} is not an object"),
        }
    }
}

// Every build counts live objects, for the leak check at the end of a run (pinned statics are
// never allocated, so never counted). Test builds also poison freed objects without reusing
// them, so a use after free fails `check_live`. Under Miri memory is really freed: Miri
// catches the use itself.

static LIVE: core::sync::atomic::AtomicIsize = core::sync::atomic::AtomicIsize::new(0);

/// Objects allocated and not yet freed.
pub fn live() -> isize {
    LIVE.load(core::sync::atomic::Ordering::Relaxed)
}

fn live_add(n: isize) {
    LIVE.fetch_add(n, core::sync::atomic::Ordering::Relaxed);
}

#[cfg(all(any(test, feature = "test-heap"), not(miri)))]
const POISON: u8 = 0xdb;

fn check_live(_rc: u32) {
    #[cfg(all(any(test, feature = "test-heap"), not(miri)))]
    assert!(_rc != 0 && _rc != u32::from_ne_bytes([POISON; 4]), "use after free");
}

unsafe fn free_memory(p: *mut Header, size: usize) {
    live_add(-1);
    #[cfg(all(any(test, feature = "test-heap"), not(miri)))]
    // SAFETY: `p` owns `size` bytes, now never reused.
    unsafe {
        ptr::write_bytes(p.cast::<u8>(), POISON, size)
    };
    #[cfg(not(all(any(test, feature = "test-heap"), not(miri))))]
    // SAFETY: `p` was allocated by `new` with this size.
    unsafe {
        alloc::alloc::dealloc(p.cast(), layout(size))
    };
}
