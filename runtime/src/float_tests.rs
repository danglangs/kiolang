use crate::float;
use crate::obj;
use crate::tests::lock;
use crate::text_tests::{take_option, OPT, T};

/// Edge cases, then the floats around them.
fn cases() -> std::vec::Vec<f64> {
    let base = [
        0.0,
        0.5,
        1.5,
        2.5,
        0.49999999999999994,
        0.5000000000000001,
        4503599627370495.5, // 2^52 - 0.5
        4503599627370496.0, // 2^52
        9007199254740993.0,
        1e300,
        5e-324,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::INFINITY,
        f64::NAN,
        9_223_372_036_854_775_808.0, // 2^63
        9_223_372_036_854_774_784.0, // the float below 2^63
    ];
    let mut out = std::vec::Vec::new();
    for x in base {
        for y in [x, -x] {
            out.extend([
                y,
                f64::from_bits(y.to_bits().wrapping_add(1)),
                f64::from_bits(y.to_bits().wrapping_sub(1)),
            ]);
        }
    }
    // Random bits, and random values near the integers where rounding is decided.
    let mut r: u64 = 1;
    for _ in 0..if cfg!(miri) { 300 } else { 300_000 } {
        r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        out.push(f64::from_bits(r));
        out.push((r >> 11) as f64 / (1u64 << 40) as f64 - 4096.0);
    }
    out
}

#[test]
fn round_is_rusts() {
    for x in cases() {
        let (got, want) = (float::round(x), x.round());
        assert!(
            got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()),
            "{x:e}: {got} vs {want}"
        );
    }
    assert_eq!(float::round(0.49999999999999994), 0.0);
    assert_eq!(float::round(2.5), 3.0);
    assert_eq!(float::round(-2.5), -3.0);
    assert!(float::round(-0.4).is_sign_negative());
}

/// The interpreter's `to_int` (lang/src/eval.rs).
fn interp_to_int(x: f64) -> Option<i64> {
    let t = x.trunc();
    (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0)
        .contains(&t)
        .then_some(t as i64)
}

#[test]
fn to_int_is_the_interpreters() {
    let _g = lock();
    let base = obj::live();
    for x in cases().into_iter().take(if cfg!(miri) { 200 } else { usize::MAX }) {
        // SAFETY: null for a new `None`.
        let p = unsafe { float::to_int(&T, OPT, core::ptr::null_mut(), x) };
        assert_eq!(take_option(p).map(|n| n as i64), interp_to_int(x), "{x:e}");
    }
    assert_eq!(obj::live(), base);
}
