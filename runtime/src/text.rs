//! `Str` and `Bytes` operations, with the interpreter's results and errors (lang/src/eval.rs).
//!
//! Every function borrows its pointer arguments and returns a new object with count 1 (or a
//! plain value). A `Str` always holds valid UTF-8: only `new_str`, `to_str` and these
//! operations make one, so `str_of` need not check.
//!
//! An `Err` is a program error: codegen turns it into a `trap` with the site's message.
//! Nothing here panics on a program's input.
//!
//! Each `unsafe fn` here has the contract of `obj`'s: its pointers keep the `obj` invariants for
//! `t`, and each is a `Str` or `Bytes` as the function says.

use crate::desc::{self, Table};
use crate::obj::{self, to_slot, Header, List};
use alloc::string::ToString;

/// Why an operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Err {
    /// `split` with an empty separator.
    EmptySeparator,
    /// `to_str` of bytes that are not UTF-8.
    InvalidUtf8,
    /// A `Bytes` index outside `0..len`.
    OutOfBounds,
    /// A `push` of an `Int` outside `0..=255`.
    ByteRange,
}

/// A `Str` holding a copy of `s`.
pub fn new_str(s: &str) -> *mut Header {
    obj::new_bytes(desc::STR, s.as_bytes())
}

/// A `Bytes` holding a copy of `b`.
pub fn new_bytes(b: &[u8]) -> *mut Header {
    obj::new_bytes(desc::BYTES, b)
}

/// A `Str`'s contents.
///
/// # Safety
/// `p` is a `Str` that outlives `'a` unchanged.
unsafe fn str_of<'a>(p: *mut Header) -> &'a str {
    // SAFETY: the caller's guarantee; a `Str` is UTF-8 (module doc).
    unsafe { core::str::from_utf8_unchecked(obj::bytes(p)) }
}

/// `len` of a `Str` or `Bytes`, in bytes.
///
/// # Safety
/// The module's contract.
pub unsafe fn len(p: *mut Header) -> i64 {
    // SAFETY: the caller's guarantee.
    unsafe { obj::bytes(p).len() as i64 }
}

/// Structural equality of two `Str`s or two `Bytes`.
///
/// # Safety
/// The module's contract; `a` and `b` are both `Str` or both `Bytes`.
pub unsafe fn eq(a: *mut Header, b: *mut Header) -> bool {
    // SAFETY: the caller's guarantee.
    unsafe { obj::bytes(a) == obj::bytes(b) }
}

/// `a + b` on `Str`s.
///
/// # Safety
/// The module's contract.
pub unsafe fn concat(a: *mut Header, b: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee; `p` has room for both, `a`'s bytes then `len`.
    unsafe {
        let (a, b) = (obj::bytes(a), obj::bytes(b));
        let len = a.len() + b.len();
        let p = obj::new_bytes_cap(desc::STR, a, len);
        core::ptr::copy_nonoverlapping(b.as_ptr(), obj::field(p, obj::BYTES_DATA + a.len()), b.len());
        *obj::field::<u32>(p, 8) = u32::try_from(len).expect("over 4 GiB");
        p
    }
}

/// `Str.to_bytes()`.
///
/// # Safety
/// The module's contract.
pub unsafe fn to_bytes(s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { new_bytes(obj::bytes(s)) }
}

/// `Bytes.to_str()`; `InvalidUtf8` unless the bytes are UTF-8.
///
/// # Safety
/// The module's contract.
pub unsafe fn to_str(b: *mut Header) -> Result<*mut Header, Err> {
    // SAFETY: the caller's guarantee.
    match core::str::from_utf8(unsafe { obj::bytes(b) }) {
        Ok(s) => Ok(new_str(s)),
        Err(_) => Err(Err::InvalidUtf8),
    }
}

/// `Int.to_str()`.
pub fn int_to_str(n: i64) -> *mut Header {
    new_str(&n.to_string())
}

/// A `List` (descriptor `list`, of `Str`) of new `Str`s.
fn strs<'a>(list: u32, parts: impl Iterator<Item = &'a str>) -> *mut Header {
    obj::new_list(list, parts.map(|s| to_slot(new_str(s))).collect::<List>())
}

/// `Str.split(sep)`: a `List<Str>` with descriptor `list`. `EmptySeparator` if `sep` is empty.
///
/// # Safety
/// The module's contract; `list` is the descriptor of `List<Str>`.
pub unsafe fn split(list: u32, s: *mut Header, sep: *mut Header) -> Result<*mut Header, Err> {
    // SAFETY: the caller's guarantee.
    unsafe {
        let sep = str_of(sep);
        if sep.is_empty() {
            return Err(Err::EmptySeparator);
        }
        Ok(strs(list, str_of(s).split(sep)))
    }
}

