//! Heap objects and their reference counts.
//!
//! Layout, in bytes; every object is 8-aligned and a slot is 8 bytes:
//!
//! ```text
//! Record      [rc u32][desc u32][slot 0]..[slot n-1]
//! Enum        [rc][desc][variant u32][0 u32][slot 0]..     the slots of that variant
//! Str, Bytes  [rc][desc][len u32][cap u32][bytes; cap]
//! List        [rc][desc][ListBody: elements, len, cap]
//! Map         [rc][desc][Map]
//! Fn          [rc][desc][table index u32][0 u32][capture 0]..
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
use crate::heap;
use alloc::alloc::{handle_alloc_error, Layout};
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
/// order by bytes and are not counted. Ordered as a derived `Ord` would (`Int` before `Str`),
/// with `Str`s compared 8 bytes at a time.
#[derive(Debug, Clone)]
pub enum Key {
    Int(i64),
    Str(Box<[u8]>),
}

impl Ord for Key {
    fn cmp(&self, other: &Key) -> core::cmp::Ordering {
        match (self, other) {
            (Key::Int(a), Key::Int(b)) => a.cmp(b),
            (Key::Str(a), Key::Str(b)) => crate::text::bytes_cmp(a, b),
            (Key::Int(_), Key::Str(_)) => core::cmp::Ordering::Less,
            (Key::Str(_), Key::Int(_)) => core::cmp::Ordering::Greater,
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Key) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        match (self, other) {
            (Key::Int(a), Key::Int(b)) => a == b,
            (Key::Str(a), Key::Str(b)) => crate::text::bytes_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for Key {}

pub type List = Vec<u64>;
pub type Map = BTreeMap<Key, u64>;

/// How a `List` object holds its elements: the parts of a `Vec<u64>`, in a layout fixed here
/// (`Vec`'s own is not), so generated code may read them. On wasm32 the elements' address is
/// at byte 8 of the object, `len` at 12 and `cap` at 16; codegen reads `len` and the elements
/// and, for a `push` with room, writes an element and `len`.
#[repr(C)]
pub struct ListBody {
    ptr: *mut u64,
    len: usize,
    cap: usize,
}

#[cfg(target_arch = "wasm32")]
const _: () =
    assert!(HEADER + core::mem::offset_of!(ListBody, len) == 12 && HEADER + core::mem::offset_of!(ListBody, cap) == 16);

pub(crate) const HEADER: usize = 8;
const ENUM_SLOTS: usize = 16;
pub(crate) const BYTES_DATA: usize = 16;
const _: () = assert!(HEADER.is_multiple_of(align_of::<ListBody>()) && HEADER.is_multiple_of(align_of::<Map>()));

impl ListBody {
    fn of(v: List) -> ListBody {
        let mut v = core::mem::ManuallyDrop::new(v);
        ListBody {
            ptr: v.as_mut_ptr(),
            len: v.len(),
            cap: v.capacity(),
        }
    }
}

/// The `ListBody` of `List` object `p`.
///
/// # Safety
/// `p` is a `List`.
unsafe fn body(p: *mut Header) -> *mut ListBody {
    // SAFETY: the caller's guarantee.
    unsafe { field(p, HEADER) }
}

/// The elements of `List` `p`.
///
/// # Safety
/// `p` is a `List` that outlives `'a` unchanged.
pub unsafe fn elems<'a>(p: *mut Header) -> &'a [u64] {
    // SAFETY: the caller's guarantee; `ptr` and `len` are a `Vec`'s.
    unsafe {
        let b = &*body(p);
        core::slice::from_raw_parts(b.ptr, b.len)
    }
}

/// The elements of `List` `p`, to change.
///
/// # Safety
/// `p` is a `List` that outlives `'a`, and nothing else reads or changes its elements
/// meanwhile.
pub unsafe fn elems_mut<'a>(p: *mut Header) -> &'a mut [u64] {
    // SAFETY: the caller's guarantee; `ptr` and `len` are a `Vec`'s.
    unsafe {
        let b = &*body(p);
        core::slice::from_raw_parts_mut(b.ptr, b.len)
    }
}

/// `List` `p`'s elements as the `Vec` they are, to change its length; written back when the
/// guard drops.
///
/// # Safety
/// `p` is a `List` that outlives `'a`, and nothing else reads or changes it meanwhile.
pub unsafe fn list_mut<'a>(p: *mut Header) -> ListMut<'a> {
    // SAFETY: the caller's guarantee; the parts are a `Vec`'s, which the guard owns until it
    // puts them back.
    unsafe {
        let b = &*body(p);
        ListMut {
            body: body(p),
            v: core::mem::ManuallyDrop::new(Vec::from_raw_parts(b.ptr, b.len, b.cap)),
            _p: core::marker::PhantomData,
        }
    }
}

/// `list_mut`'s guard.
pub struct ListMut<'a> {
    body: *mut ListBody,
    v: core::mem::ManuallyDrop<List>,
    _p: core::marker::PhantomData<&'a mut List>,
}

impl core::ops::Deref for ListMut<'_> {
    type Target = List;
    fn deref(&self) -> &List {
        &self.v
    }
}

impl core::ops::DerefMut for ListMut<'_> {
    fn deref_mut(&mut self) -> &mut List {
        &mut self.v
    }
}

