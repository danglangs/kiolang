//! The allocator: size-class free lists over dlmalloc (C8b3), fresh blocks carved from chunks
//! (C8b5).
//!
//! A block of at most 512 bytes and alignment at most 8 is rounded up to a class of 8, 16, ..,
//! 64 bytes, or above that of 128, 192, .., 512 (C8b6: `BTreeMap` nodes, lines, list buffers).
//! A freed block goes on its class's list, linked through its first word, and the next
//! allocation of that class takes it back with no call into dlmalloc. Anything larger, or more
//! aligned, goes to dlmalloc directly.
//!
//! When its list is empty a class carves a fresh block from its current chunk: `CHUNK` bytes,
//! `CHUNK`-aligned, from dlmalloc, holding blocks of that class only, after a `Chunk` header.
//! So building a large value calls dlmalloc once per chunk, not once per block. A carved block
//! finds its chunk by masking its address. A chunk goes back to dlmalloc only when every block
//! carved from it is listed (`flush`): one live block keeps the whole chunk.
//!
//! The lists are handed back when dlmalloc is asked for memory and more than `HELD` bytes (or
//! more than the lists kept at the last hand-back, if that is larger) were freed onto them
//! since: every chunk whose blocks are all listed goes back to dlmalloc, which coalesces them.
//! The same happens, and the allocation is retried once, when dlmalloc fails (the memory limit
//! refused a grow). A hand-back walks the lists twice; the "kept" rule keeps the walks within
//! twice the bytes freed.
//!
//! Generated code pops a list itself (C8b5): `free` is at offset 0, class `c`'s head at
//! `c * size_of::<usize>()`, and `rt_heap_lists` (abi.rs) gives its address. A pop changes
//! nothing else here, so it needs no call.
//!
//! Test-heap builds use dlmalloc alone (lib.rs): there freed objects are poisoned and never
//! reused, and nothing should be recycled behind that check.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use core::ptr;

const STEP: usize = 8;
/// The classes generated code pops (8 to 64 bytes).
pub const CLASSES: usize = 8;
/// The largest block of those classes.
pub const SMALL: usize = STEP * CLASSES;
/// Larger classes, which only the runtime uses (C8b6): 128 to 512 bytes in steps of 64.
const BIG_STEP: usize = 64;
const ALL: usize = CLASSES + 7;
/// The largest block a free list holds.
pub const LISTED: usize = SMALL + BIG_STEP * (ALL - CLASSES);
/// The bytes freed onto the lists before a dlmalloc call hands them back.
pub const HELD: usize = 256 * 1024;
/// A chunk's alignment, and its size with dlmalloc's 8-byte chunk header: dlmalloc places
/// chunks allocated one after another `CHUNK` bytes apart, so aligning them wastes nothing.
pub const CHUNK: usize = 64 * 1024;
const CHUNK_BYTES: usize = CHUNK - 8;
/// The blocks carved at once.
const BATCH: usize = 32;
/// A chunk's first block, after its header.
const FIRST: usize = core::mem::size_of::<Chunk>().next_multiple_of(STEP);

/// The header at the start of a chunk.
#[repr(C)]
struct Chunk {
    /// Blocks carved from it so far.
    carved: u32,
    /// While `flush` runs, its listed blocks; 0 otherwise.
    listed: u32,
    /// While `flush` runs, once it is to be handed back: the next such chunk.
    next: *mut Chunk,
    dead: bool,
}

#[repr(C)]
pub struct Heap<A> {
    /// Each class's most recently freed block; its first word holds the next one. At offset 0:
    /// generated code reads and writes it.
    free: [Cell<*mut u8>; ALL],
    /// Each class's chunk to carve from (null: none), and the offset of its first uncarved byte.
    cur: [Cell<*mut Chunk>; ALL],
    top: [Cell<usize>; ALL],
    /// Bytes freed onto the lists since the last hand-back.
    freed: Cell<usize>,
    /// Bytes the lists kept at the last hand-back.
    kept: Cell<usize>,
    inner: A,
}

