//! The `compiled` world's glue (caps.wit): the `run` export, `rt_trap`, and the two
//! placeholder functions whose bodies codegen replaces with the program's entry. docs/codegen.md, "Entry".

use crate::abi;
use crate::display::outcome;
use crate::obj;
use alloc::vec::Vec;

wit_bindgen::generate!({ world: "compiled", path: "../caps.wit" });

struct Runtime;

impl Guest for Runtime {
    fn run(_grants: Vec<Grant>) -> Result<Outcome, ()> {
        let slot = kio_main();
        let t = abi::table();
        // SAFETY: codegen's entry returns a value of `kio_main_desc` and has called `rt_init`.
        let (p, failed) = unsafe { outcome(&t, kio_main_desc(), slot) };
        // SAFETY: `outcome` made `p`, a live `Str`.
        let value = unsafe { core::str::from_utf8(obj::bytes(p)).expect("a Str is UTF-8").into() };
        // SAFETY: `p` is ours and live, and so is main's value if it is a pointer.
        unsafe {
            abi::rt_dec(abi::addr(p));
            if t.is_ptr(kio_main_desc()) {
                obj::dec(&t, obj::from_slot(slot));
            }
        }
        let live = abi::rt_live();
        if live != 0 {
            rt_trap(LEAKED, i64::from(live), 0);
        }
        Ok(Outcome { value, failed })
    }
}

export!(Runtime);

/// The error site of a runtime bug; program sites count from 0.
pub const INTERNAL: u32 = u32::MAX;

/// The error site of objects left alive when a run ends (a codegen bug), with their count as
/// `a`. The host reports it (host/src/compiled.rs); the differential tests rely on it.
pub const LEAKED: u32 = u32::MAX - 1;

/// The error site whose `trap` returns: the host keeps `a` as the third value (`{c}`) of the
/// message of the `trap` that follows (host/src/compiled.rs).
pub const EXTRA: u32 = u32::MAX - 2;

/// A runtime error at `site` whose message needs a third value `c`: `trap` has two, so `c`
/// goes first through a `trap` at `EXTRA`, which returns.
pub fn trap3(site: u32, a: i64, b: i64, c: i64) -> ! {
    trap(EXTRA, c, 0);
    rt_trap(site, a, b)
}

/// A runtime error at `site`: the host formats the message with `a` and `b`. Generated code
/// calls this, not the import, and the call keeps the import in the module, so codegen never
/// adds one (which would shift every function index).
#[no_mangle]
pub extern "C" fn rt_trap(site: u32, a: i64, b: i64) -> ! {
    trap(site, a, b);
    core::arch::wasm32::unreachable()
}

/// Placeholders, found by export name: codegen replaces their bodies, so no index moves.
/// `kio_main` calls `rt_init` with the program's descriptor table, then the program's `main`,
/// and returns main's value as a slot; `kio_main_desc` returns that value's descriptor.
///
/// Each body calls through a pointer read with `read_volatile`, so the optimizer can assume
/// nothing about what it does, keeps every call, and does not inline it.
static UNSET: extern "C" fn() -> u64 = unset;

extern "C" fn unset() -> u64 {
    panic!("no program: codegen did not fill the entry")
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn kio_main() -> u64 {
    // SAFETY: a valid static.
    unsafe { core::ptr::read_volatile(&UNSET)() }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn kio_main_desc() -> u32 {
    // SAFETY: a valid static.
    unsafe { core::ptr::read_volatile(&UNSET)() as u32 }
}
