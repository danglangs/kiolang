#![allow(dead_code)]
use lang::CapTable;

/// The capability table every capability test uses.
pub fn caps() -> CapTable {
    CapTable::v0()
}

struct Moved<T>(T);
// SAFETY: `big_stack` hands over a result only after its thread has finished, so no
// reference count is touched from two threads at once.
unsafe impl<T> Send for Moved<T> {}

/// Runs `f` on twice the stack `run` documents for debug builds, so growth shows up before CI
/// breaks. `Value` shares storage by reference count, so it is not `Send` on its own.
pub fn big_stack<T: 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let t = std::thread::Builder::new().stack_size(16 << 20);
    t.spawn(move || Moved(f())).unwrap().join().unwrap().0
}
