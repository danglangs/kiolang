//! The compiled-kio runtime: heap objects, refcounts and (later) the builtins generated code
//! calls. Design: docs/codegen.md.
//!
//! - `desc`: type descriptors, the table codegen emits as data.
//! - `display`: a value as `kio run` prints it.
//! - `obj`: object layout, allocation, `inc`, `dec`, `unique`; the safe-to-call Rust layer.
//! - `abi`: the `extern "C"` functions generated code calls, over `u32` addresses (wasm32 only).
//! - `entry`: the `compiled` world's `run` export, and the entry codegen fills (wasm32 only).
//! - `caps`: capability calls over the world's imports, and `cabi_realloc` (wasm32 only).
//!
//! `no_std` + `alloc`. On wasm32 the allocator is dlmalloc and a panic is a Wasm trap; natively
//! (tests) std supplies the system allocator and the panic handler.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;
#[cfg(not(target_arch = "wasm32"))]
extern crate std;

pub mod coll;
pub mod desc;
pub mod display;
pub mod float;
pub mod obj;
pub mod text;

#[cfg(target_arch = "wasm32")]
mod abi;
#[cfg(target_arch = "wasm32")]
mod caps;
#[cfg(target_arch = "wasm32")]
mod entry;

#[cfg(test)]
mod coll_tests;
#[cfg(test)]
mod display_tests;
#[cfg(test)]
mod float_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_tests;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

/// A runtime bug or out of memory: the internal-error site, so it is never mistaken for a
/// program error.
#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    entry::rt_trap(entry::INTERNAL, 0, 0)
}
