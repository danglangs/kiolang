//! Built-in JSON compiled (C12): `Str.parse_json`, `Json.to_json` and the `Json` accessors over
//! kio objects. The twin of lang/src/json.rs, with the same grammar, numbers, escapes, error
//! messages and encoding; the differential and JSON fuzz tests hold them together.
//!
//! A `Json` is an enum object of the program's `Json` descriptor, its variants in declaration
//! order (`NULL`..`OBJ`). `parse` finds that descriptor, and those of `List<Json>` and
//! `Map<Str, Json>`, through the result descriptor codegen passes, so a program that parses has
//! all three in its table.
//!
//! Parsing recurses once per container, at most `MAX_DEPTH` deep, so its stack is bounded.
//! Encoding loops with its own stack, since a value the program built may nest deeper.
//!
//! Ownership as coll.rs's: arguments are borrowed, results are new. A parse that fails frees
//! what it built. Every `unsafe fn` has the contract of `obj`, its `j` is a `Json`, and its
//! descriptor arguments are what it says.

use crate::desc::{self, Desc, Table};
use crate::obj::{self, from_slot, to_slot, Header, Key, Map, StrKey};
use alloc::vec::Vec;
use core::cell::Cell;
use core::fmt::Write;

/// Containers may nest this deep; one more is an error.
pub const MAX_DEPTH: usize = 128;

pub const NULL: u32 = 0;
pub const BOOL: u32 = 1;
pub const NUM: u32 = 2;
pub const FLOAT: u32 = 3;
pub const STR: u32 = 4;
pub const ARR: u32 = 5;
pub const OBJ: u32 = 6;

