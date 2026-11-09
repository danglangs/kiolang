#![allow(dead_code)]
use lang::CapTable;

pub mod collide;
pub mod corpus;

/// The capability table every capability test uses.
pub fn caps() -> CapTable {
    CapTable::v0()
}

struct Moved<T>(T);
// SAFETY: `big_stack` hands over its closure, which owns what it captures, before its thread
// starts and takes the result only after the thread has finished, so no reference count is
// touched from two threads at once.
unsafe impl<T> Send for Moved<T> {}

/// Runs `f` on twice the stack `run` documents for debug builds, so growth shows up before CI
/// breaks. `Value` and `Program` share storage by reference count, so they are not `Send` on their own.
pub fn big_stack<T: 'static>(f: impl FnOnce() -> T + 'static) -> T {
    let t = std::thread::Builder::new().stack_size(16 << 20);
    let f = Moved(f);
    t.spawn(move || {
        let f = f;
        Moved((f.0)())
    })
    .unwrap()
    .join()
    .unwrap()
    .0
}
