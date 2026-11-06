//! The compiled-kio runtime: heap objects, refcounts and (later) the builtins generated code
//! calls. Design: docs/codegen.md.
//!
//! - `desc`: type descriptors, the table codegen emits as data.
//! - `obj`: object layout, allocation, `inc`, `dec`, `unique`; the safe-to-call Rust layer.
//! - `abi`: the `extern "C"` functions generated code calls, over `u32` addresses (wasm32 only).
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
pub mod obj;
pub mod text;

#[cfg(target_arch = "wasm32")]
mod abi;

#[cfg(test)]
mod coll_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_tests;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

/// A runtime bug or out of memory. C4 replaces this with a call to the `trap` import and an
/// internal-error site, so it is never mistaken for a program error.
#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
