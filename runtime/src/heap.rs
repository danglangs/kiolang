//! The allocator: size-class free lists over dlmalloc (C8b3).
//!
//! A block of at most 64 bytes and alignment at most 8 is rounded up to a class of 8, 16, ..,
//! 64 bytes. A freed block goes on its class's list, linked through its first word, and the
//! next allocation of that class takes it back with no call into dlmalloc. Anything larger,
//! or more aligned, goes to dlmalloc directly. Every small block is allocated from dlmalloc
//! with its class's size and alignment 8, so any list's block can be handed back to dlmalloc.
//!
//! The lists hold only blocks the program freed, and never more than `HELD` bytes when
//! dlmalloc is asked for memory: past that, every listed block is handed back first (dlmalloc
//! coalesces them), so dlmalloc never grows memory while the lists keep more than `HELD` from
//! it. The same happens, and the allocation is retried once, when dlmalloc fails (the memory
//! limit refused a grow). So a program runs out of memory where it did before, give or take
//! `HELD` and where dlmalloc places blocks. Handing a block back costs the `free` that listing
//! it saved, so a flush never costs more than dlmalloc alone would have.
//!
//! Test-heap builds use dlmalloc alone (lib.rs): there freed objects are poisoned and never
//! reused, and nothing should be recycled behind that check.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use core::ptr;

const STEP: usize = 8;
const CLASSES: usize = 8;
/// The largest block a free list holds.
pub const SMALL: usize = STEP * CLASSES;
/// The most the lists may hold when dlmalloc is called.
pub const HELD: usize = 256 * 1024;

pub struct Heap<A> {
    inner: A,
    /// Each class's most recently freed block; its first word holds the next one.
    free: [Cell<*mut u8>; CLASSES],
    /// The bytes listed.
    held: Cell<usize>,
}

// SAFETY: wasm32-unknown-unknown has one thread, and the runtime starts none, so the lists are
// never touched concurrently. `Sync` is only what `#[global_allocator]` asks for.
#[cfg(target_arch = "wasm32")]
unsafe impl<A: Sync> Sync for Heap<A> {}

