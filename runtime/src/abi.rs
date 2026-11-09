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
use crate::{coll, desc, float, json, text};
use alloc::boxed::Box;
use core::cell::UnsafeCell;

/// The table, with its plans (C8b7), as the first `rt_init` read it; `descs` is empty before.
struct Registered(UnsafeCell<Table<'static>>);

// SAFETY: wasm32-unknown-unknown has one thread, and the runtime starts none.
unsafe impl Sync for Registered {}

static TABLE: Registered = Registered(UnsafeCell::new(Table {
    descs: &[],
    slots: &[],
    fields: &[],
    variants: &[],
    names: &[],
    plans: &[],
}));

/// The table, by reference: a runtime call reads only the parts it uses.
pub(crate) fn table() -> &'static Table<'static> {
    let t = registered();
    assert!(!t.descs.is_empty(), "rt_init was not called");
    t
}

fn registered() -> &'static Table<'static> {
    // SAFETY: written only by `rt_init`, the first runtime call, which holds no reference to
    // it meanwhile.
    unsafe { &*TABLE.0.get() }
}

pub(crate) fn ptr(a: u32) -> *mut Header {
    core::ptr::with_exposed_provenance_mut(a as usize)
}

pub(crate) fn addr(p: *mut Header) -> u32 {
    p.expose_provenance() as u32
}

/// # Safety
/// `table` is the address of a `RawTable` whose arrays are valid, aligned, never written, and
/// keep `desc`'s invariants. A later call does nothing.
#[no_mangle]
pub unsafe extern "C" fn rt_init(table: u32) {
    // SAFETY: the caller's guarantee; Wasm memory starts at address 0. No reference to
    // `TABLE` is held across the write. The plans are leaked: they live as long as the table.
    unsafe {
        if !registered().descs.is_empty() {
            return;
        }
        let mut t = RawTable::read(0, table);
        t.plans = Box::leak(obj::plans(&t).into_boxed_slice());
        *TABLE.0.get() = t;
    }
}

/// A struct, variant `variant` of an enum, or a closure calling table entry `variant`, with
/// count 1 and every slot 0.
#[no_mangle]
pub extern "C" fn rt_alloc(desc: u32, variant: u32) -> u32 {
    addr(obj::alloc(table(), desc, variant))
}

/// The address of the free lists' heads (heap.rs), for generated code's inline allocation
/// (C8b5). Codegen reads it from this body, which must stay a single `i32.const`.
#[no_mangle]
pub extern "C" fn rt_heap_lists() -> u32 {
    crate::heap::lists().expose_provenance() as u32
}

/// The address of the live-object count (obj.rs), which generated code's inline allocation
/// adds to. Codegen reads it from this body, as `rt_heap_lists`'s.
#[no_mangle]
pub extern "C" fn rt_live_count() -> u32 {
    obj::live_count().expose_provenance() as u32
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
    unsafe { obj::dec(table(), ptr(p)) }
}

/// # Safety
/// `obj`'s contract, and `p` is not 0.
#[no_mangle]
pub unsafe extern "C" fn rt_unique(p: u32) -> u32 {
    // SAFETY: the caller's guarantee.
    unsafe { addr(obj::unique(table(), ptr(p))) }
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

/// `Map::new()` or `HashMap::new()`, by descriptor `desc`. Each `rt_map_` function takes
/// either kind.
#[no_mangle]
pub extern "C" fn rt_map_new(desc: u32) -> u32 {
    addr(coll::map_new(table(), desc))
}

/// `a == b` on values of descriptor `desc`.
#[no_mangle]
pub unsafe extern "C" fn rt_eq(desc: u32, a: i64, b: i64) -> bool {
    // SAFETY: the group's contract.
    unsafe { coll::eq(table(), desc, a as u64, b as u64) }
}

/// `a.cmp(b)` on two `Str`s by bytes, as Rust's `str`: -1, 0 or 1.
#[no_mangle]
pub unsafe extern "C" fn rt_str_cmp(a: u32, b: u32) -> i32 {
    // SAFETY: the group's contract.
    unsafe { text::bytes_cmp(obj::bytes(ptr(a)), obj::bytes(ptr(b))) as i32 }
}

/// `a + b` on `Str`.
#[no_mangle]
pub unsafe extern "C" fn rt_concat(a: u32, b: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::concat(ptr(a), ptr(b))) }
}

