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
/// `Some`). `k` is an `Int` or a `Str` pointer, read not consumed. `Some` holds a new reference;
/// `none` as `obj::option`'s.
///
/// # Safety
/// `p` is a `Map`; `k` is a key of its key type; `option_desc` is `Option` of its value type;
/// `none` is null or a `None` of it.
pub unsafe fn map_get(t: &Table, p: *mut Header, k: u64, option_desc: u32, none: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe {
        let (key, value) = kv(t, p);
        let v = map(p).get(&key_of(key, k)).map(|&v| share(t, value, v));
        obj::option(t, option_desc, none, v)
    }
}

/// `xs.contains(v)`: `==` with each element. `v` is borrowed.
///
/// # Safety
/// `p` is a `List`; `v` a slot value of its element type.
pub unsafe fn list_contains(t: &Table, p: *mut Header, v: u64) -> bool {
    // SAFETY: the caller's guarantee.
    unsafe {
        let d = elem(t, p);
        list(p).iter().any(|&x| eq(t, d, x, v))
    }
}

/// `xs.sort()` on a unique `List<Int>` (signed) or `List<Str>` (by bytes); stable, as the
/// interpreter's `sort_by`.
///
/// # Safety
/// `p` is a unique `List<Int>` or `List<Str>`.
pub unsafe fn list_sort(t: &Table, p: *mut Header) {
    // SAFETY: the caller's guarantee; a `List<Str>` holds live `Str`s.
    unsafe {
        match elem(t, p) {
            desc::INT => list(p).sort_by_key(|&v| v as i64),
            desc::STR => list(p).sort_by(|&a, &b| bytes(from_slot(a)).cmp(bytes(from_slot(b)))),
            d => panic!("sort of a List of descriptor {d}"),
        }
    }
}

/// `xs.pop()` on a unique list, as an `Option<T>` of descriptor `option_desc`: the last
/// element moves into the `Some` (its reference is the `Some`'s now). `none` as `obj::option`'s.
///
/// # Safety
/// `p` is a unique `List`; `option_desc` is `Option` of its element type; `none` is null or a
/// `None` of it.
pub unsafe fn list_pop(t: &Table, p: *mut Header, option_desc: u32, none: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, option_desc, none, list(p).pop()) }
}

/// The payload of `Option` or `Result` object `p`, shared, if it is variant `v`.
///
/// # Safety
/// `p` is an `Option` or `Result`.
unsafe fn payload_if(t: &Table, p: *mut Header, v: u32) -> Option<u64> {
    // SAFETY: the caller's guarantee; each variant of these has one slot or none.
    unsafe {
        if *field::<u32>(p, BODY) != v {
            return None;
        }
        let (_, ds) = obj::record(t, p);
        Some(share(t, ds[0], *obj::slot(t, p, 0)))
    }
}

/// `r.ok()` (`v` 0) or `r.err()` (`v` 1) as an `Option` of descriptor `option_desc`, holding a
/// new reference to the payload; `none` as `obj::option`'s.
///
/// # Safety
/// `r` is a `Result`; `option_desc` is `Option` of the chosen variant's payload type; `none` is
/// null or a `None` of it.
pub unsafe fn result_variant(t: &Table, r: *mut Header, v: u32, option_desc: u32, none: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, option_desc, none, payload_if(t, r, v)) }
}

/// `o.ok_or(e)` as a `Result` of descriptor `result_desc`: `Ok` with a new reference to `o`'s
/// payload, or `Err` with a new reference to `e`, which is borrowed.
///
/// # Safety
/// `o` is an `Option`; `result_desc` is `Result` of its payload type and `e`'s type, a slot
/// value of which `e` is.
pub unsafe fn option_ok_or(t: &Table, o: *mut Header, e: u64, result_desc: u32) -> *mut Header {
    // SAFETY: the caller's guarantee; each variant of a `Result` has one slot.
    unsafe {
        let (v, x) = match payload_if(t, o, 1) {
            Some(x) => (0, x),
            None => (1, e),
        };
        let r = obj::alloc(t, result_desc, v);
        let (_, ds) = obj::record(t, r);
        *obj::slot(t, r, 0) = if v == 1 { share(t, ds[0], x) } else { x };
        r
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
    let desc = t.desc(d);
    // Equal bits are equal values except for a `Float` (`NaN != NaN`; `-0.0 == 0.0`) and for one
    // object compared with itself, which may hold a NaN, as the interpreter's `Rc` does not
    // assume.
    if desc == Desc::Float {
        return f64::from_bits(a) == f64::from_bits(b);
    }
    // SAFETY: `a` and `b` are live objects of `desc`, or scalars.
    unsafe {
        let (a, b) = (from_slot(a), from_slot(b));
        match desc {
            Desc::Unit | Desc::Int | Desc::Bool | Desc::Float => a == b,
            Desc::Str | Desc::Bytes => a == b || bytes(a) == bytes(b),
            // Shared references: `a` and `b` may be one object.
            Desc::List { elem } => {
                let (x, y) = (&*field::<List>(a, BODY), &*field::<List>(b, BODY));
                x.len() == y.len() && {
                    work.extend(x.iter().zip(y.iter()).map(|(&x, &y)| (elem, x, y)));
                    true
                }
            }
            Desc::Map { value, .. } => {
                let (x, y) = (&*field::<Map>(a, BODY), &*field::<Map>(b, BODY));
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
