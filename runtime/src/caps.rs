//! Capability calls: one `rt_cap_<capability>_<method>` per caps.wit method, over the
//! `compiled` world's imports (docs/codegen.md, "Host calls"). Hand-written; codegen's
//! `every_capability_method_compiles` test fails if one is missing or has the wrong signature.
//!
//! Each takes the descriptor of its result type first (ignored for `Str`, `Bytes` and `Int`),
//! then the capability's handle, then its arguments, which it borrows, and returns a new value
//! (+1), exactly the interpreter's (guest/src/lib.rs). A `Str` or `Bytes` argument is passed to
//! the host as a pointer into its object; a `string` or `list<u8>` result is written by the host
//! into memory `cabi_realloc` reserved with room for an object header, and becomes a `Str` or
//! `Bytes` in place. Error enums are found by variant name in the descriptor table, as the
//! guest finds them in the `CapTable`.
//!
//! A handle is the component's own: the canonical ABI checks on every call that it is live and
//! of the method's resource type, and the host checks it is a grant (host/src/lib.rs).

use crate::abi::{addr, ptr, table};
use crate::desc::{self, Desc, Table};
use crate::entry::kio::caps::clock::Clock;
use crate::entry::kio::caps::db::{Db, DbErr, SqlValue};
use crate::entry::kio::caps::fs::{Dir, IoErr};
use crate::entry::kio::caps::kv::{Kv, KvEntry, KvErr};
use crate::entry::kio::caps::log::{Log, LogErr};
use crate::entry::kio::caps::net::{Http, HttpErr, HttpRequest, HttpResponse};
use crate::entry::kio::caps::rng::Rng;
use crate::entry::kio::caps::web::Incoming;
use crate::entry::kio::caps::ws::{WsErr, WsMessage, WsSend};
use crate::obj::{self, to_slot, Header};
use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// Set while a capability call runs: the host's byte-aligned allocations (its `string` and
/// `list<u8>` results) then get room for a header in front (`obj::bytes_room`).
static IN_PLACE: AtomicBool = AtomicBool::new(false);

/// The canonical ABI's allocator, as wit-bindgen's (whose weak definition this replaces), except
/// during a capability call.
///
/// # Safety
/// The canonical ABI's contract for `cabi_realloc`.
#[no_mangle]
pub unsafe extern "C" fn cabi_realloc(old: *mut u8, old_len: usize, align: usize, new_len: usize) -> *mut u8 {
    if align == 1 && IN_PLACE.load(Relaxed) {
        // Wasmtime lowers a UTF-8 string or a `list<u8>` with one fresh allocation.
        assert!(old_len == 0, "cabi_realloc: a string grew");
        return obj::bytes_room(new_len);
    }
    // SAFETY: the canonical ABI passes a valid alignment and, for a resize, a live block.
    unsafe {
        let p = if old_len == 0 {
            if new_len == 0 {
                return align as *mut u8;
            }
            alloc::alloc::alloc(Layout::from_size_align_unchecked(new_len, align))
        } else {
            alloc::alloc::realloc(old, Layout::from_size_align_unchecked(old_len, align), new_len)
        };
        assert!(!p.is_null(), "out of memory");
        p
    }
}

/// Runs one capability call with the host's strings landing in place.
fn in_place<T>(call: impl FnOnce() -> T) -> T {
    IN_PLACE.store(true, Relaxed);
    let r = call();
    IN_PLACE.store(false, Relaxed);
    r
}

/// The `Str` or `Bytes` a `string` or `list<u8>` result became, without copying.
fn adopt(desc: u32, v: Vec<u8>) -> *mut Header {
    let mut v = ManuallyDrop::new(v);
    assert!(v.len() == v.capacity());
    // SAFETY: the host wrote it into memory from `cabi_realloc` during `in_place`, so from
    // `bytes_room(len)`; `v` is forgotten, so it is adopted once.
    unsafe { obj::adopt(desc, v.as_mut_ptr(), v.len()) }
}

fn str_obj(s: String) -> *mut Header {
    adopt(desc::STR, s.into_bytes())
}

fn bytes_obj(b: Vec<u8>) -> *mut Header {
    adopt(desc::BYTES, b)
}

