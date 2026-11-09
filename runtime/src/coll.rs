//! List and Map operations, and structural equality. Semantics are the interpreter's.
//!
//! Ownership, per docs/codegen.md: a receiver is borrowed (codegen decs it after the call), and
//! so is an `Int` or `Str` key. A mutator (`list_set`, `push`, `insert`, `remove`) takes a
//! receiver the caller made `unique` and changes it in place. A value passed to `list_set`,
//! `push` or `insert` is owned by the container from then on, except when the call fails: then
//! nothing was consumed. Results are owned (+1). Whether a `u64` is a pointer comes from the
//! container's own descriptor, so callers pass no element types.
//!
//! Every `unsafe fn` has the contract of `obj`, and its container argument has the type its
//! name says.

use crate::desc::{self, Desc, Table};
use crate::obj::{self, bytes, field, from_slot, to_slot, Header, Key, List, Map};
use alloc::vec::Vec;

/// A program error. Codegen turns it into a `trap`: `index {index} out of bounds for length {len}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Index { index: i64, len: u64 },
}

const BODY: usize = 8;

unsafe fn list<'a>(p: *mut Header) -> &'a mut List {
    // SAFETY: a `List` object holds its `List` at BODY.
    unsafe { &mut *field(p, BODY) }
}

unsafe fn map<'a>(p: *mut Header) -> &'a mut Map {
    // SAFETY: a `Map` object holds its `Map` at BODY.
    unsafe { &mut *field(p, BODY) }
}

/// The element descriptor of list `p`.
unsafe fn elem(t: &Table, p: *mut Header) -> u32 {
    // SAFETY: `p` is live.
    match t.desc(unsafe { (*p).desc }) {
        Desc::List { elem } => elem,
        d => panic!("{d:?} is not a List"),
    }
}

/// The key and value descriptors of map `p`.
unsafe fn kv(t: &Table, p: *mut Header) -> (u32, u32) {
    // SAFETY: `p` is live.
    match t.desc(unsafe { (*p).desc }) {
        Desc::Map { key, value } => (key, value),
        d => panic!("{d:?} is not a Map"),
    }
}

/// The `Key` for slot value `v` of key type `key`; a `Str` is read, not consumed.
unsafe fn key_of(key: u32, v: u64) -> Key {
    match key {
        desc::INT => Key::Int(v as i64),
        // SAFETY: `v` is a live `Str`.
        desc::STR => Key::Str(unsafe { bytes(from_slot(v)) }.into()),
        k => panic!("descriptor {k} is not a key type"),
    }
}

/// A slot value for a counted reference to `v`: `v` itself, inc'd if it is a pointer.
unsafe fn share(t: &Table, d: u32, v: u64) -> u64 {
    if t.is_ptr(d) {
        // SAFETY: a pointer slot holds a live object.
        unsafe { obj::inc(from_slot(v)) };
    }
    v
}

/// Drops the reference `v` holds, if it is a pointer.
unsafe fn release(t: &Table, d: u32, v: u64) {
    if t.is_ptr(d) {
        // SAFETY: a pointer slot holds an owned reference.
        unsafe { obj::dec(t, from_slot(v)) };
    }
}

fn at(index: i64, len: usize) -> Result<usize, Error> {
    match usize::try_from(index) {
        Ok(i) if i < len => Ok(i),
        _ => Err(Error::Index { index, len: len as u64 }),
    }
}

/// A list of descriptor `desc` owning `elems`' references.
pub fn list_new(desc: u32, elems: Vec<u64>) -> *mut Header {
    obj::new_list(desc, elems)
}

/// # Safety
/// `p` is a `List`.
pub unsafe fn list_len(p: *mut Header) -> i64 {
    // SAFETY: the caller's guarantee.
    unsafe { list(p) }.len() as i64
}

/// `xs[i]`, owned: a pointer element gains a reference.
///
/// # Safety
/// `p` is a `List`.
pub unsafe fn list_get(t: &Table, p: *mut Header, i: i64) -> Result<u64, Error> {
    // SAFETY: the caller's guarantee.
    unsafe {
        let l = list(p);
        let v = l[at(i, l.len())?];
        Ok(share(t, elem(t, p), v))
    }
}

/// The slot of `xs[i]`, for a place write that descends through it or changes it in place.
/// Valid until the list next changes or is freed.
///
/// # Safety
/// `p` is a unique `List`.
pub unsafe fn list_slot(p: *mut Header, i: i64) -> Result<*mut u64, Error> {
    // SAFETY: the caller's guarantee.
    let l = unsafe { list(p) };
    let i = at(i, l.len())?;
    Ok(&mut l[i])
}

/// `xs[i] = v` on a unique list. The slot takes `v` before the old element is dec'd, so a `v`
/// that is (a reference to) the old element, which holds its own count, is never freed first.
///
/// # Safety
/// `p` is a unique `List`.
pub unsafe fn list_set(t: &Table, p: *mut Header, i: i64, v: u64) -> Result<(), Error> {
    // SAFETY: the caller's guarantee.
    unsafe {
        let l = list(p);
        let i = at(i, l.len())?;
        let old = core::mem::replace(&mut l[i], v);
        release(t, elem(t, p), old);
        Ok(())
    }
}

/// `xs.push(v)` on a unique list.
///
/// # Safety
/// `p` is a unique `List`.
pub unsafe fn list_push(p: *mut Header, v: u64) {
    // SAFETY: the caller's guarantee.
    unsafe { list(p) }.push(v);
}

