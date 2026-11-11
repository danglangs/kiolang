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
    /// A `slice(a, b)` unless `0 <= a <= b <= len`.
    SliceBounds,
    /// A `Str.slice(a, b)` in bounds whose `a` or `b` is inside a character.
    CharBoundary,
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
    unsafe { bytes_eq(obj::bytes(a), obj::bytes(b)) }
}

/// `a == b`, 8 bytes at a time while 8 are left, then byte by byte (C8b2): on wasm32 `[u8]`'s
/// own `==` is `memcmp`, a loop over single bytes.
pub fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && {
        let i = same_words(a, b);
        a[i..].iter().zip(&b[i..]).all(|(p, q)| p == q)
    }
}

/// `a.cmp(b)` by bytes: 8 at a time while 8 are left in both, then byte by byte, so a short
/// string that differs early costs no more than `memcmp`. In an unequal word the lowest
/// differing byte (the word is little-endian) is the first: Wasm has no byte swap, so this
/// finds it with `ctz` rather than comparing big-endian words.
pub fn bytes_cmp(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    let n = a.len().min(b.len());
    let mut i = same_words(a, b);
    if i + 8 <= n {
        let (p, q) = (word(a, i), word(b, i));
        let at = (p ^ q).trailing_zeros() / 8 * 8;
        return ((p >> at) & 0xff).cmp(&((q >> at) & 0xff));
    }
    while i < n {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
        i += 1;
    }
    a.len().cmp(&b.len())
}

/// The offset of the first 8-byte word where `a` and `b` differ, else of the first byte after
/// the whole words both have: every byte before it is equal.
fn same_words(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + 8 <= n && word(a, i) == word(b, i) {
        i += 8;
    }
    i
}

/// The 8 bytes of `s` at `i`, little-endian (byte `i` lowest).
fn word(s: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(s[i..i + 8].try_into().expect("8 bytes"))
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

/// The capacity to grow to for `needed` bytes from capacity `cap`: at least double, so a run
/// of appends copies O(final length) bytes in all.
fn grown(needed: usize, cap: usize) -> usize {
    // Doubling stops at 1 GiB: twice that would not fit a `Layout` on wasm32.
    needed.max(cap.saturating_mul(2).min(1 << 30)).max(16)
}

/// `a + b` on `Str`s for `a = a + b`, taking `a`'s reference and borrowing `b`. If this is
/// `a`'s only reference, `b` is appended to `a` itself (in its spare capacity, or after moving
/// it to a buffer at least twice as large); otherwise (shared or pinned) the result is a new
/// `Str` with room to grow, and `a` is dropped. Returns the result, count 1.
///
/// `a` and `b` may be one object: it is then copied, never grown under `b`, and dropped once
/// copied, so the caller may hold one reference or two.
///
/// # Safety
/// The module's contract.
pub unsafe fn append(t: &Table, a: *mut Header, b: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee; `a` has `len` and `cap` at 8 and 12 and `cap` bytes of
    // room. `b` stays live and unchanged until its bytes are copied: only an `a` with count 1,
    // which is then not `b`, is written before that, and `a` is freed after.
    unsafe {
        let add = obj::bytes(b);
        let len = *obj::field::<u32>(a, 8) as usize;
        let cap = *obj::field::<u32>(a, 12) as usize;
        let needed = len.checked_add(add.len()).expect("over 4 GiB");
        let len32 = u32::try_from(needed).expect("over 4 GiB");
        if (*a).rc == 1 && a != b {
            let a = if needed <= cap {
                a
            } else {
                obj::regrow(a, grown(needed, cap))
            };
            core::ptr::copy_nonoverlapping(add.as_ptr(), obj::field(a, obj::BYTES_DATA + len), add.len());
            *obj::field::<u32>(a, 8) = len32;
            a
        } else {
            // `a` is dropped after `b`'s bytes are copied: it may be `b` (C8b2).
            let q = obj::new_bytes_cap(desc::STR, obj::bytes(a), grown(needed, len));
            core::ptr::copy_nonoverlapping(add.as_ptr(), obj::field(q, obj::BYTES_DATA + len), add.len());
            *obj::field::<u32>(q, 8) = len32;
            obj::dec(t, a);
            q
        }
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
    obj::new_bytes(desc::STR, int_text(n, &mut [0; 20]))
}

/// `n` in decimal, as `{}` writes it, in the end of `buf`, without `core::fmt` (C8b6). 20 bytes
/// hold `i64::MIN`: a sign and 19 digits.
pub fn int_text(n: i64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    let mut m = n.unsigned_abs();
    loop {
        i -= 1;
        buf[i] = b'0' + (m % 10) as u8;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    if n < 0 {
        i -= 1;
        buf[i] = b'-';
    }
    &buf[i..]
}

/// `Float.to_str()`: Rust's `{}`, as the interpreter's `to_string`.
pub fn float_to_str(x: f64) -> *mut Header {
    new_str(&x.to_string())
}

/// `Str.parse_float()`: an `Option<Float>` with descriptor `opt` (the payload as its bits), as
/// Rust's `f64::from_str`. `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn parse_float(t: &Table, opt: u32, none: *mut Header, s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, parse_float_opt(s)) }
}

/// `parse_float`'s payload (the bits) without the `Option` object (C8c1).
///
/// # Safety
/// The module's contract.
pub unsafe fn parse_float_opt(s: *mut Header) -> Option<u64> {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).parse::<f64>().ok().map(f64::to_bits) }
}

