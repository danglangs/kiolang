//! The functions generated code calls. A pointer is a `u32` address in linear memory, 0 for
//! none; an `Int` or a slot value is an `i64`; a `Bool` is returned as 0 or 1. Each function
//! keeps `obj`'s contract if its arguments do, which codegen guarantees.
//!
//! The descriptor table is data codegen emits: a `RawTable` of five (address, length) pairs,
//! registered once with `rt_init` before any other call.
//!
//! Errors: a function that can fail takes the error site as its last argument and on failure
//! calls `rt_trap(site, a, b)` with the values the site's message needs, so it never returns an
//! error and codegen emits no check. Each such function has one kind of error.

use crate::desc::{RawTable, Table};
use crate::entry::{rt_trap, trap3};
use crate::obj::{self, Header};
use crate::{coll, desc, float, text};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

static TABLE: AtomicU32 = AtomicU32::new(0);

pub(crate) fn table() -> Table<'static> {
    let raw = TABLE.load(Relaxed);
    assert!(raw != 0, "rt_init was not called");
    // SAFETY: `rt_init`'s contract; Wasm memory starts at address 0.
    unsafe { RawTable::read(0, raw) }
}

pub(crate) fn ptr(a: u32) -> *mut Header {
    core::ptr::with_exposed_provenance_mut(a as usize)
}

pub(crate) fn addr(p: *mut Header) -> u32 {
    p.expose_provenance() as u32
}

/// # Safety
/// `table` is the address of a `RawTable` whose arrays are valid, aligned, never written, and
/// keep `desc`'s invariants.
#[no_mangle]
pub unsafe extern "C" fn rt_init(table: u32) {
    TABLE.store(table, Relaxed);
}

/// A struct, or variant `variant` of an enum, with count 1 and every slot 0.
#[no_mangle]
pub extern "C" fn rt_alloc(desc: u32, variant: u32) -> u32 {
    addr(obj::alloc(&table(), desc, variant))
}

/// # Safety
/// `obj`'s contract.
#[no_mangle]
pub unsafe extern "C" fn rt_inc(p: u32) {
    // SAFETY: the caller's guarantee.
    unsafe { obj::inc(ptr(p)) }
}

/// # Safety
/// `obj`'s contract.
#[no_mangle]
pub unsafe extern "C" fn rt_dec(p: u32) {
    // SAFETY: the caller's guarantee.
    unsafe { obj::dec(&table(), ptr(p)) }
}

/// # Safety
/// `obj`'s contract, and `p` is not 0.
#[no_mangle]
pub unsafe extern "C" fn rt_unique(p: u32) -> u32 {
    // SAFETY: the caller's guarantee.
    unsafe { addr(obj::unique(&table(), ptr(p))) }
}

/// Objects allocated and not yet freed: 0 at the end of a run that did not trap.
#[no_mangle]
pub extern "C" fn rt_live() -> u32 {
    obj::live() as u32
}

// Values and built-in methods. Every argument is borrowed: the caller still owns it and decs it
// after the call (docs/codegen.md, "Ownership"). Every pointer result is new (+1), as is a
// pointer slot result (`rt_list_get`, `rt_option_unwrap_or`). A `desc` argument is the
// descriptor of the result, for results the runtime cannot name itself.
//
// # Safety (every function in this group and the next)
// `obj`'s contract, and each pointer is of the type its name says.

/// `[]` of descriptor `desc`, with room for `cap` elements; `rt_list_push` fills it.
#[no_mangle]
pub extern "C" fn rt_list_new(desc: u32, cap: u32) -> u32 {
    addr(coll::list_new(desc, alloc::vec::Vec::with_capacity(cap as usize)))
}

/// `Map::new()` of descriptor `desc`.
#[no_mangle]
pub extern "C" fn rt_map_new(desc: u32) -> u32 {
    addr(coll::map_new(desc))
}

/// `a == b` on values of descriptor `desc`.
#[no_mangle]
pub unsafe extern "C" fn rt_eq(desc: u32, a: i64, b: i64) -> bool {
    // SAFETY: the group's contract.
    unsafe { coll::eq(&table(), desc, a as u64, b as u64) }
}