/// An empty map of descriptor `desc`.
pub fn map_new(desc: u32) -> *mut Header {
    obj::new_map(desc, Map::new())
}

/// # Safety
/// `p` is a `Map`.
pub unsafe fn map_len(p: *mut Header) -> i64 {
    // SAFETY: the caller's guarantee.
    unsafe { map(p) }.len() as i64
}

/// `m.keys()` as a `List` of descriptor `list_desc`, in key order. `Str` keys are rebuilt as
/// new `Str` objects.
///
/// # Safety
/// `p` is a `Map`, and `list_desc` is the list of its key type.
pub unsafe fn map_keys(t: &Table, p: *mut Header, list_desc: u32) -> *mut Header {
    // SAFETY: the caller's guarantee.
    let keys = unsafe { map(p) }
        .keys()
        .map(|k| match k {
            Key::Int(n) => *n as u64,
            Key::Str(s) => to_slot(obj::new_bytes(desc::STR, s)),
        })
        .collect();
    debug_assert!(matches!(t.desc(list_desc), Desc::List { .. }));
    obj::new_list(list_desc, keys)
}

/// `m.get(k)` as an `Option<V>` object of descriptor `option_desc` (variant 0 `None`, 1
/// `Some`). `k` is an `Int` or a `Str` pointer, read not consumed. `Some` holds a new reference.
///
/// # Safety
/// `p` is a `Map`; `k` is a key of its key type; `option_desc` is `Option` of its value type.
pub unsafe fn map_get(t: &Table, p: *mut Header, k: u64, option_desc: u32) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe {
        let (key, value) = kv(t, p);
        match map(p).get(&key_of(key, k)) {
            None => obj::alloc(t, option_desc, 0),
            Some(&v) => {
                let o = obj::alloc(t, option_desc, 1);
                *obj::slot(t, o, 0) = share(t, value, v);
                o
            }
        }
    }
}

/// `m.insert(k, v)` on a unique map. The map owns `v`; an existing value is replaced and
/// dec'd. `k` is an `Int` or a `Str` pointer, read not consumed.
///
/// # Safety
/// `p` is a unique `Map`; `k` is a key of its key type.
pub unsafe fn map_insert(t: &Table, p: *mut Header, k: u64, v: u64) {
    // SAFETY: the caller's guarantee.
    unsafe {
        let (key, value) = kv(t, p);
        if let Some(old) = map(p).insert(key_of(key, k), v) {
            release(t, value, old);
        }
    }
}

/// `m.remove(k)` on a unique map; an absent key does nothing.
///
/// # Safety
/// `p` is a unique `Map`; `k` is a key of its key type.
pub unsafe fn map_remove(t: &Table, p: *mut Header, k: u64) {
    // SAFETY: the caller's guarantee.
    unsafe {
        let (key, value) = kv(t, p);
        if let Some(old) = map(p).remove(&key_of(key, k)) {
            release(t, value, old);
        }
    }
}

/// Structural `==` of two values of descriptor `d`, as slot values. A worklist, not recursion,
/// so a deep value costs heap, not stack.
///
/// # Safety
/// `a` and `b` are slot values of descriptor `d`.
pub unsafe fn eq(t: &Table, d: u32, a: u64, b: u64) -> bool {
    let mut work = alloc::vec![(d, a, b)];
    while let Some((d, a, b)) = work.pop() {
        // SAFETY: each pair is slot values of its descriptor, by the caller and by induction.
        if !unsafe { eq_one(t, &mut work, d, a, b) } {
            return false;
        }
    }
    true
}

/// Compares one pair, pushing the pairs its equality also needs.
unsafe fn eq_one(t: &Table, work: &mut Vec<(u32, u64, u64)>, d: u32, a: u64, b: u64) -> bool {
    if a == b {
        return true; // equal bits, or the same object
    }
    let desc = t.desc(d);
    // SAFETY: `a` and `b` are live objects of `desc`, or scalars.
    unsafe {
        let (a, b) = (from_slot(a), from_slot(b));
        match desc {
            Desc::Unit | Desc::Int | Desc::Bool => false,
            Desc::Str | Desc::Bytes => bytes(a) == bytes(b),
            Desc::List { elem } => {
                let (x, y) = (list(a), list(b));
                x.len() == y.len() && {
                    work.extend(x.iter().zip(y.iter()).map(|(&x, &y)| (elem, x, y)));
                    true
                }
            }
            Desc::Map { value, .. } => {
                let (x, y) = (map(a), map(b));
                x.len() == y.len()
                    && x.iter().zip(y.iter()).all(|((kx, vx), (ky, vy))| {
                        work.push((value, *vx, *vy));
                        kx == ky
                    })
            }
            Desc::Record { slots, .. } => {
                for (i, &s) in t.slots(slots).iter().enumerate() {
                    work.push((s, *obj::slot(t, a, i), *obj::slot(t, b, i)));
                }
                true
            }
            Desc::Enum { variants, .. } => {
                let v = *field::<u32>(a, BODY);
                if v != *field::<u32>(b, BODY) {
                    return false;
                }
                for (i, &s) in t.slots(t.variant(variants, v).slots).iter().enumerate() {
                    work.push((s, *obj::slot(t, a, i), *obj::slot(t, b, i)));
                }
                true
            }
        }
    }
}