/// A `List<Str>` of descriptor `list`.
fn strs(list: u32, v: Vec<String>) -> *mut Header {
    obj::new_list(list, v.into_iter().map(|s| to_slot(str_obj(s))).collect())
}

/// The contents of a `Str` or `Bytes` argument, in place.
///
/// # Safety
/// `p` is a live `Str` or `Bytes` that outlives the call.
unsafe fn bytes<'a>(p: u32) -> &'a [u8] {
    // SAFETY: the caller's guarantee.
    unsafe { obj::bytes(ptr(p)) }
}

/// # Safety
/// `p` is a live `Str` that outlives the call.
unsafe fn text<'a>(p: u32) -> &'a str {
    // SAFETY: the caller's guarantee; a `Str` is UTF-8.
    unsafe { core::str::from_utf8_unchecked(bytes(p)) }
}

/// The payload descriptor of variant `v` of enum descriptor `e` (one slot).
fn payload(t: &Table, e: u32, v: u32) -> u32 {
    match t.desc(e) {
        Desc::Enum { variants, .. } => t.slots(t.variant(variants, v).slots)[0],
        d => panic!("{d:?} is not an enum"),
    }
}

/// `Ok(v)` of `Result` descriptor `ret`, taking `v`'s reference.
fn ok(t: &Table, ret: u32, v: u64) -> u32 {
    let p = obj::alloc(t, ret, 0);
    // SAFETY: `Ok` has one slot.
    unsafe { *obj::slot(t, p, 0) = v };
    addr(p)
}

/// `Err(E::case)` of `Result` descriptor `ret`, with an `Int` payload if `n`.
fn err(t: &Table, ret: u32, case: &str, n: Option<i64>) -> u32 {
    let e = payload(t, ret, 1);
    let Desc::Enum { variants, .. } = t.desc(e) else {
        panic!("not an error enum")
    };
    let v = (0..variants.len)
        .find(|&i| t.name(t.variant(variants, i).name) == case.as_bytes())
        .expect("a caps.wit error variant");
    let p = obj::alloc(t, e, v);
    if let Some(n) = n {
        // SAFETY: a variant with a payload has one `Int` slot.
        unsafe { *obj::slot(t, p, 0) = n as u64 };
    }
    let r = obj::alloc(t, ret, 1);
    // SAFETY: `Err` has one slot; it takes `p`'s reference.
    unsafe { *obj::slot(t, r, 0) = to_slot(p) };
    addr(r)
}

/// A `Result` of descriptor `ret`: `Ok` of `f`'s slot, or `Err` of the case `name` gives.
fn result<T, E>(
    ret: u32,
    r: Result<T, E>,
    f: impl FnOnce(&Table, T) -> u64,
    name: impl FnOnce(E) -> &'static str,
) -> u32 {
    let t = table();
    match r {
        Ok(v) => {
            let v = f(t, v);
            ok(t, ret, v)
        }
        Err(e) => err(t, ret, name(e), None),
    }
}

fn unit<T>(_: &Table, _: T) -> u64 {
    0
}

fn io_err(e: IoErr) -> &'static str {
    match e {
        IoErr::NotFound => "NotFound",
        IoErr::Denied => "Denied",
        IoErr::TooLarge => "TooLarge",
        IoErr::InvalidPath => "InvalidPath",
        IoErr::Exists => "Exists",
    }
}

fn kv_err(e: KvErr) -> &'static str {
    match e {
        KvErr::TooLarge => "TooLarge",
        KvErr::Full => "Full",
        KvErr::NotInt => "NotInt",
        KvErr::Overflow => "Overflow",
    }
}

fn ws_err(e: WsErr) -> &'static str {
    match e {
        WsErr::Closed => "Closed",
        WsErr::TooLarge => "TooLarge",
        WsErr::Invalid => "Invalid",
    }
}

/// The resource behind handle `h`, not dropped when done: the grant still owns it.
///
/// # Safety
/// None for memory: a wrong handle makes the call trap in the canonical ABI.
unsafe fn handle<T>(h: u32, from: unsafe fn(u32) -> T) -> ManuallyDrop<T> {
    // SAFETY: as above.
    ManuallyDrop::new(unsafe { from(h) })
}