/// A message and the byte offset it names.
type Error = (&'static str, usize);

/// The descriptor of payload slot 0 of variant `v` of enum descriptor `d`.
fn payload_desc(t: &Table, d: u32, v: u32) -> u32 {
    match t.desc(d) {
        Desc::Enum { variants, .. } => t.slots(t.variant(variants, v).slots)[0],
        d => panic!("{d:?} is not an enum"),
    }
}

/// `s.parse_json()` as a `Result<Json, Str>` of descriptor `result`.
///
/// # Safety
/// The module's contract; `s` is a `Str`.
pub unsafe fn parse(t: &Table, result: u32, s: *mut Header) -> *mut Header {
    let json = payload_desc(t, result, 0);
    let mut p = Parser {
        t,
        // SAFETY: the caller's guarantee; `s` outlives the parse.
        b: unsafe { obj::bytes(s) },
        json,
        list: payload_desc(t, json, ARR),
        map: payload_desc(t, json, OBJ),
        scratch: Vec::new(),
    };
    let (v, slot) = match p.document() {
        Ok(j) => (0, j),
        Err((msg, at)) => {
            let mut e = alloc::string::String::new();
            let _ = write!(e, "{msg} at byte {at}");
            (1, to_slot(obj::new_bytes(desc::STR, e.as_bytes())))
        }
    };
    // Each variant of a `Result` has one slot; the result owns `slot`.
    obj::variant1(t, result, v, slot)
}

struct Parser<'a, 't> {
    t: &'a Table<'t>,
    b: &'a [u8],
    json: u32,
    list: u32,
    map: u32,
    /// A string with escapes, decoded.
    scratch: Vec<u8>,
}

/// Where a parsed string's bytes are: `b[a..e]` if it had no escapes, else `scratch`.
type Text = Option<(usize, usize)>;

/// Owned `Json` slots, released if a parse fails before they are put in a list.
struct Items<'a, 't> {
    t: &'a Table<'t>,
    v: Vec<u64>,
}

impl Drop for Items<'_, '_> {
    fn drop(&mut self) {
        for &x in &self.v {
            // SAFETY: each is an owned `Json`.
            unsafe { obj::dec(self.t, from_slot(x)) };
        }
    }
}

/// Owned fields, released if a parse fails before they are put in a map.
struct Fields<'a, 't> {
    t: &'a Table<'t>,
    m: Map,
}

impl Drop for Fields<'_, '_> {
    fn drop(&mut self) {
        for x in self.m.values() {
            // SAFETY: each is an owned `Json`.
            unsafe { obj::dec(self.t, from_slot(x.get())) };
        }
    }
}

impl Parser<'_, '_> {
    fn document(&mut self) -> Result<u64, Error> {
        let (v, i) = self.value(self.ws(0), 0)?;
        let end = self.ws(i);
        if end != self.b.len() {
            // SAFETY: `v` is the owned `Json` just built.
            unsafe { obj::dec(self.t, from_slot(v)) };
            return Err(("expected end of input", end));
        }
        Ok(v)
    }

    fn byte(&self, i: usize) -> Option<u8> {
        self.b.get(i).copied()
    }

    fn ws(&self, mut i: usize) -> usize {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.byte(i) {
            i += 1;
        }
        i
    }

    fn digit(&self, i: usize) -> bool {
        self.byte(i).is_some_and(|c| c.is_ascii_digit())
    }

    /// A new `Json` of variant `v`, owning `payload` if it has one.
    fn make(&self, v: u32, payload: Option<u64>) -> u64 {
        // Every variant but `Null` has one slot.
        to_slot(match payload {
            Some(x) => obj::variant1(self.t, self.json, v, x),
            None => obj::alloc(self.t, self.json, v),
        })
    }

    /// The value at `i`, inside `depth` containers, and the offset after it.
    fn value(&mut self, i: usize, depth: usize) -> Result<(u64, usize), Error> {
        match self.byte(i) {
            Some(b'n') => self.literal(i, b"null", "expected 'null'", NULL, None),
            Some(b't') => self.literal(i, b"true", "expected 'true'", BOOL, Some(1)),
            Some(b'f') => self.literal(i, b"false", "expected 'false'", BOOL, Some(0)),
            Some(b'"') => {
                let (text, j) = self.string(i)?;
                let s = obj::new_bytes(desc::STR, self.text(text));
                Ok((self.make(STR, Some(to_slot(s))), j))
            }
            Some(b'[') => self.array(i, depth + 1),
            Some(b'{') => self.object(i, depth + 1),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(i),
            _ => Err(("expected a value", i)),
        }
    }

    fn literal(&self, i: usize, word: &[u8], msg: &'static str, v: u32, p: Option<u64>) -> Result<(u64, usize), Error> {
        match self.b[i..].starts_with(word) {
            true => Ok((self.make(v, p), i + word.len())),
            false => Err((msg, i)),
        }
    }

    /// The offset after one or more digits at `i`.
    fn digits(&self, i: usize) -> Result<usize, Error> {
        if !self.digit(i) {
            return Err(("expected a digit", i));
        }
        let mut j = i;
        while self.digit(j) {
            j += 1;
        }
        Ok(j)
    }

    /// A number: `Num` without fraction or exponent if it fits `Int`, else `Float`.
    fn number(&self, i: usize) -> Result<(u64, usize), Error> {
        let neg = self.byte(i) == Some(b'-');
        let mut j = i + usize::from(neg);
        if !self.digit(j) {
            return Err(("expected a digit", j));
        }
        // The magnitude, `None` once it passes `u64`. A leading 0 ends the integer part.
        let mut mag = Some(0u64);
        if self.byte(j) == Some(b'0') {
            j += 1;
        } else {
            while let Some(c @ b'0'..=b'9') = self.byte(j) {
                // `m * 10 + d` unless it passes `u64::MAX` (`checked_mul` is a 128-bit multiply
                // on wasm32).
                let d = u64::from(c - b'0');
                mag = mag.filter(|&m| m < u64::MAX / 10 || (m == u64::MAX / 10 && d <= u64::MAX % 10));
                mag = mag.map(|m| m * 10 + d);
                j += 1;
            }
        }
        let mut real = false;
        if self.byte(j) == Some(b'.') {
            real = true;
            j = self.digits(j + 1)?;
        }
        if let Some(b'e' | b'E') = self.byte(j) {
            real = true;
            j += 1;
            if let Some(b'+' | b'-') = self.byte(j) {
                j += 1;
            }
            j = self.digits(j)?;
        }
        if !real {
            let n = match (neg, mag) {
                (false, Some(m)) if m <= i64::MAX as u64 => Some(m as i64),
                (true, Some(m)) if m <= 1 << 63 => Some((m as i64).wrapping_neg()),
                _ => None,
            };
            if let Some(n) = n {
                return Ok((self.make(NUM, Some(n as u64)), j));
            }
        }
        // SAFETY: the number's bytes are ASCII.
        let text = unsafe { core::str::from_utf8_unchecked(&self.b[i..j]) };
        let x: f64 = text.parse().expect("JSON number grammar");
        if !x.is_finite() {
            return Err(("number out of range", i));
        }
        Ok((self.make(FLOAT, Some(x.to_bits())), j))
    }

    fn text(&self, t: Text) -> &[u8] {
        match t {
            Some((a, e)) => &self.b[a..e],
            None => &self.scratch,
        }
    }

    /// The string whose opening quote is at `i`, and the offset after its closing quote.
    fn string(&mut self, i: usize) -> Result<(Text, usize), Error> {
        let mut j = plain(self.b, i + 1);
        if self.byte(j) == Some(b'"') {
            return Ok((Some((i + 1, j)), j + 1));
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.b[i + 1..j]);
        loop {
            match self.byte(j) {
                None => return Err(("unterminated string starting", i)),
                Some(b'"') => return Ok((None, j + 1)),
                Some(b'\\') => {}
                Some(_) => return Err(("unescaped control character", j)),
            }
            if self.byte(j + 1) == Some(b'u') {
                let mut cp = self.hex4(j + 2)?;
                j += 6;
                if (0xd800..0xdc00).contains(&cp) {
                    if self.byte(j) != Some(b'\\') || self.byte(j + 1) != Some(b'u') {
                        return Err(("lone surrogate", j - 6));
                    }
                    let lo = self.hex4(j + 2)?;
                    if !(0xdc00..0xe000).contains(&lo) {
                        return Err(("lone surrogate", j - 6));
                    }
                    cp = 0x10000 + (cp - 0xd800) * 0x400 + (lo - 0xdc00);
                    j += 6;
                } else if (0xdc00..0xe000).contains(&cp) {
                    return Err(("lone surrogate", j - 6));
                }
                let c = char::from_u32(cp).expect("not a surrogate");
                self.scratch.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            } else {
                let c = match self.byte(j + 1) {
                    Some(b'"') => b'"',
                    Some(b'\\') => b'\\',
                    Some(b'/') => b'/',
                    Some(b'b') => 8,
                    Some(b'f') => 12,
                    Some(b'n') => b'\n',
                    Some(b'r') => b'\r',
                    Some(b't') => b'\t',
                    _ => return Err(("invalid escape", j)),
                };
                self.scratch.push(c);
                j += 2;
            }
            let run = j;
            j = plain(self.b, j);
            self.scratch.extend_from_slice(&self.b[run..j]);
        }
    }

    /// The four hex digits at `i`, as a number.
    fn hex4(&self, i: usize) -> Result<u32, Error> {
        let mut n = 0;
        for k in 0..4 {
            match self.byte(i + k).and_then(|c| (c as char).to_digit(16)) {
                Some(d) => n = n * 16 + d,
                None => return Err(("expected 4 hex digits", i)),
            }
        }
        Ok(n)
    }

    /// The array whose `[` is at `i`; `depth` includes it.
    fn array(&mut self, i: usize, depth: usize) -> Result<(u64, usize), Error> {
        if depth > MAX_DEPTH {
            return Err(("nesting deeper than 128", i));
        }
        let mut items = Items {
            t: self.t,
            v: Vec::new(),
        };
        let mut j = self.ws(i + 1);
        if self.byte(j) != Some(b']') {
            loop {
                let (v, k) = self.value(j, depth)?;
                items.v.push(v);
                j = self.ws(k);
                match self.byte(j) {
                    Some(b']') => break,
                    Some(b',') => j = self.ws(j + 1),
                    _ => return Err(("expected ',' or ']'", j)),
                }
            }
        }
        let list = obj::new_list(self.list, core::mem::take(&mut items.v));
        Ok((self.make(ARR, Some(to_slot(list))), j + 1))
    }

    /// The object whose `{` is at `i`; `depth` includes it. A repeated key keeps the last value.
    fn object(&mut self, i: usize, depth: usize) -> Result<(u64, usize), Error> {
        if depth > MAX_DEPTH {
            return Err(("nesting deeper than 128", i));
        }
        let mut fields = Fields {
            t: self.t,
            m: Map::new(),
        };
        let mut j = self.ws(i + 1);
        if self.byte(j) != Some(b'}') {
            loop {
                if self.byte(j) != Some(b'"') {
                    return Err(("expected a string key", j));
                }
                let (k, after) = self.string(j)?;
                let key = Key::Str(StrKey::from(self.text(k)));
                j = self.ws(after);
                if self.byte(j) != Some(b':') {
                    return Err(("expected ':'", j));
                }
                let (v, after) = self.value(self.ws(j + 1), depth)?;
                if let Some(old) = fields.m.insert(key, Cell::new(v)) {
                    // SAFETY: the replaced value was owned here.
                    unsafe { obj::dec(self.t, from_slot(old.get())) };
                }
                j = self.ws(after);
                match self.byte(j) {
                    Some(b'}') => break,
                    Some(b',') => j = self.ws(j + 1),
                    _ => return Err(("expected ',' or '}'", j)),
                }
            }
        }
        let map = obj::new_map(self.map, core::mem::take(&mut fields.m));
        Ok((self.make(OBJ, Some(to_slot(map))), j + 1))
    }
}

/// The offset of the first byte from `i` on that ends a string's plain run: `"`, `\`, a
/// control byte, or the end. Eight bytes at a time while eight are left.
fn plain(b: &[u8], mut i: usize) -> usize {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    // A byte of `w` is zero exactly where the high bit of `has_zero(w)` is set, for the lowest
    // such byte (borrows only run upward, so the first match is exact).
    let has_zero = |w: u64| w.wrapping_sub(ONES) & !w & HIGH;
    while i + 8 <= b.len() {
        let w = u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bytes"));
        // Bytes below 0x20: `w - 0x20` borrows into the high bit where the byte is below it.
        let low = w.wrapping_sub(ONES * 0x20) & !w & HIGH;
        let m = has_zero(w ^ (ONES * u64::from(b'"'))) | has_zero(w ^ (ONES * u64::from(b'\\'))) | low;
        if m != 0 {
            return i + (m.trailing_zeros() / 8) as usize;
        }
        i += 8;
    }
    while let Some(&c) = b.get(i) {
        if c == b'"' || c == b'\\' || c < 0x20 {
            break;
        }
        i += 1;
    }
    i
}

/// The variant and payload slot of `Json` `j`; the payload is 0 for `Null`.
unsafe fn parts(j: *mut Header) -> (u32, u64) {
    // SAFETY: the caller's guarantee; an enum has its variant at HEADER, and every `Json`
    // variant but `Null` has one slot.
    unsafe {
        let v = *obj::field::<u32>(j, obj::HEADER);
        (v, if v == NULL { 0 } else { obj::payload(j) })
    }
}

/// `Vec<u8>` as a `fmt::Write`, for numbers.
struct Out(Vec<u8>);

impl Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// An array or object being encoded: the items left, and whether one was written.
enum Frame<'m> {
    Arr(core::slice::Iter<'m, u64>, bool),
    Obj(alloc::collections::btree_map::Iter<'m, Key, Cell<u64>>, bool),
}

/// `j.to_json()` as a new `Str`: compact, object keys in `Map` order.
///
/// # Safety
/// The module's contract.
pub unsafe fn encode(j: *mut Header) -> *mut Header {
    let mut out = Out(Vec::new());
    let mut stack: Vec<Frame> = Vec::new();
    let mut next = Some(j);
    loop {
        if let Some(p) = next.take() {
            // SAFETY: `p` is `j` or a `Json` inside it, which outlives this call unchanged.
            unsafe {
                let (v, x) = parts(p);
                match v {
                    NULL => out.0.extend_from_slice(b"null"),
                    BOOL => out.0.extend_from_slice(if x != 0 { b"true" } else { b"false" }),
                    NUM => out.0.extend_from_slice(crate::text::int_text(x as i64, &mut [0; 20])),
                    FLOAT => float(&mut out, f64::from_bits(x)),
                    STR => quote(&mut out.0, obj::bytes(from_slot(x))),
                    ARR => {
                        out.0.push(b'[');
                        stack.push(Frame::Arr(obj::elems(from_slot(x)).iter(), false));
                    }
                    OBJ => {
                        out.0.push(b'{');
                        let m = &*obj::field::<Map>(from_slot(x), obj::HEADER);
                        stack.push(Frame::Obj(m.iter(), false));
                    }
                    v => panic!("Json variant {v}"),
                }
            }
        }
        match stack.last_mut() {
            None => break,
            Some(Frame::Arr(items, written)) => match items.next() {
                Some(&x) => {
                    if core::mem::replace(written, true) {
                        out.0.push(b',');
                    }
                    next = Some(from_slot(x));
                }
                None => {
                    out.0.push(b']');
                    stack.pop();
                }
            },
            Some(Frame::Obj(items, written)) => match items.next() {
                Some((Key::Str(k), x)) => {
                    if core::mem::replace(written, true) {
                        out.0.push(b',');
                    }
                    quote(&mut out.0, k);
                    out.0.push(b':');
                    next = Some(from_slot(x.get()));
                }
                Some((Key::Int(_), _)) => panic!("an Int key in a Json object"),
                None => {
                    out.0.push(b'}');
                    stack.pop();
                }
            },
        }
    }
    obj::new_bytes(desc::STR, &out.0)
}

/// Rust's `{}`, with `.0` added when it has no `.`, `e` or `E`; NaN and infinities as `null`.
fn float(out: &mut Out, x: f64) {
    if !x.is_finite() {
        out.0.extend_from_slice(b"null");
        return;
    }
    let start = out.0.len();
    let _ = write!(out, "{x}");
    if !out.0[start..].iter().any(|c| matches!(c, b'.' | b'e' | b'E')) {
        out.0.extend_from_slice(b".0");
    }
}

/// A JSON string literal: `"` and `\` escaped, `\n` and `\t` by name, other control bytes as
/// `\u00XX`.
fn quote(out: &mut Vec<u8>, s: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    let mut i = 0;
    loop {
        let j = plain(s, i);
        out.extend_from_slice(&s[i..j]);
        let Some(&c) = s.get(j) else { break };
        match c {
            b'"' | b'\\' => out.extend_from_slice(&[b'\\', c]),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            _ => out.extend_from_slice(&[
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(c >> 4)],
                HEX[usize::from(c & 15)],
            ]),
        }
        i = j + 1;
    }
    out.push(b'"');
}

/// `j.get(k)` as an `Option<Json>` of descriptor `opt`; `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `k` is a `Str`; `none` is null or a `None` of `opt`.
pub unsafe fn get(t: &Table, opt: u32, none: *mut Header, j: *mut Header, k: *mut Header) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, get_opt(j, k)) }
}