// Generated code's pop (codegen `FuncGen::alloc`) relies on these: the heads at offset 0, 4
// bytes each, and classes of 8 to 64 bytes in steps of 8.
const _: () = assert!(core::mem::offset_of!(Heap<()>, free) == 0 && SMALL == 64 && STEP == 8);
#[cfg(target_arch = "wasm32")]
const _: () = assert!(core::mem::size_of::<Cell<*mut u8>>() == 4);

// SAFETY: wasm32-unknown-unknown has one thread, and the runtime starts none, so the lists are
// never touched concurrently. `Sync` is only what `#[global_allocator]` asks for.
#[cfg(target_arch = "wasm32")]
unsafe impl<A: Sync> Sync for Heap<A> {}

impl<A> Heap<A> {
    pub const fn new(inner: A) -> Heap<A> {
        Heap {
            free: [const { Cell::new(ptr::null_mut()) }; ALL],
            cur: [const { Cell::new(ptr::null_mut()) }; ALL],
            top: [const { Cell::new(0) }; ALL],
            freed: Cell::new(0),
            kept: Cell::new(0),
            inner,
        }
    }

    #[cfg(test)]
    pub(crate) fn inner(&self) -> &A {
        &self.inner
    }
}

/// The class of `l` if it is listed: blocks of `block(class)` bytes. The small classes are
/// tested first, as before the larger ones were added.
#[inline(always)]
fn class(l: Layout) -> Option<usize> {
    if l.size() <= SMALL && l.align() <= STEP {
        Some(l.size().saturating_sub(1) / STEP)
    } else if l.size() <= LISTED && l.align() <= STEP {
        Some(CLASSES + (l.size() - SMALL - 1) / BIG_STEP)
    } else {
        None
    }
}

/// Each class's block size.
const BLOCK: [usize; ALL] = {
    let mut b = [0; ALL];
    let mut c = 0;
    while c < ALL {
        b[c] = if c < CLASSES {
            STEP * (c + 1)
        } else {
            SMALL + BIG_STEP * (c - CLASSES + 1)
        };
        c += 1;
    }
    b
};

#[inline(always)]
fn block(c: usize) -> usize {
    BLOCK[c]
}

fn chunk_layout() -> Layout {
    // SAFETY: a power-of-two alignment, and a size below it.
    unsafe { Layout::from_size_align_unchecked(CHUNK_BYTES, CHUNK) }
}

/// The chunk a carved block is in.
fn chunk(p: *mut u8) -> *mut Chunk {
    p.map_addr(|a| a & !(CHUNK - 1)).cast()
}

/// The block after `p` on its list.
///
/// # Safety
/// `p` is listed.
unsafe fn next(p: *mut u8) -> *mut u8 {
    // SAFETY: a listed block's first word is the next one.
    unsafe { p.cast::<*mut u8>().read() }
}

impl<A: GlobalAlloc> Heap<A> {
    /// `f`'s allocation from `inner`, after a hand-back if more than `HELD` (or than the lists
    /// kept at the last one) was freed since, and retried once after a hand-back if it fails.
    unsafe fn inner_alloc(&self, f: impl Fn() -> *mut u8) -> *mut u8 {
        // SAFETY: the lists hold only blocks carved from chunks of `inner`.
        unsafe {
            if self.freed.get() > HELD.max(self.kept.get()) {
                self.flush();
            }
            let p = f();
            if !p.is_null() || !self.flush() {
                return p;
            }
            f()
        }
    }

