//! `Float` operations Wasm has no instruction for, with the interpreter's results (Rust's
//! `f64`). `core` has no `f64::round` (it is `std`'s), so `round` works on the bits.

use crate::desc::Table;
use crate::obj::{self, Header};

const SIGN: u64 = 1 << 63;

/// `a % b`: the remainder with `a`'s sign, exact, as Rust's `%` (C's `fmod`).
pub fn rem(a: f64, b: f64) -> f64 {
    a % b
}

/// `round`: the nearest integer, halves away from zero, as Rust's `f64::round`; keeps the
/// sign, so `-0.4` rounds to `-0.0`. Exact for every input: adding half a unit of the lowest
/// integer bit to the magnitude's bits carries into the integer part exactly when the fraction
/// is at least one half, and then the fraction bits are cleared.
pub fn round(x: f64) -> f64 {
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32 - 1023;
    let sign = bits & SIGN;
    if e >= 52 {
        x // an integer already, or NaN or infinite
    } else if e < -1 {
        f64::from_bits(sign) // |x| < 0.5
    } else if e == -1 {
        f64::from_bits(sign | 1f64.to_bits()) // 0.5 <= |x| < 1
    } else {
        let frac = (52 - e) as u32;
        f64::from_bits((bits + (1 << (frac - 1))) & !((1 << frac) - 1))
    }
}

/// `to_int`: `x` truncated toward zero if that is an `Int`, else `None` (NaN, infinite, or
/// out of range), as an `Option<Int>` with descriptor `opt`; `none` as `obj::option`'s.
/// `-2^63` is the lowest such `x` (the next float down is far below `i64::MIN`) and `2^63` the
/// first above, so this range test on `x` is the interpreter's on `trunc(x)`.
///
/// # Safety
/// `none` is null or a `None` of `opt`.
pub unsafe fn to_int(t: &Table, opt: u32, none: *mut Header, x: f64) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, to_int_opt(x)) }
}

/// `to_int`'s payload without the `Option` object (C8c1).
pub fn to_int_opt(x: f64) -> Option<u64> {
    const LIMIT: f64 = 9_223_372_036_854_775_808.0; // 2^63
    (-LIMIT..LIMIT).contains(&x).then_some(x as i64 as u64)
}
