//! `List.sort_by` and `sort_by_key` (L11, C14): one stable merge sort for the interpreter and
//! the compiled runtime, which includes this file by path. `core` and `alloc` only, no `unsafe`.
//!
//! The sort is a state machine that asks for one comparison at a time, so the caller runs the
//! program's `less` between steps (the interpreter through `apply`, compiled code inline) and
//! both paths ask the same questions in the same order. A `less` that is not a strict order
//! still gives a permutation, the same one on both paths.
//!
//! Bottom-up: runs of width 1, 2, 4, .. are merged in pairs from the left. Before a merge, one
//! question checks whether the right run's first element goes before the left run's last; if
//! not, the pair is already in order and is copied. While merging, the right run's head is
//! taken only if it goes before the left run's head (`less(right, left)`), so equal elements
//! keep their order.

use alloc::vec::Vec;

/// A sort in progress over the indexes `0..n`.
pub struct MergeSort {
    src: Vec<u32>,
    dst: Vec<u32>,
    width: usize,
    /// The pair being merged: `lo..mid` and `mid..hi`; heads `i` and `j`.
    lo: usize,
    mid: usize,
    hi: usize,
    i: usize,
    j: usize,
    state: State,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// The next pair starts at `lo`.
    Start,
    /// Asked whether `src[mid]` goes before `src[mid - 1]`.
    Check,
    /// Merging; when both runs have a head, asked whether `src[j]` goes before `src[i]`.
    Merge,
    Done,
}

impl MergeSort {
    pub fn new(n: usize) -> MergeSort {
        let n32 = u32::try_from(n).expect("a list of at most u32::MAX elements");
        MergeSort {
            src: (0..n32).collect(),
            dst: alloc::vec![0; n],
            width: 1,
            lo: 0,
            mid: 0,
            hi: 0,
            i: 0,
            j: 0,
            state: State::Start,
        }
    }

    /// The next question, `(a, b)`: does element `a` go before element `b`, `less(a, b)`?
    /// `None` once sorted. Each question is answered with `answer` before the next `question`.
    pub fn question(&mut self) -> Option<(usize, usize)> {
        let n = self.src.len();
        loop {
            match self.state {
                State::Done => return None,
                State::Start if self.width >= n => {
                    self.state = State::Done;
                    return None;
                }
                State::Start if self.lo >= n => {
                    core::mem::swap(&mut self.src, &mut self.dst);
                    self.width *= 2;
                    self.lo = 0;
                }
                State::Start => {
                    self.mid = n.min(self.lo + self.width);
                    self.hi = n.min(self.lo + 2 * self.width);
                    (self.i, self.j) = (self.lo, self.mid);
                    if self.mid == self.hi {
                        self.copy_rest();
                        continue;
                    }
                    self.state = State::Check;
                    return Some((self.src[self.mid] as usize, self.src[self.mid - 1] as usize));
                }
                State::Merge if self.i < self.mid && self.j < self.hi => {
                    return Some((self.src[self.j] as usize, self.src[self.i] as usize));
                }
                State::Merge => {
                    self.copy_rest();
                    self.state = State::Start;
                }
                State::Check => panic!("MergeSort::question before answer"),
            }
        }
    }

    /// The answer to the last question.
    pub fn answer(&mut self, less: bool) {
        match self.state {
            State::Check if less => self.state = State::Merge,
            State::Check => {
                self.copy_rest();
                self.state = State::Start;
            }
            State::Merge => {
                let k = self.i + self.j - self.mid;
                if less {
                    self.dst[k] = self.src[self.j];
                    self.j += 1;
                } else {
                    self.dst[k] = self.src[self.i];
                    self.i += 1;
                }
            }
            _ => panic!("MergeSort::answer without a question"),
        }
    }

    /// Copies what is left of the pair, `i..mid` then `j..hi`, and moves to the next pair.
    fn copy_rest(&mut self) {
        let (i, j, mid, hi) = (self.i, self.j, self.mid, self.hi);
        let k = i + j - mid;
        self.dst[k..k + mid - i].copy_from_slice(&self.src[i..mid]);
        self.dst[k + mid - i..hi].copy_from_slice(&self.src[j..hi]);
        self.lo = self.hi;
    }

    /// The sorted order once `question` returned `None`: position `k` holds the index of the element
    /// that goes there.
    pub fn order(self) -> Vec<u32> {
        assert!(self.state == State::Done, "MergeSort::order before the sort ended");
        self.src
    }
}

/// Sorts `0..n` with `less(a, b)`, which may fail; the order, or `less`'s first error.
pub fn sort_by<E>(n: usize, mut less: impl FnMut(usize, usize) -> Result<bool, E>) -> Result<Vec<u32>, E> {
    let mut s = MergeSort::new(n);
    while let Some((a, b)) = s.question() {
        let r = less(a, b)?;
        s.answer(r);
    }
    Ok(s.order())
}