// # Safety (every function below)
// `obj`'s contract, and each pointer argument is of the type its method's signature says.

#[no_mangle]
pub unsafe extern "C" fn rt_cap_dir_read(ret: u32, h: u32, path: u32) -> u32 {
    // SAFETY: the contract above.
    let (d, path) = unsafe { (handle(h, Dir::from_handle), text(path)) };
    let r = in_place(|| d.read(path));
    result(ret, r, |_, b| to_slot(bytes_obj(b)), io_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_dir_create(ret: u32, h: u32, path: u32, data: u32) -> u32 {
    // SAFETY: the contract above.
    let (d, path, data) = unsafe { (handle(h, Dir::from_handle), text(path), bytes(data)) };
    result(ret, in_place(|| d.create(path, data)), unit, io_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_dir_list(ret: u32, h: u32, path: u32) -> u32 {
    // SAFETY: the contract above.
    let (d, path) = unsafe { (handle(h, Dir::from_handle), text(path)) };
    let r = in_place(|| d.list(path));
    result(ret, r, |t, v| to_slot(strs(payload(t, ret, 0), v)), io_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_dir_write(ret: u32, h: u32, path: u32, data: u32) -> u32 {
    // SAFETY: the contract above.
    let (d, path, data) = unsafe { (handle(h, Dir::from_handle), text(path), bytes(data)) };
    result(ret, in_place(|| d.write(path, data)), unit, io_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_dir_remove(ret: u32, h: u32, path: u32) -> u32 {
    // SAFETY: the contract above.
    let (d, path) = unsafe { (handle(h, Dir::from_handle), text(path)) };
    result(ret, in_place(|| d.remove(path)), unit, io_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_dir_exists(ret: u32, h: u32, path: u32) -> u32 {
    // SAFETY: the contract above.
    let (d, path) = unsafe { (handle(h, Dir::from_handle), text(path)) };
    result(ret, in_place(|| d.exists(path)), |_, b| b as u64, io_err)
}

/// `Err(e)` of `Result` descriptor `ret`.
fn http_err(t: &Table, ret: u32, e: HttpErr) -> u32 {
    match e {
        HttpErr::Denied => err(t, ret, "Denied", None),
        HttpErr::Timeout => err(t, ret, "Timeout", None),
        HttpErr::TooLarge => err(t, ret, "TooLarge", None),
        HttpErr::Status(n) => err(t, ret, "Status", Some(n.into())),
        HttpErr::Invalid => err(t, ret, "Invalid", None),
    }
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_http_get(ret: u32, h: u32, path: u32) -> u32 {
    // SAFETY: the contract above.
    let (c, path) = unsafe { (handle(h, Http::from_handle), text(path)) };
    let t = table();
    match in_place(|| c.get(path)) {
        Ok(b) => ok(t, ret, to_slot(bytes_obj(b))),
        Err(e) => http_err(t, ret, e),
    }
}

/// The text of the `Str` in slot value `s`.
///
/// # Safety
/// `s` holds a live `Str` that outlives `'a`.
unsafe fn slot_text<'a>(s: u64) -> &'a str {
    // SAFETY: the caller's guarantee; a `Str` is UTF-8.
    unsafe { core::str::from_utf8_unchecked(obj::bytes(obj::from_slot(s))) }
}

/// A `List<Str>` argument's strings, copied (wit-bindgen's binding takes `&[String]`).
///
/// # Safety
/// `l` is a live `List<Str>` that outlives the call.
unsafe fn strings(l: u32) -> Vec<String> {
    // SAFETY: the caller's guarantee; each element is a live `Str`.
    unsafe { obj::elems(ptr(l)).iter().map(|&s| String::from(slot_text(s))).collect() }
}

/// A `Result<HttpResponse, HttpErr>` of descriptor `ret` as a slot value: the record's slots are
/// the status, the headers (a `List<Str>` of the record's second slot type) and the body.
fn http_result(t: &Table, ret: u32, r: Result<HttpResponse, HttpErr>) -> u64 {
    let r = match r {
        Ok(r) => r,
        Err(e) => return to_slot(ptr(http_err(t, ret, e))),
    };
    let rec = payload(t, ret, 0);
    let Desc::Record { slots, .. } = t.desc(rec) else {
        panic!("not a record")
    };
    let headers = t.slots(slots)[1];
    let p = obj::alloc(t, rec, 0);
    // SAFETY: `HttpResponse` has three slots; the headers and body take the objects' references.
    unsafe {
        *obj::slot(t, p, 0) = u64::from(r.status);
        *obj::slot(t, p, 1) = to_slot(strs(headers, r.headers));
        *obj::slot(t, p, 2) = to_slot(bytes_obj(r.body));
    }
    to_slot(ptr(ok(t, ret, to_slot(p))))
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_http_send(ret: u32, h: u32, method: u32, path: u32, headers: u32, body: u32) -> u32 {
    // SAFETY: the contract above.
    let (c, method, path, headers, body) = unsafe {
        (
            handle(h, Http::from_handle),
            text(method),
            text(path),
            strings(headers),
            bytes(body),
        )
    };
    let r = in_place(|| c.send(method, path, &headers, body));
    addr(obj::from_slot(http_result(table(), ret, r)))
}

/// An `HttpRequest` argument's fields, copied. A `timeout_ms` outside `u32` is passed as
/// `u32::MAX`, which the host caps at the grant's timeout, as it does any value past it.
///
/// # Safety
/// `p` is a live `HttpRequest` that outlives the call.
unsafe fn request(t: &Table, p: *mut Header) -> HttpRequest {
    // SAFETY: the caller's guarantee; the slots are a `Str`, a `Str`, a `List<Str>`, a `Bytes`
    // and an `Int`, in field order.
    unsafe {
        HttpRequest {
            method: String::from(slot_text(*obj::slot(t, p, 0))),
            path: String::from(slot_text(*obj::slot(t, p, 1))),
            headers: strings(addr(obj::from_slot(*obj::slot(t, p, 2)))),
            body: obj::bytes(obj::from_slot(*obj::slot(t, p, 3))).to_vec(),
            timeout_ms: u32::try_from(*obj::slot(t, p, 4) as i64).unwrap_or(u32::MAX),
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_http_send_all(ret: u32, h: u32, reqs: u32) -> u32 {
    let t = table();
    // SAFETY: the contract above; `reqs` is a `List<HttpRequest>`.
    let (c, reqs) = unsafe {
        let reqs: Vec<HttpRequest> = obj::elems(ptr(reqs))
            .iter()
            .map(|&r| request(t, obj::from_slot(r)))
            .collect();
        (handle(h, Http::from_handle), reqs)
    };
    let rs = in_place(|| c.send_all(&reqs));
    let Desc::List { elem } = t.desc(ret) else {
        panic!("not a list")
    };
    addr(obj::new_list(
        ret,
        rs.into_iter().map(|r| http_result(t, elem, r)).collect(),
    ))
}

fn incoming(h: u32) -> ManuallyDrop<Incoming> {
    // SAFETY: see `handle`.
    unsafe { handle(h, Incoming::from_handle) }
}

#[no_mangle]
pub extern "C" fn rt_cap_incoming_method(_: u32, h: u32) -> u32 {
    let r = incoming(h);
    addr(str_obj(in_place(|| r.method())))
}

#[no_mangle]
pub extern "C" fn rt_cap_incoming_target(_: u32, h: u32) -> u32 {
    let r = incoming(h);
    addr(str_obj(in_place(|| r.target())))
}

#[no_mangle]
pub extern "C" fn rt_cap_incoming_header_names(ret: u32, h: u32) -> u32 {
    let r = incoming(h);
    addr(strs(ret, in_place(|| r.header_names())))
}

#[no_mangle]
pub extern "C" fn rt_cap_incoming_header_values(ret: u32, h: u32) -> u32 {
    let r = incoming(h);
    addr(strs(ret, in_place(|| r.header_values())))
}

#[no_mangle]
pub extern "C" fn rt_cap_incoming_body(_: u32, h: u32) -> u32 {
    let r = incoming(h);
    addr(bytes_obj(in_place(|| r.body())))
}

// `send` is imported directly, so its two `list<string>` arguments are lists of pointers into
// their `Str` objects rather than copies (wit-bindgen's binding takes `&[String]`). The
// signature is the canonical ABI's lowering of caps.wit's `send`; wit-component checks it.
#[link(wasm_import_module = "kio:caps/web")]
extern "C" {
    #[link_name = "[method]outgoing.send"]
    fn outgoing_send(
        h: i32,
        status: i32,
        names: *mut u8,
        names_len: usize,
        values: *mut u8,
        values_len: usize,
        body: *mut u8,
        body_len: usize,
        ret: *mut u8,
    );
}

/// A `List<Str>` as a canonical `list<string>`: a `(pointer, length)` pair per element.
///
/// # Safety
/// `l` is a live `List<Str>` that outlives the call.
unsafe fn string_list(l: u32) -> Vec<[u32; 2]> {
    // SAFETY: the caller's guarantee; each element is a live `Str`.
    unsafe {
        let list = obj::elems(ptr(l));
        list.iter()
            .map(|&s| {
                let b = obj::bytes(obj::from_slot(s));
                [b.as_ptr() as u32, b.len() as u32]
            })
            .collect()
    }
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_outgoing_send(
    ret: u32,
    h: u32,
    status: i64,
    names: u32,
    values: u32,
    body: u32,
) -> u32 {
    let t = table();
    // A status that is not a `u16` cannot be valid, so it never reaches the host.
    let Ok(status) = u16::try_from(status) else {
        return err(t, ret, "Invalid", None);
    };
    // SAFETY: the contract above.
    let (mut names, mut values, body) = unsafe { (string_list(names), string_list(values), bytes(body)) };
    // The canonical ABI's result<_, web-err>: the discriminant, then the error's.
    let mut out = [0u8; 2];
    // SAFETY: every pointer and length names memory that lives through the call; `out` has
    // room for the result.
    unsafe {
        outgoing_send(
            h as i32,
            status.into(),
            names.as_mut_ptr().cast(),
            names.len(),
            values.as_mut_ptr().cast(),
            values.len(),
            body.as_ptr().cast_mut(),
            body.len(),
            out.as_mut_ptr(),
        )
    };
    match out {
        [0, _] => ok(t, ret, 0),
        // caps.wit: `variant web-err { already-sent, invalid }`.
        [1, 0] => err(t, ret, "AlreadySent", None),
        [1, 1] => err(t, ret, "Invalid", None),
        _ => panic!("send: a malformed result"),
    }
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_kv_get(ret: u32, h: u32, key: u32) -> u32 {
    // SAFETY: the contract above.
    let (k, key) = unsafe { (handle(h, Kv::from_handle), text(key)) };
    let r = in_place(|| k.get(key));
    let some = |t: &Table, v: Option<Vec<u8>>| {
        let v = v.map(|b| to_slot(bytes_obj(b)));
        // SAFETY: `Ok`'s payload is an `Option<Bytes>`; a null `none` allocates one.
        to_slot(unsafe { obj::option(t, payload(t, ret, 0), core::ptr::null_mut(), v) })
    };
    result(ret, r, some, kv_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_kv_set(ret: u32, h: u32, key: u32, value: u32) -> u32 {
    // SAFETY: the contract above.
    let (k, key, value) = unsafe { (handle(h, Kv::from_handle), text(key), bytes(value)) };
    result(ret, in_place(|| k.set(key, value)), unit, kv_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_kv_delete(ret: u32, h: u32, key: u32) -> u32 {
    // SAFETY: the contract above.
    let (k, key) = unsafe { (handle(h, Kv::from_handle), text(key)) };
    result(ret, in_place(|| k.delete(key)), unit, kv_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_kv_keys(ret: u32, h: u32, prefix: u32) -> u32 {
    // SAFETY: the contract above.
    let (k, prefix) = unsafe { (handle(h, Kv::from_handle), text(prefix)) };
    let r = in_place(|| k.keys(prefix));
    result(ret, r, |t, v| to_slot(strs(payload(t, ret, 0), v)), kv_err)
}

/// An `Option<Bytes>` argument's contents.
///
/// # Safety
/// `o` is a live `Option<Bytes>` that outlives the call.
unsafe fn bytes_option<'a>(t: &Table, o: u32) -> Option<&'a [u8]> {
    let p = ptr(o);
    // SAFETY: the caller's guarantee: variant 1 (`Some`) has one `Bytes` slot.
    unsafe { (*obj::field::<u32>(p, 8) == 1).then(|| obj::bytes(obj::from_slot(*obj::slot(t, p, 0)))) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_kv_swap(ret: u32, h: u32, key: u32, old: u32, new: u32) -> u32 {
    let t = table();
    // SAFETY: the contract above.
    let (k, key, old, new) = unsafe {
        (
            handle(h, Kv::from_handle),
            text(key),
            bytes_option(t, old),
            bytes_option(t, new),
        )
    };
    result(ret, in_place(|| k.swap(key, old, new)), |_, b| b as u64, kv_err)
}

/// An `Option<Str>` argument's contents.
///
/// # Safety
/// `o` is a live `Option<Str>` that outlives the call.
unsafe fn str_option<'a>(t: &Table, o: u32) -> Option<&'a str> {
    // SAFETY: the caller's guarantee; a `Str` is UTF-8.
    unsafe { bytes_option(t, o).map(|b| core::str::from_utf8_unchecked(b)) }
}

/// A `List<KvEntry>` of descriptor `list`: each entry a record of its `Str` key and `Bytes` value.
fn entries(t: &Table, list: u32, v: Vec<KvEntry>) -> *mut Header {
    let Desc::List { elem } = t.desc(list) else {
        panic!("not a list")
    };
    let entry = |e: KvEntry| {
        let p = obj::alloc(t, elem, 0);
        // SAFETY: `KvEntry` has two slots, key then value, which take the objects' references.
        unsafe {
            *obj::slot(t, p, 0) = to_slot(str_obj(e.key));
            *obj::slot(t, p, 1) = to_slot(bytes_obj(e.value));
        }
        to_slot(p)
    };
    obj::new_list(list, v.into_iter().map(entry).collect())
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_kv_scan(ret: u32, h: u32, prefix: u32, after: u32, limit: i64) -> u32 {
    let t = table();
    // A limit that is not a `u32` is outside 1 to 1,000, so it never reaches the host.
    let Ok(limit) = u32::try_from(limit) else {
        return err(t, ret, "TooLarge", None);
    };
    // SAFETY: the contract above.
    let (k, prefix, after) = unsafe { (handle(h, Kv::from_handle), text(prefix), str_option(t, after)) };
    let r = in_place(|| k.scan(prefix, after, limit));
    result(ret, r, |t, v| to_slot(entries(t, payload(t, ret, 0), v)), kv_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_kv_add(ret: u32, h: u32, key: u32, n: i64) -> u32 {
    // SAFETY: the contract above.
    let (k, key) = unsafe { (handle(h, Kv::from_handle), text(key)) };
    result(ret, in_place(|| k.add(key, n)), |_, v| v as u64, kv_err)
}

fn message(h: u32) -> ManuallyDrop<WsMessage> {
    // SAFETY: see `handle`.
    unsafe { handle(h, WsMessage::from_handle) }
}

#[no_mangle]
pub extern "C" fn rt_cap_ws_message_kind(_: u32, h: u32) -> u32 {
    let m = message(h);
    addr(str_obj(in_place(|| m.kind())))
}

/// Ids count up from 1, so they fit an `Int`.
#[no_mangle]
pub extern "C" fn rt_cap_ws_message_conn(_: u32, h: u32) -> i64 {
    message(h).conn() as i64
}

#[no_mangle]
pub extern "C" fn rt_cap_ws_message_data(_: u32, h: u32) -> u32 {
    let m = message(h);
    addr(bytes_obj(in_place(|| m.data())))
}

#[no_mangle]
pub extern "C" fn rt_cap_ws_message_target(_: u32, h: u32) -> u32 {
    let m = message(h);
    addr(str_obj(in_place(|| m.target())))
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_ws_message_header(ret: u32, h: u32, name: u32) -> u32 {
    // SAFETY: the contract above.
    let name = unsafe { text(name) };
    let m = message(h);
    let v = in_place(|| m.header(name)).map(|s| to_slot(str_obj(s)));
    // SAFETY: `ret` is an `Option<Str>`; a null `none` allocates one.
    addr(unsafe { obj::option(table(), ret, core::ptr::null_mut(), v) })
}

fn sender(h: u32) -> ManuallyDrop<WsSend> {
    // SAFETY: see `handle`.
    unsafe { handle(h, WsSend::from_handle) }
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_ws_send_text(ret: u32, h: u32, s: u32) -> u32 {
    // SAFETY: the contract above.
    let s = unsafe { text(s) };
    let w = sender(h);
    result(ret, in_place(|| w.text(s)), unit, ws_err)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_ws_send_binary(ret: u32, h: u32, b: u32) -> u32 {
    // SAFETY: the contract above.
    let b = unsafe { bytes(b) };
    let w = sender(h);
    result(ret, in_place(|| w.binary(b)), unit, ws_err)
}

#[no_mangle]
pub extern "C" fn rt_cap_ws_send_close(ret: u32, h: u32, code: i64) -> u32 {
    // A code that is not a `u16` cannot be valid, so it never reaches the host.
    let Ok(code) = u16::try_from(code) else {
        return err(table(), ret, "Invalid", None);
    };
    let w = sender(h);
    result(ret, in_place(|| w.close(code)), unit, ws_err)
}

/// Codegen has trapped on an `n` outside 0 to 65,536 (`rng_check`); one that still arrives is
/// passed on as `u32::MAX`, which the host refuses.
#[no_mangle]
pub extern "C" fn rt_cap_rng_bytes(_: u32, h: u32, n: i64) -> u32 {
    // SAFETY: see `handle`.
    let r = unsafe { handle(h, Rng::from_handle) };
    let n = u32::try_from(n).unwrap_or(u32::MAX);
    addr(bytes_obj(in_place(|| r.bytes(n))))
}

#[no_mangle]
pub extern "C" fn rt_cap_rng_int(_: u32, h: u32, lo: i64, hi: i64) -> i64 {
    // SAFETY: see `handle`.
    let r = unsafe { handle(h, Rng::from_handle) };
    r.int(lo, hi)
}

#[no_mangle]
pub extern "C" fn rt_cap_clock_now_ms(_: u32, h: u32) -> i64 {
    // SAFETY: see `handle`.
    let c = unsafe { handle(h, Clock::from_handle) };
    c.now_ms()
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_log_write(ret: u32, h: u32, line: u32) -> u32 {
    // SAFETY: the contract above.
    let (l, line) = unsafe { (handle(h, Log::from_handle), text(line)) };
    result(ret, in_place(|| l.write(line)), unit, |LogErr::TooLarge| "TooLarge")
}

fn database(h: u32) -> ManuallyDrop<Db> {
    // SAFETY: see `handle`.
    unsafe { handle(h, Db::from_handle) }
}

/// A `Result<_, DbErr>` of descriptor `ret`: `Ok` of `f`'s slot, or the error, whose `Invalid`
/// and `Constraint` carry SQLite's message as a `Str`.
fn db_result<T>(ret: u32, r: Result<T, DbErr>, f: impl FnOnce(&Table, T) -> u64) -> u32 {
    let t = table();
    let (case, msg) = match r {
        Ok(v) => {
            let v = f(t, v);
            return ok(t, ret, v);
        }
        Err(DbErr::Invalid(m)) => ("Invalid", Some(m)),
        Err(DbErr::Denied) => ("Denied", None),
        Err(DbErr::Busy) => ("Busy", None),
        Err(DbErr::TooLarge) => ("TooLarge", None),
        Err(DbErr::Constraint(m)) => ("Constraint", Some(m)),
    };
    // `err` writes its payload as slot bits: a `Str`'s is its pointer's slot.
    err(t, ret, case, msg.map(|m| to_slot(str_obj(m)) as i64))
}

/// The variants of `SqlValue`, in caps.wit order; the checker numbers them the same way.
const NULL: u32 = 0;
const INT: u32 = 1;
const FLOAT: u32 = 2;
const TEXT: u32 = 3;
const BLOB: u32 = 4;

/// The `SqlValue`s of a `List<SqlValue>` argument, copied: wit-bindgen takes them owned.
///
/// # Safety
/// `l` is a live `List<SqlValue>` that outlives the call.
unsafe fn sql_values(l: u32) -> Vec<SqlValue> {
    // SAFETY: the caller's guarantee: each element is a live `SqlValue`, an enum whose variant
    // is at byte 8 and whose payload, if any, is slot 0 (an `Int`'s or `Float`'s bits, or a
    // `Str`'s or `Bytes`' pointer).
    unsafe {
        obj::elems(ptr(l))
            .iter()
            .map(|&s| {
                let p = obj::from_slot(s);
                let object = || obj::bytes(obj::from_slot(obj::payload(p)));
                match *obj::field::<u32>(p, 8) {
                    NULL => SqlValue::Null,
                    INT => SqlValue::Int(obj::payload(p) as i64),
                    FLOAT => SqlValue::Float(f64::from_bits(obj::payload(p))),
                    TEXT => SqlValue::Text(String::from(core::str::from_utf8_unchecked(object()))),
                    BLOB => SqlValue::Blob(object().to_vec()),
                    v => panic!("SqlValue variant {v}"),
                }
            })
            .collect()
    }
}

/// A `List<List<SqlValue>>` of descriptor `list`: each text or blob becomes a `Str` or `Bytes`
/// in place (the host wrote it into `cabi_realloc`'s room).
fn rows(t: &Table, list: u32, rows: Vec<Vec<SqlValue>>) -> *mut Header {
    let Desc::List { elem: row } = t.desc(list) else {
        panic!("not a list")
    };
    let Desc::List { elem: value } = t.desc(row) else {
        panic!("not a list of lists")
    };
    let cell = |v: SqlValue| {
        to_slot(match v {
            SqlValue::Null => obj::alloc(t, value, NULL),
            SqlValue::Int(n) => obj::variant1(t, value, INT, n as u64),
            SqlValue::Float(x) => obj::variant1(t, value, FLOAT, x.to_bits()),
            SqlValue::Text(s) => obj::variant1(t, value, TEXT, to_slot(str_obj(s))),
            SqlValue::Blob(b) => obj::variant1(t, value, BLOB, to_slot(bytes_obj(b))),
        })
    };
    let row = |r: Vec<SqlValue>| to_slot(obj::new_list(row, r.into_iter().map(cell).collect()));
    obj::new_list(list, rows.into_iter().map(row).collect())
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_db_query(ret: u32, h: u32, sql: u32, params: u32) -> u32 {
    // SAFETY: the contract above.
    let (sql, params) = unsafe { (text(sql), sql_values(params)) };
    let d = database(h);
    let r = in_place(|| d.query(sql, &params));
    db_result(ret, r, |t, v| to_slot(rows(t, payload(t, ret, 0), v)))
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_db_exec(ret: u32, h: u32, sql: u32, params: u32) -> u32 {
    // SAFETY: the contract above.
    let (sql, params) = unsafe { (text(sql), sql_values(params)) };
    let d = database(h);
    db_result(ret, in_place(|| d.exec(sql, &params)), |_, n| n as u64)
}

#[no_mangle]
pub unsafe extern "C" fn rt_cap_db_transaction(ret: u32, h: u32, sqls: u32, params: u32) -> u32 {
    // SAFETY: the contract above: `sqls` is a live `List<Str>` and `params` a live
    // `List<List<SqlValue>>`, each element live.
    let (sqls, params): (Vec<String>, Vec<Vec<SqlValue>>) = unsafe {
        let sqls = obj::elems(ptr(sqls))
            .iter()
            .map(|&s| String::from(text(addr(obj::from_slot(s)))));
        let params = obj::elems(ptr(params))
            .iter()
            .map(|&l| sql_values(addr(obj::from_slot(l))));
        (sqls.collect(), params.collect())
    };
    let d = database(h);
    db_result(ret, in_place(|| d.transaction(&sqls, &params)), |_, n| n as u64)
}