/// `a + b` on `Str`.
#[no_mangle]
pub unsafe extern "C" fn rt_concat(a: u32, b: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::concat(ptr(a), ptr(b))) }
}

#[no_mangle]
pub extern "C" fn rt_int_to_str(n: i64) -> u32 {
    addr(text::int_to_str(n))
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_len(s: u32) -> i64 {
    // SAFETY: the group's contract.
    unsafe { text::len(ptr(s)) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_to_bytes(s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::to_bytes(ptr(s))) }
}

/// `desc` is `List<Str>`. Error: an empty separator; `a` and `b` are 0.
#[no_mangle]
pub unsafe extern "C" fn rt_str_split(desc: u32, s: u32, sep: u32, site: u32) -> u32 {
    // SAFETY: the group's contract.
    match unsafe { text::split(desc, ptr(s), ptr(sep)) } {
        Ok(p) => addr(p),
        Err(_) => rt_trap(site, 0, 0),
    }
}

/// `desc` is `List<Str>`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_lines(desc: u32, s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::lines(desc, ptr(s))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_trim(s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::trim(ptr(s))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_contains(s: u32, sub: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { text::contains(ptr(s), ptr(sub)) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_starts_with(s: u32, prefix: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { text::starts_with(ptr(s), ptr(prefix)) }
}

/// `desc` is `Option<Int>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_parse_int(desc: u32, none: u32, s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::parse_int(&table(), desc, ptr(none), ptr(s))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_ends_with(s: u32, suffix: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { text::ends_with(ptr(s), ptr(suffix)) }
}

/// `desc` is `Option<Str>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_strip_prefix(desc: u32, none: u32, s: u32, p: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::strip_prefix(&table(), desc, ptr(none), ptr(s), ptr(p))) }
}

/// `desc` is `Option<Str>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_strip_suffix(desc: u32, none: u32, s: u32, p: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::strip_suffix(&table(), desc, ptr(none), ptr(s), ptr(p))) }
}

/// `desc` is `Option<Int>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_find(desc: u32, none: u32, s: u32, sub: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::find(&table(), desc, ptr(none), ptr(s), ptr(sub))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_replace(s: u32, from: u32, to: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::replace(ptr(s), ptr(from), ptr(to))) }
}

/// `desc` is `Option<Float>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_parse_float(desc: u32, none: u32, s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::parse_float(&table(), desc, ptr(none), ptr(s))) }
}

/// `s.slice(a, b)`. Errors: out of bounds at `site`, with `a`, `b` and the length as the
/// message's third value (`trap3`); not on a character boundary at `boundary`, with `a`, `b`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_slice(s: u32, a: i64, b: i64, site: u32, boundary: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe {
        match text::slice(ptr(s), a, b) {
            Ok(p) => addr(p),
            Err(text::Err::CharBoundary) => rt_trap(boundary, a, b),
            Err(_) => trap3(site, a, b, text::len(ptr(s))),
        }
    }
}

/// `v.slice(a, b)`. Error: out of bounds, with `a`, `b` and the length as the message's third
/// value (`trap3`).
#[no_mangle]
pub unsafe extern "C" fn rt_bytes_slice(v: u32, a: i64, b: i64, site: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe {
        match text::bytes_slice(ptr(v), a, b) {
            Ok(p) => addr(p),
            Err(_) => trap3(site, a, b, text::len(ptr(v))),
        }
    }
}

#[no_mangle]
pub extern "C" fn rt_float_to_str(x: f64) -> u32 {
    addr(text::float_to_str(x))
}

/// `desc` is `Option<Int>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_float_to_int(desc: u32, none: u32, x: f64) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(float::to_int(&table(), desc, ptr(none), x)) }
}

#[no_mangle]
pub extern "C" fn rt_float_rem(a: f64, b: f64) -> f64 {
    float::rem(a, b)
}

#[no_mangle]
pub extern "C" fn rt_float_round(x: f64) -> f64 {
    float::round(x)
}

#[no_mangle]
pub unsafe extern "C" fn rt_bytes_len(b: u32) -> i64 {
    // SAFETY: the group's contract.
    unsafe { text::len(ptr(b)) }
}