/// Offsets `a..b` as a range within `len`; `SliceBounds` unless `0 <= a <= b <= len`.
fn range(a: i64, b: i64, len: usize) -> Result<core::ops::Range<usize>, Err> {
    match (usize::try_from(a), usize::try_from(b)) {
        (Ok(x), Ok(y)) if x <= y && y <= len => Ok(x..y),
        _ => Err(Err::SliceBounds),
    }
}

/// `Str.slice(a, b)`: bytes `a..b` as a new `Str`; `SliceBounds`, then `CharBoundary` if either
/// end splits a character.
///
/// # Safety
/// The module's contract.
pub unsafe fn slice(s: *mut Header, a: i64, b: i64) -> Result<*mut Header, Err> {
    // SAFETY: the caller's guarantee.
    let s = unsafe { str_of(s) };
    s.get(range(a, b, s.len())?).map(new_str).ok_or(Err::CharBoundary)
}

/// `Bytes.slice(a, b)`: bytes `a..b` as a new `Bytes`; `SliceBounds`.
///
/// # Safety
/// The module's contract.
pub unsafe fn bytes_slice(p: *mut Header, a: i64, b: i64) -> Result<*mut Header, Err> {
    // SAFETY: the caller's guarantee.
    let v = unsafe { obj::bytes(p) };
    Ok(new_bytes(&v[range(a, b, v.len())?]))
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
        let s = str_of(s);
        Ok(match *sep.as_bytes() {
            [c] => split_byte(list, s.as_bytes(), c),
            _ => strs(list, s.split(sep)),
        })
    }
}

/// `split` at the one-byte separator `c` (C8b6): the list sized once by counting `c` first,
/// and `c` found 8 bytes at a time. `c` is ASCII (a one-byte `Str`), and an ASCII byte in UTF-8
/// is always a whole character, so these are `str::split`'s parts, and each is UTF-8.
fn split_byte(list: u32, s: &[u8], c: u8) -> *mut Header {
    let mut n = 1;
    each_byte(s, c, |_| n += 1);
    let mut parts = List::with_capacity(n);
    let mut start = 0;
    each_byte(s, c, |i| {
        parts.push(to_slot(obj::new_bytes(desc::STR, &s[start..i])));
        start = i + 1;
    });
    parts.push(to_slot(obj::new_bytes(desc::STR, &s[start..])));
    obj::new_list(list, parts)
}

/// Calls `f` with the offset of each byte of `s` equal to `c`, in order: 8 bytes at a time
/// while 8 are left, then one at a time.
fn each_byte(s: &[u8], c: u8, mut f: impl FnMut(usize)) {
    const LOW7: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    let mut i = 0;
    while i + 8 <= s.len() {
        // The high bit of each byte that is 0 in `x`, exactly: `(x & LOW7) + LOW7` sets it in
        // each byte whose low 7 bits are not all 0, without carrying into the next byte.
        let x = word(s, i) ^ (u64::from(c) * 0x0101_0101_0101_0101);
        let mut m = !(((x & LOW7) + LOW7) | x | LOW7);
        while m != 0 {
            f(i + (m.trailing_zeros() / 8) as usize);
            m &= m - 1;
        }
        i += 8;
    }
    for (k, &b) in s[i..].iter().enumerate() {
        if b == c {
            f(i + k);
        }
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

/// `Str.chars()`: a `List<Str>` with descriptor `list`, one `Str` per Unicode scalar value.
///
/// # Safety
/// The module's contract; `list` is the descriptor of `List<Str>`.
pub unsafe fn chars(list: u32, s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    let s = unsafe { str_of(s) };
    strs(list, s.char_indices().map(|(i, c)| &s[i..i + c.len_utf8()]))
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
    unsafe { obj::option(t, opt, none, parse_int_opt(s)) }
}

/// `parse_int`'s payload without the `Option` object (C8c1).
///
/// # Safety
/// The module's contract.
pub unsafe fn parse_int_opt(s: *mut Header) -> Option<u64> {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).parse::<i64>().ok().map(|n| n as u64) }
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
    unsafe { obj::option(t, opt, none, strip_prefix_opt(s, p)) }
}

