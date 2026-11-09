//! `heap::Heap` over the system allocator, counting and limiting what reaches it (Miri runs
//! these too: every block is handed back by the end).

use crate::heap::{Heap, CHUNK, HELD, LISTED, SMALL};
use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use std::alloc::System;

/// The system allocator, with a count of live blocks and a limit on it.
struct Inner {
    live: Cell<usize>,
    limit: Cell<usize>,
}

unsafe impl GlobalAlloc for Inner {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if self.live.get() == self.limit.get() {
            return core::ptr::null_mut();
        }
        self.live.set(self.live.get() + 1);
        // SAFETY: the caller's contract.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        self.live.set(self.live.get() - 1);
        // SAFETY: the caller's contract.
        unsafe { System.dealloc(p, l) }
    }
}

fn heap(limit: usize) -> Heap<Inner> {
    Heap::new(Inner {
        live: Cell::new(0),
        limit: Cell::new(limit),
    })
}

fn inner_live(h: &Heap<Inner>) -> usize {
    h.inner().live.get()
}

fn l(size: usize) -> Layout {
    Layout::from_size_align(size, 8).unwrap()
}

#[test]
fn a_freed_block_is_reused_by_its_class_only() {
    let h = heap(usize::MAX);
    unsafe {
        let a = h.alloc(l(24));
        h.dealloc(a, l(24));
        let b = h.alloc(l(40));
        assert_ne!(a, b, "another class");
        let c = h.alloc(Layout::from_size_align(17, 1).unwrap());
        assert_eq!(a, c, "17 bytes is the 24-byte class");
        h.dealloc(b, l(40));
        h.dealloc(c, Layout::from_size_align(17, 1).unwrap());
        assert_eq!(inner_live(&h), 2, "listed: two chunks held");
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

/// The larger classes (C8b6): 65 to 128 bytes share a block, 129 is the next class, 512 is
/// listed and 513 goes to the inner allocator.
#[test]
fn larger_classes_step_by_64_up_to_listed() {
    let h = heap(usize::MAX);
    unsafe {
        let a = h.alloc(l(65));
        h.dealloc(a, l(65));
        assert_eq!(h.alloc(l(128)), a);
        let b = h.alloc(l(129));
        assert_ne!(a, b);
        let c = h.alloc(l(LISTED));
        h.dealloc(c, l(LISTED));
        assert_eq!(h.alloc(l(LISTED - 63)), c);
        assert_eq!(inner_live(&h), 3, "a chunk per class");
        let d = h.alloc(l(LISTED + 1));
        assert_eq!(inner_live(&h), 4);
        h.dealloc(d, l(LISTED + 1));
        for (p, n) in [(a, 128), (b, 129), (c, LISTED)] {
            h.dealloc(p, l(n));
        }
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn blocks_keep_their_contents_while_others_are_recycled() {
    let h = heap(usize::MAX);
    unsafe {
        let mut blocks = std::vec::Vec::new();
        for i in 0..200usize {
            let size = 1 + i % (LISTED + 16); // small and large
            let p = h.alloc(Layout::from_size_align(size, 1).unwrap());
            core::ptr::write_bytes(p, i as u8, size);
            blocks.push((p, size, i as u8));
        }
        // Free every other one, then allocate again: the new blocks reuse the freed ones.
        let mut kept = std::vec::Vec::new();
        for (k, b) in blocks.into_iter().enumerate() {
            if k % 2 == 0 {
                h.dealloc(b.0, Layout::from_size_align(b.1, 1).unwrap());
            } else {
                kept.push(b);
            }
        }
        for i in 0..100usize {
            let size = 1 + (2 * i) % (LISTED + 16);
            let p = h.alloc(Layout::from_size_align(size, 1).unwrap());
            core::ptr::write_bytes(p, 0xee, size);
            kept.push((p, size, 0xee));
        }
        for &(p, size, v) in &kept {
            assert!(core::slice::from_raw_parts(p, size).iter().all(|&b| b == v));
        }
        for (p, size, _) in kept {
            h.dealloc(p, Layout::from_size_align(size, 1).unwrap());
        }
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn zeroed_blocks_are_zero_when_reused() {
    let h = heap(usize::MAX);
    unsafe {
        let a = h.alloc(l(64));
        core::ptr::write_bytes(a, 0xff, 64);
        h.dealloc(a, l(64));
        let b = h.alloc_zeroed(l(60));
        assert_eq!(a, b);
        assert!(core::slice::from_raw_parts(b, 60).iter().all(|&x| x == 0));
        let big = h.alloc_zeroed(l(1000));
        assert!(core::slice::from_raw_parts(big, 1000).iter().all(|&x| x == 0));
        h.dealloc(b, l(60));
        h.dealloc(big, l(1000));
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn realloc_keeps_contents_across_classes() {
    let h = heap(usize::MAX);
    unsafe {
        let p = h.alloc(l(9));
        core::ptr::write_bytes(p, 7, 9);
        let q = h.realloc(p, l(9), 16);
        assert_eq!(p, q, "same class: in place");
        let r = h.realloc(q, l(16), 40);
        assert_ne!(q, r);
        assert!(core::slice::from_raw_parts(r, 9).iter().all(|&x| x == 7));
        let s = h.realloc(r, l(40), 4000);
        assert!(core::slice::from_raw_parts(s, 9).iter().all(|&x| x == 7));
        let t = h.realloc(s, l(4000), 8);
        assert!(core::slice::from_raw_parts(t, 8).iter().all(|&x| x == 7));
        h.dealloc(t, l(8));
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn large_and_overaligned_blocks_bypass_the_lists() {
    let h = heap(usize::MAX);
    unsafe {
        for layout in [l(LISTED + 1), Layout::from_size_align(16, 16).unwrap()] {
            let p = h.alloc(layout);
            assert_eq!(p as usize % layout.align(), 0);
            h.dealloc(p, layout);
            assert_eq!(inner_live(&h), 0, "{layout:?} went straight back");
        }
    }
}

#[test]
fn running_out_hands_free_chunks_back_and_retries() {
    let h = heap(1);
    unsafe {
        let a = h.alloc(l(8));
        assert_eq!(inner_live(&h), 1, "one chunk");
        assert!(h.alloc(l(32)).is_null(), "another class needs a chunk: the limit");
        h.dealloc(a, l(8));
        assert_eq!(inner_live(&h), 1, "listed");
        // The inner allocator is full with a chunk whose blocks are all listed: the
        // allocation hands it back and succeeds.
        let b = h.alloc(l(32));
        assert!(!b.is_null());
        assert_eq!(inner_live(&h), 1);
        assert!(h.alloc(l(100)).is_null(), "nothing to hand back: the limit");
        h.dealloc(b, l(32));
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn fresh_blocks_are_carved_from_one_chunk_until_it_is_full() {
    let h = heap(usize::MAX);
    unsafe {
        // A 64 KB chunk holds a little under 64 KB / 24 blocks of 24 bytes.
        let n = CHUNK / 24 - 2;
        let mut ps: std::vec::Vec<*mut u8> = (0..n).map(|_| h.alloc(l(24))).collect();
        assert_eq!(inner_live(&h), 1, "{n} blocks, one chunk");
        let mut sorted = ps.clone();
        sorted.sort();
        for w in sorted.windows(2) {
            assert!(w[1] as usize - w[0] as usize >= 24, "blocks do not overlap");
        }
        assert!(sorted
            .iter()
            .all(|&p| p as usize / CHUNK == sorted[0] as usize / CHUNK && (p as usize).is_multiple_of(8)));
        for _ in 0..3 {
            ps.push(h.alloc(l(24)));
        }
        assert_eq!(inner_live(&h), 2, "the chunk is full: another");
        for p in ps {
            h.dealloc(p, l(24));
        }
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn a_chunk_goes_back_only_when_all_its_blocks_are_free() {
    let h = heap(usize::MAX);
    unsafe {
        let ps: std::vec::Vec<*mut u8> = (0..100).map(|_| h.alloc(l(16))).collect();
        let keep = ps[57];
        core::ptr::write_bytes(keep, 0x5a, 16);
        for &p in &ps {
            if p != keep {
                h.dealloc(p, l(16));
            }
        }
        assert!(!h.flush(), "one live block keeps the chunk");
        assert_eq!(inner_live(&h), 1);
        // The kept blocks are still listed and reused; the live one is untouched.
        let again: std::vec::Vec<*mut u8> = (0..99).map(|_| h.alloc(l(16))).collect();
        assert_eq!(inner_live(&h), 1, "all 99 from the list");
        assert!(again.iter().all(|&p| p != keep));
        assert!(core::slice::from_raw_parts(keep, 16).iter().all(|&b| b == 0x5a));
        for p in again {
            h.dealloc(p, l(16));
        }
        h.dealloc(keep, l(16));
        assert!(h.flush());
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn freeing_over_held_hands_free_chunks_back_before_the_inner_allocator_is_called() {
    let h = heap(usize::MAX);
    unsafe {
        let n = HELD / SMALL + 1;
        let ps: std::vec::Vec<_> = (0..n).map(|_| h.alloc(l(SMALL))).collect();
        let chunks = inner_live(&h);
        assert!(chunks > 1);
        let keep = h.alloc(l(8));
        for p in ps {
            h.dealloc(p, l(SMALL));
        }
        assert_eq!(inner_live(&h), chunks + 1, "listed, not handed back");
        let a = h.alloc(l(SMALL));
        assert_eq!(inner_live(&h), chunks + 1, "taken from the list, no inner call");
        h.dealloc(a, l(SMALL));
        let big = h.alloc(l(LISTED + 1));
        assert_eq!(inner_live(&h), 2, "over HELD freed: every free chunk handed back first");
        h.dealloc(big, l(LISTED + 1));
        h.dealloc(keep, l(8));
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn a_hand_back_that_keeps_much_waits_for_as_much_to_be_freed() {
    let h = heap(usize::MAX);
    unsafe {
        // Every chunk keeps one live block, so a hand-back frees nothing and keeps the rest.
        let n = 4 * HELD / SMALL;
        let ps: std::vec::Vec<_> = (0..n).map(|_| h.alloc(l(SMALL))).collect();
        let chunks = inner_live(&h);
        let per = (CHUNK - 8) / SMALL - 1;
        let (live, freed): (std::vec::Vec<_>, std::vec::Vec<_>) = ps
            .into_iter()
            .enumerate()
            .partition(|(i, _)| i % per == 0 || *i == n - 1);
        for &(_, p) in &freed {
            h.dealloc(p, l(SMALL));
        }
        let big = h.alloc(l(LISTED + 1)); // freed > HELD: a hand-back, which keeps everything
        assert_eq!(inner_live(&h), chunks + 1);
        let kept = freed.len() * SMALL;
        assert!(kept > HELD);
        // Freeing less than was kept does not walk the lists again before an inner call.
        let small: std::vec::Vec<_> = (0..HELD / 8 + 1).map(|_| h.alloc(l(8))).collect();
        let held = inner_live(&h);
        for &p in &small {
            h.dealloc(p, l(8));
        }
        let big2 = h.alloc(l(LISTED + 1));
        assert_eq!(
            inner_live(&h),
            held + 1,
            "no hand-back: the 8-byte chunks are still held"
        );
        for p in [big, big2] {
            h.dealloc(p, l(LISTED + 1));
        }
        for (_, p) in live {
            h.dealloc(p, l(SMALL));
        }
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}