impl<A> Heap<A> {
    pub const fn new(inner: A) -> Heap<A> {
        Heap {
            inner,
            free: [const { Cell::new(ptr::null_mut()) }; CLASSES],
            held: Cell::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn inner(&self) -> &A {
        &self.inner
    }
}

/// The class of `l` if it is small: blocks of `STEP * (class + 1)` bytes.
fn class(l: Layout) -> Option<usize> {
    if l.size() <= SMALL && l.align() <= STEP {
        Some(l.size().saturating_sub(1) / STEP)
    } else {
        None
    }
}

fn block(c: usize) -> Layout {
    // SAFETY: a multiple of 8 up to 64, alignment 8.
    unsafe { Layout::from_size_align_unchecked(STEP * (c + 1), STEP) }
}

impl<A: GlobalAlloc> Heap<A> {
    /// `f`'s allocation from `inner`, after handing every list back if they hold more than
    /// `HELD`, and retried once after handing them back if it fails.
    unsafe fn inner_alloc(&self, f: impl Fn() -> *mut u8) -> *mut u8 {
        // SAFETY: the lists hold only blocks of their class, allocated from `inner`.
        unsafe {
            if self.held.get() > HELD {
                self.flush();
            }
            let p = f();
            if !p.is_null() || self.held.get() == 0 {
                return p;
            }
            self.flush();
            f()
        }
    }

    /// Hands every listed block back to `inner`.
    ///
    /// # Safety
    /// Every listed block came from `inner` (as every block `dealloc` lists does).
    pub(crate) unsafe fn flush(&self) {
        for (c, head) in self.free.iter().enumerate() {
            let mut p = head.replace(ptr::null_mut());
            while !p.is_null() {
                // SAFETY: a listed block: its first word is the next one, and it came from
                // `inner` with `block(c)`.
                unsafe {
                    let next = p.cast::<*mut u8>().read();
                    self.inner.dealloc(p, block(c));
                    p = next;
                }
            }
        }
        self.held.set(0);
    }

    /// A block of class `c`, uninitialized.
    unsafe fn take(&self, c: usize) -> *mut u8 {
        let p = self.free[c].get();
        if p.is_null() {
            // SAFETY: `block(c)` is not zero-sized.
            return unsafe { self.inner_alloc(|| self.inner.alloc(block(c))) };
        }
        // SAFETY: a listed block, whose first word is the next one.
        self.free[c].set(unsafe { p.cast::<*mut u8>().read() });
        self.held.set(self.held.get() - block(c).size());
        p
    }
}

// SAFETY: a small block is at least its layout's size (its class rounds up) and 8-aligned,
// and is on no list while allocated: `take` unlinks it and `dealloc` links it only when the
// caller gives it back, which `GlobalAlloc`'s contract makes the last use.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Heap<A> {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `l`, passed on.
        unsafe {
            match class(l) {
                Some(c) => self.take(c),
                None => self.inner_alloc(|| self.inner.alloc(l)),
            }
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        match class(l) {
            Some(c) => {
                // SAFETY: `p` is a block of class `c` (allocated with a layout of the same
                // class, as the contract requires), now ours; 8-aligned, at least 8 bytes.
                unsafe { p.cast::<*mut u8>().write(self.free[c].get()) };
                self.free[c].set(p);
                self.held.set(self.held.get() + block(c).size());
            }
            // SAFETY: the caller's contract, passed on.
            None => unsafe { self.inner.dealloc(p, l) },
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `l`, passed on.
        unsafe {
            match class(l) {
                Some(c) => {
                    let p = self.take(c);
                    if !p.is_null() {
                        zero(p, block(c).size());
                    }
                    p
                }
                None => self.inner_alloc(|| self.inner.alloc_zeroed(l)),
            }
        }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller's contract: `p` is live with layout `l`, and `size` with `l`'s
        // alignment is a valid layout.
        unsafe {
            let new = Layout::from_size_align_unchecked(size, l.align());
            match (class(l), class(new)) {
                (Some(a), Some(b)) if a == b => p,
                (None, None) => self.inner_alloc(|| self.inner.realloc(p, l, size)),
                _ => {
                    let q = self.alloc(new);
                    if !q.is_null() {
                        ptr::copy_nonoverlapping(p, q, l.size().min(size));
                        self.dealloc(p, l);
                    }
                    q
                }
            }
        }
    }
}

/// Zeroes `n` bytes (a multiple of 8) at 8-aligned `p` with word stores. Volatile, so LLVM
/// does not turn the loop into a `memory.fill`, which Wasmtime runs as a call into the host
/// (5% of the tree benchmark when objects were zeroed that way).
///
/// # Safety
/// `p` is writable for `n` bytes and 8-aligned.
pub unsafe fn zero(p: *mut u8, n: usize) {
    for i in 0..n / 8 {
        // SAFETY: the caller's guarantee.
        unsafe { p.cast::<u64>().add(i).write_volatile(0) };
    }
}

/// The runtime's allocator, called directly (not through `alloc::alloc`, which adds a call
/// per allocation on wasm32): free lists on wasm32, the system allocator natively.
///
/// # Safety
/// `GlobalAlloc::alloc`'s contract.
#[inline]
pub unsafe fn alloc(l: Layout) -> *mut u8 {
    // SAFETY: the caller's contract.
    unsafe {
        #[cfg(target_arch = "wasm32")]
        return crate::ALLOC.alloc(l);
        #[cfg(not(target_arch = "wasm32"))]
        return alloc::alloc::alloc(l);
    }
}

/// Frees what `alloc` (or `alloc::alloc`) returned.
///
/// # Safety
/// `GlobalAlloc::dealloc`'s contract.
#[inline]
pub unsafe fn dealloc(p: *mut u8, l: Layout) {
    // SAFETY: the caller's contract.
    unsafe {
        #[cfg(target_arch = "wasm32")]
        crate::ALLOC.dealloc(p, l);
        #[cfg(not(target_arch = "wasm32"))]
        alloc::alloc::dealloc(p, l);
    }
}