/// Error: invalid UTF-8; `a` and `b` are 0.
#[no_mangle]
pub unsafe extern "C" fn rt_bytes_to_str(b: u32, site: u32) -> u32 {
    // SAFETY: the group's contract.
    match unsafe { text::to_str(ptr(b)) } {
        Ok(p) => addr(p),
        Err(_) => rt_trap(site, 0, 0),
    }
}

/// `b[i]`. Error: out of bounds; `a` is `i`, `b` the length.
#[no_mangle]
pub unsafe extern "C" fn rt_bytes_get(b: u32, i: i64, site: u32) -> i64 {
    // SAFETY: the group's contract.
    unsafe {
        match text::index(ptr(b), i) {
            Ok(n) => n,
            Err(_) => rt_trap(site, i, text::len(ptr(b))),
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn rt_list_len(l: u32) -> i64 {
    // SAFETY: the group's contract.
    unsafe { coll::list_len(ptr(l)) }
}

/// `l[i]`. Error: out of bounds; `a` is `i`, `b` the length.
#[no_mangle]
pub unsafe extern "C" fn rt_list_get(l: u32, i: i64, site: u32) -> i64 {
    // SAFETY: the group's contract.
    match unsafe { coll::list_get(&table(), ptr(l), i) } {
        Ok(v) => v as i64,
        Err(e) => index_trap(site, e),
    }
}

#[no_mangle]
pub unsafe extern "C" fn rt_map_len(m: u32) -> i64 {
    // SAFETY: the group's contract.
    unsafe { coll::map_len(ptr(m)) }
}

/// `desc` is `List<K>`.
#[no_mangle]
pub unsafe extern "C" fn rt_map_keys(desc: u32, m: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::map_keys(&table(), ptr(m), desc)) }
}

/// `desc` is `Option<V>`, `none` its pinned `None`; `k` is an `Int` or a `Str` pointer.
#[no_mangle]
pub unsafe extern "C" fn rt_map_get(desc: u32, none: u32, m: u32, k: i64) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::map_get(&table(), ptr(m), k as u64, desc, ptr(none))) }
}

/// `l.contains(v)`; `v` is a slot value of the element type.
#[no_mangle]
pub unsafe extern "C" fn rt_list_contains(l: u32, v: i64) -> bool {
    // SAFETY: the group's contract.
    unsafe { coll::list_contains(&table(), ptr(l), v as u64) }
}

/// `r.ok()`: `desc` is `Option<T>`, `none` its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_result_ok(desc: u32, none: u32, r: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::result_variant(&table(), ptr(r), 0, desc, ptr(none))) }
}

/// `r.err()`: `desc` is `Option<E>`, `none` its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_result_err(desc: u32, none: u32, r: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::result_variant(&table(), ptr(r), 1, desc, ptr(none))) }
}

/// `o.ok_or(e)`: `desc` is `Result<T, E>`; `e` is a slot value of `E`.
#[no_mangle]
pub unsafe extern "C" fn rt_option_ok_or(desc: u32, o: u32, e: i64) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::option_ok_or(&table(), ptr(o), e as u64, desc)) }
}