impl Drop for ListMut<'_> {
    fn drop(&mut self) {
        // SAFETY: `body` is the list's, which this guard's `list_mut` contract keeps for us;
        // the `Vec` moves back into it.
        unsafe {
            self.body
                .write(ListBody::of(core::mem::ManuallyDrop::take(&mut self.v)))
        }
    }
}

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

/// A fresh object of `size` bytes, `rc` 1. The bytes after the header are uninitialized for
/// the caller to write: a record's slots, a `List`'s or `Map`'s body, a copy's bytes, or a
/// `Str`'s or `Bytes`' header and the bytes below its `len` (those above it are never read).
fn new(size: usize, desc: u32) -> *mut Header {
    let layout = layout(size);
    // SAFETY: `size` is at least HEADER, so not zero.
    let p = unsafe { heap::alloc(layout) }.cast::<Header>();
    if p.is_null() {
        handle_alloc_error(layout);
    }
    // SAFETY: a fresh allocation of at least HEADER bytes, 8-aligned.
    unsafe { p.write(Header { rc: 1, desc }) };
    live_add(1);
    p
}

/// A struct, variant `variant` of an enum, or a closure calling table entry `variant`, every
/// slot 0. Fill the slots with `slot`.
pub fn alloc(t: &Table, desc: u32, variant: u32) -> *mut Header {
    let (off, n, is_enum) = match t.desc(desc) {
        Desc::Record { slots, .. } => (HEADER, slots.len, false),
        Desc::Enum { variants, .. } => (ENUM_SLOTS, t.variant(variants, variant).slots.len, true),
        Desc::Fn { slots, .. } => (ENUM_SLOTS, slots.len, true),
        d => panic!("rt_alloc of {d:?}"),
    };
    let size = off + 8 * n as usize;
    let p = new(size, desc);
    // SAFETY: `p` has `size` bytes, 8-aligned.
    unsafe { heap::zero(field(p, HEADER), size - HEADER) };
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
    // SAFETY: `p` has `len` and `cap` at 8 and 12, then room for `cap` >= `len` bytes: the
    // first `len` are written here, the rest are never read.
    unsafe {
        field::<u32>(p, 8).write(len);
        field::<u32>(p, 12).write(cap);
        ptr::copy_nonoverlapping(b.as_ptr(), field(p, BYTES_DATA), b.len());
    }
    p
}

/// `Str` or `Bytes` `p`, which has no other reference, moved to a buffer with room for `cap`
/// bytes (at least its length); `p` is freed.
///
/// # Safety
/// The module's contract; `p` is a `Str` or `Bytes` with count 1.
pub unsafe fn regrow(p: *mut Header, cap: usize) -> *mut Header {
    // SAFETY: the caller's guarantee; nothing else refers to `p`, so freeing it is ours.
    unsafe {
        assert!((*p).rc == 1);
        let q = new_bytes_cap((*p).desc, bytes(p), cap);
        free_memory(p, BYTES_DATA + *field::<u32>(p, 12) as usize);
        q
    }
}