/// `a + b` on `Str` for `a = a + b`: takes `a`'s reference, borrows `b`, and returns the
/// result, which is `a` itself grown in place if that was its only reference.
#[no_mangle]
pub unsafe extern "C" fn rt_str_append(a: u32, b: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::append(table(), ptr(a), ptr(b))) }
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
    unsafe { addr(text::parse_int(table(), desc, ptr(none), ptr(s))) }
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
    unsafe { addr(text::strip_prefix(table(), desc, ptr(none), ptr(s), ptr(p))) }
}

/// `desc` is `Option<Str>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_strip_suffix(desc: u32, none: u32, s: u32, p: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::strip_suffix(table(), desc, ptr(none), ptr(s), ptr(p))) }
}

/// `desc` is `Option<Int>`; `none` is its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_str_find(desc: u32, none: u32, s: u32, sub: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::find(table(), desc, ptr(none), ptr(s), ptr(sub))) }
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
    unsafe { addr(text::parse_float(table(), desc, ptr(none), ptr(s))) }
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
    unsafe { addr(float::to_int(table(), desc, ptr(none), x)) }
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
    match unsafe { coll::list_get(table(), ptr(l), i) } {
        Ok(v) => v as i64,
        Err(e) => index_trap(site, e),
    }
}

#[no_mangle]
pub unsafe extern "C" fn rt_map_len(m: u32) -> i64 {
    // SAFETY: the group's contract.
    unsafe { coll::map_len(table(), ptr(m)) }
}

/// `desc` is `List<K>`.
#[no_mangle]
pub unsafe extern "C" fn rt_map_keys(desc: u32, m: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::map_keys(table(), ptr(m), desc)) }
}

/// `desc` is `Option<V>`, `none` its pinned `None`; `k` is an `Int` or a `Str` pointer.
#[no_mangle]
pub unsafe extern "C" fn rt_map_get(desc: u32, none: u32, m: u32, k: i64) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::map_get(table(), ptr(m), k as u64, desc, ptr(none))) }
}

/// `l.contains(v)`; `v` is a slot value of the element type.
#[no_mangle]
pub unsafe extern "C" fn rt_list_contains(l: u32, v: i64) -> bool {
    // SAFETY: the group's contract.
    unsafe { coll::list_contains(table(), ptr(l), v as u64) }
}

/// `r.ok()`: `desc` is `Option<T>`, `none` its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_result_ok(desc: u32, none: u32, r: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::result_variant(table(), ptr(r), 0, desc, ptr(none))) }
}

/// `r.err()`: `desc` is `Option<E>`, `none` its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_result_err(desc: u32, none: u32, r: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::result_variant(table(), ptr(r), 1, desc, ptr(none))) }
}

/// `o.ok_or(e)`: `desc` is `Result<T, E>`; `e` is a slot value of `E`.
#[no_mangle]
pub unsafe extern "C" fn rt_option_ok_or(desc: u32, o: u32, e: i64) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::option_ok_or(table(), ptr(o), e as u64, desc)) }
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

// Built-in JSON (runtime/src/json.rs).

/// `s.parse_json()`: `desc` is `Result<Json, Str>`.
#[no_mangle]
pub unsafe extern "C" fn rt_json_parse(desc: u32, s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(json::parse(table(), desc, ptr(s))) }
}

/// `j.to_json()`.
#[no_mangle]
pub unsafe extern "C" fn rt_json_encode(j: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(json::encode(ptr(j))) }
}

/// `j.get(k)`: `desc` is `Option<Json>`, `none` its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_json_get(desc: u32, none: u32, j: u32, k: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(json::get(table(), desc, ptr(none), ptr(j), ptr(k))) }
}