/// `get`'s payload, a new reference, without the `Option` object (C8c1).
///
/// # Safety
/// The module's contract; `k` is a `Str`.
pub unsafe fn get_opt(j: *mut Header, k: *mut Header) -> Option<u64> {
    // SAFETY: the caller's guarantee; `k` outlives the probe.
    unsafe {
        let (v, x) = parts(j);
        let found = match v {
            OBJ => (*obj::field::<Map>(from_slot(x), obj::HEADER))
                .get(&*Key::probe(obj::bytes(k)))
                .map(Cell::get),
            _ => None,
        };
        found.inspect(|&f| obj::inc(from_slot(f)))
    }
}

/// `j.at(i)` as an `Option<Json>` of descriptor `opt`; `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `none` is null or a `None` of `opt`.
pub unsafe fn at(t: &Table, opt: u32, none: *mut Header, j: *mut Header, i: i64) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, at_opt(j, i)) }
}

/// `at`'s payload, a new reference, without the `Option` object (C8c1).
///
/// # Safety
/// The module's contract.
pub unsafe fn at_opt(j: *mut Header, i: i64) -> Option<u64> {
    // SAFETY: the caller's guarantee.
    unsafe {
        let (v, x) = parts(j);
        let found = match v {
            ARR => usize::try_from(i)
                .ok()
                .and_then(|i| obj::elems(from_slot(x)).get(i))
                .copied(),
            _ => None,
        };
        found.inspect(|&f| obj::inc(from_slot(f)))
    }
}

