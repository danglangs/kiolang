//! `Int.pow`, URL encoding and ISO dates (L29): one source for the interpreter and the compiled
//! runtime, which includes this file by path. `core` and `alloc` only, no `unsafe`.

use alloc::string::String;
use alloc::vec::Vec;

/// `a.pow(b)`: `None` on overflow; `b` is 0 or more (the caller checks).
pub fn pow(a: i64, b: i64) -> Option<i64> {
    // Squaring: once the base overflows with bits of `b` left, the result would too.
    let (mut r, mut base, mut e) = (1i64, a, b);
    while e > 0 {
        if e & 1 == 1 {
            r = r.checked_mul(base)?;
        }
        e >>= 1;
        if e > 0 {
            base = base.checked_mul(base)?;
        }
    }
    Some(r)
}

/// `%XX` (either case) as its byte; `None` on a `%` without two hex digits after it. `+` stays.
pub fn url_decode(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(crate::digest::from_hex(hex)?[0]);
            i += 3;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    Some(out)
}

/// A query string's or form body's decoding: `+` is a space, then `url_decode` (`%2B` is `+`).
pub fn form_decode(s: &[u8]) -> Option<Vec<u8>> {
    let spaced: Vec<u8> = s.iter().map(|&b| if b == b'+' { b' ' } else { b }).collect();
    url_decode(&spaced)
}

/// Every byte outside RFC 3986's unreserved set (`A-Z a-z 0-9 - . _ ~`) as `%XX`, uppercase.
pub fn url_encode(s: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(DIGITS[(b >> 4) as usize] as char);
            out.push(DIGITS[(b & 15) as usize] as char);
        }
    }
    out
}

const DAY_MS: i64 = 86_400_000;

/// Days from 1970-01-01 to `y-m-d`, proleptic Gregorian (Hinnant's `days_from_civil`).
const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The first and last millisecond `iso_utc` shows: 0000-01-01 and 9999-12-31T23:59:59.999Z.
pub const ISO_MIN_MS: i64 = days_from_civil(0, 1, 1) * DAY_MS;
pub const ISO_MAX_MS: i64 = days_from_civil(10_000, 1, 1) * DAY_MS - 1;

/// Unix milliseconds `ms` as `YYYY-MM-DDTHH:MM:SS.mmmZ`; `None` outside years 0000 to 9999.
pub fn iso_utc(ms: i64) -> Option<String> {
    if !(ISO_MIN_MS..=ISO_MAX_MS).contains(&ms) {
        return None;
    }
    // Hinnant's `civil_from_days`: eras of 400 years from 0000-03-01.
    let (z, t) = (ms.div_euclid(DAY_MS) + 719_468, ms.rem_euclid(DAY_MS));
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let mut s = String::with_capacity(24);
    let mut put = |n: i64, width: u32, after: char| {
        for k in (0..width).rev() {
            s.push((b'0' + (n / 10i64.pow(k) % 10) as u8) as char);
        }
        s.push(after);
    };
    put(y, 4, '-');
    put(m, 2, '-');
    put(d, 2, 'T');
    put(t / 3_600_000, 2, ':');
    put(t / 60_000 % 60, 2, ':');
    put(t / 1000 % 60, 2, '.');
    put(t % 1000, 3, 'Z');
    Some(s)
}

/// `iso_utc`'s inverse: exactly `YYYY-MM-DDTHH:MM:SS.mmmZ`, or `YYYY-MM-DD` (midnight UTC);
/// `None` for anything else, an invalid day (`2026-02-30`) included.
pub fn parse_iso_utc(s: &[u8]) -> Option<i64> {
    let num = |at: usize, width: usize| -> Option<i64> {
        let digits = s.get(at..at + width)?;
        (digits.iter()).try_fold(0, |n, &c| c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0')))
    };
    let at = |i: usize, c: u8| s.get(i) == Some(&c);
    if !(s.len() == 10 || s.len() == 24) || !(at(4, b'-') && at(7, b'-')) {
        return None;
    }
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&m) || !(1..=days[m as usize - 1]).contains(&d) {
        return None;
    }
    let day = days_from_civil(y, m, d) * DAY_MS;
    if s.len() == 10 {
        return Some(day);
    }
    let seps = [(10, b'T'), (13, b':'), (16, b':'), (19, b'.'), (23, b'Z')];
    if !seps.iter().all(|&(i, c)| at(i, c)) {
        return None;
    }
    let (h, min, sec, ms) = (num(11, 2)?, num(14, 2)?, num(17, 2)?, num(20, 3)?);
    if h > 23 || min > 59 || sec > 59 {
        return None;
    }
    Some(day + ((h * 60 + min) * 60 + sec) * 1000 + ms)
}

/// The capability error enums with `to_str` (L31); the runtime numbers them in this order.
pub const ERR_ENUMS: [&str; 5] = ["IoErr", "HttpErr", "WebErr", "KvErr", "DbErr"];

/// Each enum's variant texts, in declaration order (caps.wit); language.md lists them.
const ERR_TEXTS: [&[&str]; 5] = [
    &["not found", "denied", "too large", "invalid path", "already exists"],
    &[
        "denied",
        "timed out",
        "too large",
        "status",
        "invalid request",
        "unreachable",
    ],
    &["already sent", "invalid response"],
    &["too large", "store full", "not an integer", "overflow"],
    &["invalid", "denied", "busy", "too large", "constraint"],
];

/// A payload, as `err_to_str` shows it.
pub enum ErrPayload<'a> {
    None,
    Int(i64),
    Str(&'a [u8]),
}

/// What variant `v` of enum `e` (`ERR_ENUMS`' index) holds: 1 for `HttpErr::Status`'s `Int`,
/// 2 for `DbErr::Invalid`'s and `Constraint`'s message, else 0.
pub fn err_payload_kind(e: usize, v: usize) -> u8 {
    match (e, v) {
        (1, 3) => 1,
        (4, 0 | 4) => 2,
        _ => 0,
    }
}

/// `to_str` of variant `v` of enum `e`: its text, then an `Int` payload after a space or a
/// message after `: ` (`status 404`, `constraint: UNIQUE constraint failed: t.id`).
pub fn err_to_str(e: usize, v: usize, payload: ErrPayload) -> Vec<u8> {
    let mut out = ERR_TEXTS[e][v].as_bytes().to_vec();
    match payload {
        ErrPayload::None => {}
        ErrPayload::Int(n) => out.extend_from_slice(alloc::format!(" {n}").as_bytes()),
        ErrPayload::Str(s) => {
            out.extend_from_slice(b": ");
            out.extend_from_slice(s);
        }
    }
    out
}