/// `j.at(i)`: `desc` is `Option<Json>`, `none` its pinned `None`.
#[no_mangle]
pub unsafe extern "C" fn rt_json_at(desc: u32, none: u32, j: u32, i: i64) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(json::at(table(), desc, ptr(none), ptr(j), i)) }
}

/// The `as_*` accessors: `desc` is `Option` of the payload type, `none` its pinned `None`.
macro_rules! json_as {
    ($($name:ident: $variant:ident),*) => {$(
        #[no_mangle]
        pub unsafe extern "C" fn $name(desc: u32, none: u32, j: u32) -> u32 {
            // SAFETY: the group's contract.
            unsafe { addr(json::as_variant(table(), desc, ptr(none), ptr(j), json::$variant)) }
        }
    )*};
}

json_as!(
    rt_json_as_str: STR,
    rt_json_as_int: NUM,
    rt_json_as_float: FLOAT,
    rt_json_as_bool: BOOL,
    rt_json_as_list: ARR,
    rt_json_as_map: OBJ
);

// Mutating methods and place writes. The container is the place's own object, which the caller
// has made unique (`rt_unique`) and stored back; it is changed in place. A value argument is
// consumed: the container owns it, except when the call traps. A key is borrowed.

/// `l[i] = v`. Error: out of bounds; `a` is `i`, `b` the length.
#[no_mangle]
pub unsafe extern "C" fn rt_list_set(l: u32, i: i64, v: i64, site: u32) {
    // SAFETY: the group's contract.
    if let Err(e) = unsafe { coll::list_set(table(), ptr(l), i, v as u64) } {
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
    unsafe { coll::list_sort(table(), ptr(l)) }
}

/// `l.pop()`: `desc` is `Option<T>`, `none` its pinned `None`. The element moves into the
/// `Some`.
#[no_mangle]
pub unsafe extern "C" fn rt_list_pop(desc: u32, none: u32, l: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::list_pop(table(), ptr(l), desc, ptr(none))) }
}

/// `b.push(n)`: returns the `Bytes` to store back in the place, `b` or a larger copy (`b` then
/// freed). Error: `n` outside 0..=255; `a` is `n`, `b` 0.
#[no_mangle]
pub unsafe extern "C" fn rt_bytes_push(b: u32, n: i64, site: u32) -> u32 {
    // SAFETY: the group's contract.
    match unsafe { text::push(table(), ptr(b), n) } {
        Ok(p) => addr(p),
        Err(_) => rt_trap(site, n, 0),
    }
}

/// `m.insert(k, v)`.
#[no_mangle]
pub unsafe extern "C" fn rt_map_insert(m: u32, k: i64, v: i64) {
    // SAFETY: the group's contract.
    unsafe { coll::map_insert(table(), ptr(m), k as u64, v as u64) }
}