    /// Hands back to `inner` every chunk whose carved blocks are all listed, unlinking those
    /// blocks; true if there was one.
    ///
    /// # Safety
    /// Every listed block was carved from a chunk (as every block `dealloc` lists was).
    pub(crate) unsafe fn flush(&self) -> bool {
        // SAFETY: a listed block is in a chunk we hold, whose header is ours; chunks are handed
        // back only after the last walk.
        unsafe {
            for head in &self.free {
                let mut p = head.get();
                while !p.is_null() {
                    (*chunk(p)).listed += 1;
                    p = next(p);
                }
            }
            // Relink each list without the blocks of chunks whose blocks are all listed. The
            // first kept block of a chunk resets its count, so its others are kept too.
            let mut dead: *mut Chunk = ptr::null_mut();
            let mut kept = 0;
            for (c, head) in self.free.iter().enumerate() {
                let mut p = head.replace(ptr::null_mut());
                let mut last: *mut u8 = ptr::null_mut();
                while !p.is_null() {
                    let n = next(p);
                    let ch = chunk(p);
                    if (*ch).listed == (*ch).carved {
                        if !(*ch).dead {
                            (*ch).dead = true;
                            (*ch).next = dead;
                            dead = ch;
                        }
                    } else {
                        (*ch).listed = 0;
                        if last.is_null() {
                            head.set(p);
                        } else {
                            last.cast::<*mut u8>().write(p);
                        }
                        last = p;
                        kept += block(c);
                    }
                    p = n;
                }
                if !last.is_null() {
                    last.cast::<*mut u8>().write(ptr::null_mut());
                }
            }
            self.freed.set(0);
            self.kept.set(kept);
            let any = !dead.is_null();
            while !dead.is_null() {
                let n = (*dead).next;
                for cur in &self.cur {
                    if cur.get() == dead {
                        cur.set(ptr::null_mut());
                    }
                }
                self.inner.dealloc(dead.cast(), chunk_layout());
                dead = n;
            }
            any
        }
    }

    /// A block of class `c`, uninitialized: listed, else carved.
    #[inline]
    unsafe fn take(&self, c: usize) -> *mut u8 {
        let p = self.free[c].get();
        if p.is_null() {
            // SAFETY: the caller's contract.
            return unsafe { self.carve(c) };
        }
        // SAFETY: a listed block.
        self.free[c].set(unsafe { next(p) });
        p
    }

    /// A fresh block of class `c` from its chunk, after taking a new chunk if it has no room.
    unsafe fn carve(&self, c: usize) -> *mut u8 {
        // SAFETY: a chunk is `CHUNK_BYTES` from `inner`, `CHUNK`-aligned; blocks are carved
        // after its header and below `CHUNK_BYTES`, 8-aligned (FIRST and every class are
        // multiples of 8).
        unsafe {
            if self.cur[c].get().is_null() || self.top[c].get() + block(c) > CHUNK_BYTES {
                let p = self.inner_alloc(|| self.inner.alloc(chunk_layout()));
                if p.is_null() {
                    return p;
                }
                let ch = p.cast::<Chunk>();
                ch.write(Chunk {
                    carved: 0,
                    listed: 0,
                    next: ptr::null_mut(),
                    dead: false,
                });
                self.cur[c].set(ch);
                self.top[c].set(FIRST);
            }
            // Up to `BATCH` blocks: the first is returned, the others listed (the list is
            // empty), so the next allocations, generated code's too, pop them.
            let ch = self.cur[c].get();
            let top = self.top[c].get();
            let k = BATCH.min((CHUNK_BYTES - top) / block(c));
            let p = ch.cast::<u8>().add(top);
            let mut head = ptr::null_mut();
            for i in (1..k).rev() {
                let q = p.add(i * block(c));
                q.cast::<*mut u8>().write(head);
                head = q;
            }
            self.free[c].set(head);
            self.top[c].set(top + k * block(c));
            (*ch).carved += k as u32;
            p
        }
    }
}

// SAFETY: a small block is at least its layout's size (its class rounds up) and 8-aligned,
// and is on no list while allocated: `take` unlinks it and `dealloc` links it only when the
// caller gives it back, which `GlobalAlloc`'s contract makes the last use. Carved blocks of a
// chunk do not overlap (`top` only grows) and stay inside it.
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
                self.freed.set(self.freed.get() + block(c));
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
                        zero(p, block(c));
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

/// The address of the list heads generated code pops: the global allocator's `free`, or in a
/// test-heap build heads that stay null, so generated code always calls the runtime.
#[cfg(target_arch = "wasm32")]
pub fn lists() -> *const u8 {
    #[cfg(not(feature = "test-heap"))]
    return (&raw const crate::ALLOC).cast();
    #[cfg(feature = "test-heap")]
    {
        static NONE: [usize; CLASSES] = [0; CLASSES];
        (&raw const NONE).cast()
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