/// `Str.lines()`: a `List<Str>` with descriptor `list`. As Rust's `lines`: splits at `\n`,
/// drops one trailing `\r` per line and no final empty line.
///
/// # Safety
/// The module's contract; `list` is the descriptor of `List<Str>`.
pub unsafe fn lines(list: u32, s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { strs(list, str_of(s).lines()) }
}

/// `Str.trim()`: Unicode whitespace off both ends.
///
/// # Safety
/// The module's contract.
pub unsafe fn trim(s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { new_str(str_of(s).trim()) }
}

/// `Str.contains(sub)`.
///
/// # Safety
/// The module's contract.
pub unsafe fn contains(s: *mut Header, sub: *mut Header) -> bool {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).contains(str_of(sub)) }
}

/// `Str.starts_with(prefix)`.
///
/// # Safety
/// The module's contract.
pub unsafe fn starts_with(s: *mut Header, prefix: *mut Header) -> bool {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).starts_with(str_of(prefix)) }
}

/// `Str.parse_int()`: an `Option<Int>` with descriptor `opt`, as Rust's `i64::from_str`: an
/// optional `+` or `-` then digits, nothing else. `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn parse_int(t: &Table, opt: u32, none: *mut Header, s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, str_of(s).parse::<i64>().ok().map(|n| n as u64)) }
}

/// `Str.ends_with(suffix)`.
///
/// # Safety
/// The module's contract.
pub unsafe fn ends_with(s: *mut Header, suffix: *mut Header) -> bool {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).ends_with(str_of(suffix)) }
}

/// `Str.strip_prefix(p)`: an `Option<Str>` with descriptor `opt`; `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn strip_prefix(t: &Table, opt: u32, none: *mut Header, s: *mut Header, p: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe {
        let r = str_of(s).strip_prefix(str_of(p));
        obj::option(t, opt, none, r.map(|r| to_slot(new_str(r))))
    }
}

/// `Str.strip_suffix(p)`: an `Option<Str>` with descriptor `opt`; `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn strip_suffix(t: &Table, opt: u32, none: *mut Header, s: *mut Header, p: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe {
        let r = str_of(s).strip_suffix(str_of(p));
        obj::option(t, opt, none, r.map(|r| to_slot(new_str(r))))
    }
}

/// `Str.find(sub)`: the byte offset of the first match, an `Option<Int>` with descriptor `opt`;
/// `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn find(t: &Table, opt: u32, none: *mut Header, s: *mut Header, sub: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, str_of(s).find(str_of(sub)).map(|i| i as u64)) }
}

/// `Str.replace(from, to)`: every non-overlapping match, left to right; an empty `from`
/// matches before each char and at the end, as Rust's.
///
/// # Safety
/// The module's contract.
pub unsafe fn replace(s: *mut Header, from: *mut Header, to: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { new_str(&str_of(s).replace(str_of(from), str_of(to))) }
}

/// `bytes[i]`; `OutOfBounds` unless `0 <= i < len`.
///
/// # Safety
/// The module's contract; `b` is a `Bytes`.
pub unsafe fn index(b: *mut Header, i: i64) -> Result<i64, Err> {
    // SAFETY: the caller's guarantee.
    let b = unsafe { obj::bytes(b) };
    usize::try_from(i)
        .ok()
        .and_then(|i| b.get(i))
        .map(|&x| i64::from(x))
        .ok_or(Err::OutOfBounds)
}

/// `bytes.push(n)`; `ByteRange` unless `0 <= n <= 255`. Returns the `Bytes` to store back: `b`
/// itself if it had room, else a larger copy (capacity doubled, so pushes cost O(1) amortized)
/// and `b` freed.
///
/// `b` must be unshared: the caller has called `obj::unique` on it (as for every mutation), so
/// this owns its reference. On `Err`, `b` is untouched and still owned.
///
/// # Safety
/// The module's contract; `b` is a `Bytes` with count 1.
pub unsafe fn push(t: &Table, b: *mut Header, n: i64) -> Result<*mut Header, Err> {
    let n = u8::try_from(n).map_err(|_| Err::ByteRange)?;
    // SAFETY: the caller's guarantee; `len` and `cap` are at 8 and 12, with `cap` bytes of room.
    unsafe {
        debug_assert!((*b).rc == 1);
        let len = *obj::field::<u32>(b, 8);
        let cap = *obj::field::<u32>(b, 12);
        let b = if len < cap {
            b
        } else {
            let cap = (cap as usize * 2).max(16).max(len as usize + 1);
            let q = obj::new_bytes_cap(desc::BYTES, obj::bytes(b), cap);
            obj::dec(t, b);
            q
        };
        *obj::field::<u8>(b, obj::BYTES_DATA + len as usize) = n;
        *obj::field::<u32>(b, 8) = len + 1;
        Ok(b)
    }
}