/// Room for the `len` bytes of a `Str` or `Bytes` that `adopt` turns into one: the address
/// of its data, after space for the header. For `cabi_realloc` (runtime/src/caps.rs), so a
/// string the host returns lands in its object with no copy.
pub fn bytes_room(len: usize) -> *mut u8 {
    let layout = layout(BYTES_DATA + len);
    // SAFETY: the size is at least BYTES_DATA, so not zero.
    let p = unsafe { heap::alloc(layout) };
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
    let p = new(HEADER + size_of::<ListBody>(), desc);
    // SAFETY: room for a `ListBody` at HEADER, aligned (asserted above).
    unsafe { body(p).write(ListBody::of(v)) };
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
                               // One descriptor lookup per object (C8b2): its size and children as `size` and `ptrs` find
                               // them.
    loop {
        // SAFETY: `p` is unreferenced, so its children and memory are ours to release.
        unsafe {
            let mut child = |c| {
                if release(c) {
                    work.push(c)
                }
            };
            let size = match t.desc((*p).desc) {
                Desc::Str | Desc::Bytes => BYTES_DATA + *field::<u32>(p, 12) as usize,
                Desc::List { elem } => {
                    if t.is_ptr(elem) {
                        elems(p).iter().for_each(|&v| child(from_slot(v)));
                    }
                    let b = body(p).read();
                    drop(Vec::from_raw_parts(b.ptr, b.len, b.cap));
                    HEADER + size_of::<ListBody>()
                }
                Desc::Map { value, .. } => {
                    if t.is_ptr(value) {
                        (*field::<Map>(p, HEADER)).values().for_each(|&v| child(from_slot(v)));
                    }
                    ptr::drop_in_place(field::<Map>(p, HEADER));
                    HEADER + size_of::<Map>()
                }
                d @ (Desc::Record { .. } | Desc::Enum { .. } | Desc::Fn { .. }) => {
                    let (off, slots) = record_of(t, d, p);
                    for (i, &s) in slots.iter().enumerate() {
                        if t.is_ptr(s) {
                            child(from_slot(*field::<u64>(p, off + 8 * i)));
                        }
                    }
                    off + 8 * slots.len()
                }
                d @ (Desc::Unit | Desc::Int | Desc::Bool | Desc::Float) => panic!("{d:?} is not an object"),
            };
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
        if let Desc::Str | Desc::Bytes = t.desc(desc) {
            // Only the bytes below `len` are initialized; the copy keeps the capacity.
            return new_bytes_cap(desc, bytes(p), size - BYTES_DATA);
        }
        let q = new(size, desc);
        match t.desc(desc) {
            Desc::List { .. } => body(q).write(ListBody::of(elems(p).to_vec())),
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
            Desc::List { elem } if t.is_ptr(elem) => elems(p).iter().for_each(|&v| f(from_slot(v))),
            Desc::Map { value, .. } if t.is_ptr(value) => {
                (*field::<Map>(p, HEADER)).values().for_each(|&v| f(from_slot(v)))
            }
            Desc::Record { .. } | Desc::Enum { .. } | Desc::Fn { .. } => {
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

/// A struct's, enum variant's or closure's first slot offset and slot descriptors.
pub(crate) unsafe fn record<'t>(t: &Table<'t>, p: *mut Header) -> (usize, &'t [u32]) {
    // SAFETY: the caller's guarantee.
    unsafe { record_of(t, t.desc((*p).desc), p) }
}

/// `record`, given `p`'s descriptor `d`.
unsafe fn record_of<'t>(t: &Table<'t>, d: Desc, p: *mut Header) -> (usize, &'t [u32]) {
    // SAFETY: `p` is live; an enum has its variant at HEADER.
    unsafe {
        match d {
            Desc::Record { slots, .. } => (HEADER, t.slots(slots)),
            Desc::Enum { variants, .. } => (ENUM_SLOTS, t.slots(t.variant(variants, *field(p, HEADER)).slots)),
            Desc::Fn { slots, .. } => (ENUM_SLOTS, t.slots(slots)),
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
            Desc::List { .. } => HEADER + size_of::<ListBody>(),
            Desc::Map { .. } => HEADER + size_of::<Map>(),
            Desc::Record { .. } | Desc::Enum { .. } | Desc::Fn { .. } => {
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
    // SAFETY: `p` was allocated by `new` (or `bytes_room`) with this size.
    unsafe {
        heap::dealloc(p.cast(), layout(size))
    };
}
