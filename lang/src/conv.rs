//! `Int.pow`, URL encoding and ISO dates (L29); error texts, `pad_start` and base64 (L31): one source for the interpreter and the compiled
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

/// `iso_utc`'s inverse, read leniently (L35): `YYYY-MM-DD` (midnight UTC), or
/// `YYYY-MM-DDTHH:MM:SS`, then an optional `.` and 1 to 9 digits (cut to milliseconds), then `Z`
/// or an offset `+HH:MM`/`-HH:MM` (converted to UTC). `None` for anything else: an invalid day
/// (`2026-02-30`), a leap second, lowercase `z`, `+0100`, a space for `T`.
pub fn parse_iso_utc(s: &[u8]) -> Option<i64> {
    let num = |at: usize, width: usize| -> Option<i64> {
        let digits = s.get(at..at + width)?;
        (digits.iter()).try_fold(0, |n, &c| c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0')))
    };
    let at = |i: usize, c: u8| s.get(i) == Some(&c);
    if s.len() < 10 || !(at(4, b'-') && at(7, b'-')) {
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
    if !(at(10, b'T') && at(13, b':') && at(16, b':')) {
        return None;
    }
    let (h, min, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if h > 23 || min > 59 || sec > 59 {
        return None;
    }
    // The fraction's first three digits, padded with zeros, are the milliseconds.
    let (mut i, mut ms) = (19, 0);
    if at(i, b'.') {
        let n = s[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if !(1..=9).contains(&n) {
            return None;
        }
        for k in 0..3 {
            ms = ms * 10 + if k < n { i64::from(s[i + 1 + k] - b'0') } else { 0 };
        }
        i += 1 + n;
    }
    let offset = match &s[i..] {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let (oh, om) = (num(i + 1, 2)?, num(i + 4, 2)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let o = (oh * 60 + om) * 60_000;
            if *sign == b'+' {
                o
            } else {
                -o
            }
        }
        _ => return None,
    };
    Some(day + ((h * 60 + min) * 60 + sec) * 1000 + ms - offset)
}

/// A header line's value (L35): `line` is `name: value` (`HttpResponse.headers`); the value
/// when the name before the first `:` equals `name` ignoring ASCII case, trimmed of spaces and
/// tabs.
pub fn header_value<'a>(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let colon = line.iter().position(|&c| c == b':')?;
    line[..colon]
        .eq_ignore_ascii_case(name)
        .then(|| trim_ows(&line[colon + 1..]))
}

/// `s` without leading and trailing spaces and tabs (HTTP's optional whitespace).
pub fn trim_ows(s: &[u8]) -> &[u8] {
    let ows = |c: &u8| *c == b' ' || *c == b'\t';
    let start = s.iter().position(|c| !ows(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !ows(c)).map_or(start, |e| e + 1);
    &s[start..end]
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

/// Why `s.pad_start(width, fill)` stopped the run (L31b).
#[derive(Debug, PartialEq)]
pub enum PadErr {
    /// `width` is negative.
    Width,
    /// `fill` is empty.
    Fill,
    /// The last repetition of `fill` would be cut inside a character.
    Cut,
}

/// `pad_start`'s errors; `{a}` is the width.
/// The largest width `pad_start` takes: the largest `Str` a 32-bit memory can hold.
pub const PAD_MAX: i64 = i32::MAX as i64;
pub const PAD_WIDTH: &str = "Str.pad_start({a}, ..): the width must be 0 to 2147483647";
pub const PAD_FILL: &str = "Str.pad_start: the fill must not be empty";
pub const PAD_CUT: &str = "Str.pad_start: the fill would be cut inside a character";

/// `s.pad_start(width, fill)` (L31b): `fill` repeated in front of `s` until the result is
/// `width` bytes (`Str.len()`'s unit), the last repetition cut, as JavaScript's `padStart`; `s`
/// when it is already that long. `fill` is UTF-8, so a cut is refused unless it falls on a
/// character boundary.
pub fn pad_start(s: &[u8], width: i64, fill: &[u8]) -> Result<Vec<u8>, PadErr> {
    if !(0..=PAD_MAX).contains(&width) {
        return Err(PadErr::Width);
    }
    if fill.is_empty() {
        return Err(PadErr::Fill);
    }
    let Some(n) = (width as usize).checked_sub(s.len()) else {
        return Ok(s.to_vec());
    };
    // A UTF-8 continuation byte is `10xxxxxx`.
    if fill.get(n % fill.len()).is_some_and(|&c| c & 0xC0 == 0x80) {
        return Err(PadErr::Cut);
    }
    let mut out: Vec<u8> = fill.iter().copied().cycle().take(n).collect();
    out.extend_from_slice(s);
    Ok(out)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `b.to_base64()` (RFC 4648 section 4, padded) or, if `url`, `b.to_base64url()` (section 5,
/// without padding).
pub fn to_base64(b: &[u8], url: bool) -> String {
    let abc = if url { BASE64_URL } else { BASE64 };
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for chunk in b.chunks(3) {
        let n = (chunk.iter().enumerate()).fold(0u32, |n, (i, &c)| n | (u32::from(c) << (16 - 8 * i)));
        // One byte gives 2 characters, two give 3, three give 4.
        for i in 0..=chunk.len() {
            out.push(abc[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        if !url {
            for _ in chunk.len()..3 {
                out.push('=');
            }
        }
    }
    out
}

/// `to_base64`'s inverse, strict: `None` on a character outside the alphabet (whitespace
/// included), padding that is missing or wrong (`url`: any `=`), or non-zero bits after the
/// last byte.
pub fn from_base64(s: &[u8], url: bool) -> Option<Vec<u8>> {
    let sextet = |c: u8| -> Option<u32> {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' if !url => 62,
            b'/' if !url => 63,
            b'-' if url => 62,
            b'_' if url => 63,
            _ => return None,
        };
        Some(u32::from(v))
    };
    let body = match url {
        true => s,
        false if !s.len().is_multiple_of(4) => return None,
        false => {
            let pad = s.iter().rev().take(2).take_while(|&&c| c == b'=').count();
            &s[..s.len() - pad]
        }
    };
    if body.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    for chunk in body.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= sextet(*c)? << (18 - 6 * i);
        }
        let bytes = chunk.len() - 1;
        // The bits below the last whole byte must be zero.
        if n & ((1 << (24 - 8 * bytes)) - 1) != 0 {
            return None;
        }
        out.extend_from_slice(&n.to_be_bytes()[1..=bytes]);
    }
    Some(out)
}
