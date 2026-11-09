//! `heap::Heap` over the system allocator, counting and limiting what reaches it (Miri runs
//! these too: every block is handed back by the end).

use crate::heap::{Heap, HELD, SMALL};
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
        assert_eq!(inner_live(&h), 2, "both listed, not freed");
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
            let size = 1 + i % (SMALL + 16); // small and large
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
            let size = 1 + (2 * i) % (SMALL + 16);
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
        for layout in [l(SMALL + 1), Layout::from_size_align(16, 16).unwrap()] {
            let p = h.alloc(layout);
            assert_eq!(p as usize % layout.align(), 0);
            h.dealloc(p, layout);
            assert_eq!(inner_live(&h), 0, "{layout:?} went straight back");
        }
    }
}

#[test]
fn running_out_hands_the_lists_back_and_retries() {
    let h = heap(4);
    unsafe {
        let ps: std::vec::Vec<_> = (0..4).map(|_| h.alloc(l(8))).collect();
        assert!(h.alloc(l(32)).is_null(), "the limit");
        for p in ps {
            h.dealloc(p, l(8));
        }
        assert_eq!(inner_live(&h), 4, "listed");
        // Another class, and a large block: the inner allocator is full of listed 8-byte
        // blocks, so each allocation hands them back and succeeds.
        let a = h.alloc(l(32));
        assert!(!a.is_null());
        assert_eq!(inner_live(&h), 1);
        let b = h.alloc(l(100));
        assert!(!b.is_null());
        h.dealloc(a, l(32));
        h.dealloc(b, l(100));
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}

#[test]
fn lists_over_held_are_handed_back_before_the_inner_allocator_is_called() {
    let h = heap(usize::MAX);
    unsafe {
        let n = HELD / SMALL;
        let ps: std::vec::Vec<_> = (0..=n).map(|_| h.alloc(l(SMALL))).collect();
        for p in ps {
            h.dealloc(p, l(SMALL));
        }
        assert_eq!(inner_live(&h), n + 1);
        let a = h.alloc(l(SMALL));
        assert_eq!(inner_live(&h), n + 1, "taken from the list, which now holds HELD");
        let b = h.alloc(l(8));
        assert_eq!(inner_live(&h), n + 2, "HELD is not over HELD: no flush");
        h.dealloc(b, l(8));
        let c = h.alloc(l(16));
        assert_eq!(inner_live(&h), 2, "over HELD: every list handed back first");
        h.dealloc(a, l(SMALL));
        h.dealloc(c, l(16));
        h.flush();
        assert_eq!(inner_live(&h), 0);
    }
}