/// `m.remove(k)`.
#[no_mangle]
pub unsafe extern "C" fn rt_map_remove(m: u32, k: i64) {
    // SAFETY: the group's contract.
    unsafe { coll::map_remove(table(), ptr(m), k as u64) }
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
            obj::payload(ptr(p))
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

// L11's built-ins (C14). Non-mutating ones keep the group's contract above; `reverse`,
// `extend`, `sort_by_key` and the sort's finish change a unique place, as the mutators do.

/// `l.slice(a, b)`, a new list of `l`'s descriptor. Error: out of bounds, with `a`, `b` and the
/// length as the message's third value (`trap3`).
#[no_mangle]
pub unsafe extern "C" fn rt_list_slice(l: u32, a: i64, b: i64, site: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe {
        match coll::list_slice(table(), ptr(l), a, b) {
            Some(p) => addr(p),
            None => trap3(site, a, b, coll::list_len(ptr(l))),
        }
    }
}

/// `l.join(sep)` on a `List<Str>`.
#[no_mangle]
pub unsafe extern "C" fn rt_list_join(l: u32, sep: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(coll::list_join(ptr(l), ptr(sep))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_to_lowercase(s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::to_lowercase(ptr(s))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_str_to_uppercase(s: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::to_uppercase(ptr(s))) }
}

/// `sha256()` of a `Str` or `Bytes`.
#[no_mangle]
pub unsafe extern "C" fn rt_sha256(p: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::sha256(ptr(p))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_bytes_to_hex(b: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::to_hex(ptr(b))) }
}

/// `ct_eq` of two `Str`s or two `Bytes`.
#[no_mangle]
pub unsafe extern "C" fn rt_ct_eq(a: u32, b: u32) -> bool {
    // SAFETY: the group's contract.
    unsafe { text::ct_eq(ptr(a), ptr(b)) }
}

/// `l.reverse()`.
#[no_mangle]
pub unsafe extern "C" fn rt_list_reverse(l: u32) {
    // SAFETY: the group's contract.
    unsafe { coll::list_reverse(ptr(l)) }
}

/// `b.extend(more)`: returns the `Bytes` to store back in the place, as `rt_bytes_push`.
/// `more` is borrowed, and is not `b` (which is unique).
#[no_mangle]
pub unsafe extern "C" fn rt_bytes_extend(b: u32, more: u32) -> u32 {
    // SAFETY: the group's contract.
    unsafe { addr(text::extend(table(), ptr(b), ptr(more))) }
}

/// `l.sort_by_key(f)`, given `keys`, `f` of each element in order (a `List<Int>` or
/// `List<Str>`, borrowed).
#[no_mangle]
pub unsafe extern "C" fn rt_list_sort_by_key(l: u32, keys: u32) {
    // SAFETY: the group's contract.
    unsafe { coll::list_sort_by_key(table(), ptr(l), ptr(keys)) }
}

// `l.sort_by(less)`: generated code drives lang/src/sort.rs's merge sort, calling `less`
// itself between the steps, so the runtime never calls into the program. The sort is a plain
// allocation, not an object, owned by generated code from `rt_sort_new` to `rt_sort_finish`.

fn sort_state(s: u32) -> *mut crate::sort::MergeSort {
    core::ptr::with_exposed_provenance_mut(s as usize)
}

/// A sort of the indexes of list `l`.
#[no_mangle]
pub unsafe extern "C" fn rt_sort_new(l: u32) -> u32 {
    // SAFETY: the group's contract.
    let n = unsafe { coll::list_len(ptr(l)) } as usize;
    let s = alloc::boxed::Box::new(crate::sort::MergeSort::new(n));
    alloc::boxed::Box::into_raw(s).expose_provenance() as u32
}

/// The sort's next question, does element `a` go before element `b`, as `a << 32 | b`; -1
/// once sorted.
///
/// # Safety
/// `s` is a live sort from `rt_sort_new`, its last question answered.
#[no_mangle]
pub unsafe extern "C" fn rt_sort_next(s: u32) -> i64 {
    // SAFETY: the caller's guarantee.
    match unsafe { &mut *sort_state(s) }.question() {
        Some((a, b)) => ((a as i64) << 32) | b as i64,
        None => -1,
    }
}

/// The answer to the sort's last question: `less(a, b)`, 0 or 1.
///
/// # Safety
/// `s` is a live sort from `rt_sort_new` with a question pending.
#[no_mangle]
pub unsafe extern "C" fn rt_sort_answer(s: u32, less: u32) {
    // SAFETY: the caller's guarantee.
    unsafe { &mut *sort_state(s) }.answer(less != 0)
}

/// Puts the unique list `l` in the sorted order and frees the sort.
///
/// # Safety
/// `s` is a sort from `rt_sort_new` of `l`, done (`rt_sort_next` gave -1), freed here once.
#[no_mangle]
pub unsafe extern "C" fn rt_sort_finish(s: u32, l: u32) {
    // SAFETY: the caller's guarantee; `s` came from `Box::into_raw`.
    unsafe {
        let s = alloc::boxed::Box::from_raw(sort_state(s));
        coll::list_permute(ptr(l), &s.order());
    }
}