/// `strip_prefix`'s payload, a new `Str`, without the `Option` object (C8c1).
///
/// # Safety
/// The module's contract.
pub unsafe fn strip_prefix_opt(s: *mut Header, p: *mut Header) -> Option<u64> {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).strip_prefix(str_of(p)).map(|r| to_slot(new_str(r))) }
}

/// `Str.strip_suffix(p)`: an `Option<Str>` with descriptor `opt`; `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn strip_suffix(t: &Table, opt: u32, none: *mut Header, s: *mut Header, p: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, strip_suffix_opt(s, p)) }
}

/// `strip_suffix`'s payload, a new `Str`, without the `Option` object (C8c1).
///
/// # Safety
/// The module's contract.
pub unsafe fn strip_suffix_opt(s: *mut Header, p: *mut Header) -> Option<u64> {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).strip_suffix(str_of(p)).map(|r| to_slot(new_str(r))) }
}

/// `Str.find(sub)`: the byte offset of the first match, an `Option<Int>` with descriptor `opt`;
/// `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn find(t: &Table, opt: u32, none: *mut Header, s: *mut Header, sub: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, find_opt(s, sub)) }
}

/// `find`'s payload without the `Option` object (C8c1).
///
/// # Safety
/// The module's contract.
pub unsafe fn find_opt(s: *mut Header, sub: *mut Header) -> Option<u64> {
    // SAFETY: the caller's guarantee.
    unsafe { str_of(s).find(str_of(sub)).map(|i| i as u64) }
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

/// `Str.to_lowercase()`: Rust's Unicode mapping.
///
/// # Safety
/// The module's contract.
pub unsafe fn to_lowercase(s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { new_str(&str_of(s).to_lowercase()) }
}

/// `Str.to_uppercase()`: Rust's Unicode mapping.
///
/// # Safety
/// The module's contract.
pub unsafe fn to_uppercase(s: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { new_str(&str_of(s).to_uppercase()) }
}

/// `sha256()` of a `Str` or `Bytes`: a new `Bytes` of 32.
///
/// # Safety
/// The module's contract.
pub unsafe fn sha256(p: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    new_bytes(&crate::digest::sha256(unsafe { obj::bytes(p) }))
}

/// `Bytes.to_hex()`: lowercase, two digits a byte.
///
/// # Safety
/// The module's contract.
pub unsafe fn to_hex(p: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    new_str(&crate::digest::to_hex(unsafe { obj::bytes(p) }))
}

/// `ct_eq` of two `Str`s or two `Bytes` (digest.rs).
///
/// # Safety
/// The module's contract.
pub unsafe fn ct_eq(a: *mut Header, b: *mut Header) -> bool {
    // SAFETY: the caller's guarantee.
    unsafe { crate::digest::ct_eq(obj::bytes(a), obj::bytes(b)) }
}

/// `bytes.extend(more)`. Returns the `Bytes` to store back, as `push`: `b` itself if it had
/// room, else a larger copy (at least doubled) and `b` freed. `more` is borrowed and is not `b`.
///
/// # Safety
/// The module's contract; `b` is a `Bytes` with count 1, `more` another `Bytes`.
pub unsafe fn extend(t: &Table, b: *mut Header, more: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee; `len` and `cap` are at 8 and 12, with `cap` bytes of room.
    unsafe {
        debug_assert!((*b).rc == 1 && b != more);
        let add = obj::bytes(more);
        let len = *obj::field::<u32>(b, 8) as usize;
        let cap = *obj::field::<u32>(b, 12) as usize;
        let needed = len.checked_add(add.len()).expect("over 4 GiB");
        let len32 = u32::try_from(needed).expect("over 4 GiB");
        let b = if needed <= cap {
            b
        } else {
            let q = obj::new_bytes_cap(desc::BYTES, obj::bytes(b), grown(needed, cap));
            obj::dec(t, b);
            q
        };
        core::ptr::copy_nonoverlapping(add.as_ptr(), obj::field(b, obj::BYTES_DATA + len), add.len());
        *obj::field::<u32>(b, 8) = len32;
        b
    }
}