/// `as_str`, `as_int`, `as_float`, `as_bool`, `as_list` or `as_map` (by the variant `want`) as
/// an `Option` of descriptor `opt` holding a new reference to the payload; `as_float` also
/// takes a `Num`. `none` as `obj::option`'s.
///
/// # Safety
/// The module's contract; `opt` is `Option` of `want`'s payload type; `none` is null or a
/// `None` of it.
pub unsafe fn as_variant(t: &Table, opt: u32, none: *mut Header, j: *mut Header, want: u32) -> *mut Header {
    // SAFETY: the caller's guarantee.
    unsafe { obj::option(t, opt, none, as_variant_opt(j, want)) }
}

/// `as_variant`'s payload (a new reference when it is an object) without the `Option` object
/// (C8c1).
///
/// # Safety
/// The module's contract.
pub unsafe fn as_variant_opt(j: *mut Header, want: u32) -> Option<u64> {
    // SAFETY: the caller's guarantee.
    unsafe {
        let (v, x) = parts(j);
        match v {
            _ if v == want => {
                if matches!(v, STR | ARR | OBJ) {
                    obj::inc(from_slot(x));
                }
                Some(x)
            }
            NUM if want == FLOAT => Some((x as i64 as f64).to_bits()),
            _ => None,
        }
    }
}
