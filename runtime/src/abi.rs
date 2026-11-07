//! The functions generated code calls. A pointer is a `u32` address in linear memory, 0 for
//! none; each function keeps `obj`'s contract if its arguments do, which codegen guarantees.
//!
//! The descriptor table is data codegen emits: a `RawTable` of five (address, length) pairs,
//! registered once with `rt_init` before any other call.

use crate::desc::Table;
use crate::obj::{self, Header};
use core::slice;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// `Table`'s arrays as codegen lays them out: addresses and element counts.
#[repr(C)]
pub struct RawTable {
    descs: [u32; 2],
    slots: [u32; 2],
    fields: [u32; 2],
    variants: [u32; 2],
    names: [u32; 2],
}

static TABLE: AtomicU32 = AtomicU32::new(0);

pub(crate) fn table() -> Table<'static> {
    let raw = TABLE.load(Relaxed);
    assert!(raw != 0, "rt_init was not called");
    // SAFETY: `rt_init`'s contract.
    unsafe {
        let raw = &*ptr::<RawTable>(raw);
        Table {
            descs: array(raw.descs),
            slots: array(raw.slots),
            fields: array(raw.fields),
            variants: array(raw.variants),
            names: array(raw.names),
        }
    }
}

/// # Safety
/// `len` `T`s at `addr`, aligned, never written.
unsafe fn array<T>([addr, len]: [u32; 2]) -> &'static [T] {
    // SAFETY: the caller's guarantee.
    unsafe { slice::from_raw_parts(ptr(addr), len as usize) }
}

fn ptr<T>(a: u32) -> *mut T {
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

/// Objects allocated and not yet freed, for the differential tests' leak check.
#[cfg(feature = "test-heap")]
#[no_mangle]
pub extern "C" fn rt_live() -> u32 {
    obj::live() as u32
}