/// `o.unwrap_or(d)` on an `Option`.
#[no_mangle]
pub unsafe extern "C" fn rt_option_unwrap_or(o: u32, d: i64) -> i64 {
    // SAFETY: the group's contract.
    unsafe { unwrap_or(o, 1, d) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_option_is_some(o: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { variant(o) == 1 }
}

#[no_mangle]
pub unsafe extern "C" fn rt_option_is_none(o: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { variant(o) == 0 }
}

/// `r.unwrap_or(d)` on a `Result`.
#[no_mangle]
pub unsafe extern "C" fn rt_result_unwrap_or(r: u32, d: i64) -> i64 {
    // SAFETY: the group's contract.
    unsafe { unwrap_or(r, 0, d) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_result_is_ok(r: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { variant(r) == 0 }
}

#[no_mangle]
pub unsafe extern "C" fn rt_result_is_err(r: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { variant(r) == 1 }
}

// Mutating methods and place writes. The container is the place's own object, which the caller
// has made unique (`rt_unique`) and stored back; it is changed in place. A value argument is
// consumed: the container owns it, except when the call traps. A key is borrowed.

/// `l[i] = v`. Error: out of bounds; `a` is `i`, `b` the length.
#[no_mangle]
pub unsafe extern "C" fn rt_list_set(l: u32, i: i64, v: i64, site: u32) {
    // SAFETY: the group's contract.
    if let Err(e) = unsafe { coll::list_set(&table(), ptr(l), i, v as u64) } {
        index_trap(site, e)
    }
}

/// The address of `l[i]`'s `u64` slot, which generated code reads and writes: to make a
/// container on a place's path unique and store it back, or for `l[i] += v`. Valid until `l`
/// next changes. Error: out of bounds; `a` is `i`, `b` the length.
#[no_mangle]
pub unsafe extern "C" fn rt_list_at(l: u32, i: i64, site: u32) -> u32 {
    // SAFETY: the group's contract.
    match unsafe { coll::list_slot(ptr(l), i) } {
        Ok(p) => p.expose_provenance() as u32,
        Err(e) => index_trap(site, e),
    }
}

/// `l.push(v)`.
#[no_mangle]
pub unsafe extern "C" fn rt_list_push(l: u32, v: i64) {
    // SAFETY: the group's contract.
    unsafe { coll::list_push(ptr(l), v as u64) }
}

/// `l.sort()` on a `List<Int>` or `List<Str>`.
#[no_mangle]
pub unsafe extern "C" fn rt_list_sort(l: u32) {
    // SAFETY: the group's contract.
    unsafe { coll::list_sort(&table(), ptr(l)) }
}

/// `l.pop()`: `desc` is `Option<T>`, `none` its pinned `None`. The element moves into the
/// `Some`.
#[no_mangle]
pub unsafe extern "C" fn rt_list_pop(desc: u32, none: u32, l: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::list_pop(&table(), ptr(l), desc, ptr(none))) }
}

/// `b.push(n)`: returns the `Bytes` to store back in the place, `b` or a larger copy (`b` then
/// freed). Error: `n` outside 0..=255; `a` is `n`, `b` 0.
#[no_mangle]
pub unsafe extern "C" fn rt_bytes_push(b: u32, n: i64, site: u32) -> u32 {
    // SAFETY: the group's contract.
    match unsafe { text::push(&table(), ptr(b), n) } {
        Ok(p) => addr(p),
        Err(_) => rt_trap(site, n, 0),
    }
}

/// `m.insert(k, v)`.
#[no_mangle]
pub unsafe extern "C" fn rt_map_insert(m: u32, k: i64, v: i64) {
    // SAFETY: the group's contract.
    unsafe { coll::map_insert(&table(), ptr(m), k as u64, v as u64) }
}

/// `m.remove(k)`.
#[no_mangle]
pub unsafe extern "C" fn rt_map_remove(m: u32, k: i64) {
    // SAFETY: the group's contract.
    unsafe { coll::map_remove(&table(), ptr(m), k as u64) }
}

fn index_trap(site: u32, e: coll::Error) -> ! {
    let coll::Error::Index { index, len } = e;
    rt_trap(site, index, len as i64)
}

/// The variant number of enum object `p`.
///
/// # Safety
/// `p` is an enum object.
unsafe fn variant(p: u32) -> u32 {
    // SAFETY: the caller's guarantee; an enum has its variant after the header.
    unsafe { *obj::field::<u32>(ptr(p), obj::HEADER) }
}

/// The payload of `p` if it is variant `success`, else `d`; owned either way.
///
/// # Safety
/// `p` is an `Option` or `Result` object and `d` a value of its payload type.
unsafe fn unwrap_or(p: u32, success: u32, d: i64) -> i64 {
    let t = table();
    // SAFETY: the caller's guarantee; variant `success` has one slot.
    unsafe {
        let v = if variant(p) == success {
            *obj::slot(&t, ptr(p), 0)
        } else {
            d as u64
        };
        let desc::Desc::Enum { variants, .. } = t.desc((*ptr(p)).desc) else {
            unreachable!("an Option or Result")
        };
        if t.is_ptr(t.slots(t.variant(variants, success).slots)[0]) {
            obj::inc(obj::from_slot(v));
        }
        v as i64
    }
}
