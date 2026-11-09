//! Built-in JSON (L9): the `Json` enum's declaration, `Str.parse_json` and `Json.to_json`, with
//! kio-web's json.kio rules and messages. runtime/src/json.rs is the compiled twin: the same
//! grammar, numbers, escapes, errors and encoding; the differential tests hold them together.

use crate::ast::{Enum, Type, TypeKind, Variant};
use crate::eval::{Key, Value};
use crate::ir::{EnumId, EnumRef};
use crate::Span;
use std::collections::BTreeMap;
use std::rc::Rc;

/// Containers may nest this deep; one more is an error.
pub const MAX_DEPTH: usize = 128;

/// Variant indexes, in declaration order.
pub const NULL: usize = 0;
pub const BOOL: usize = 1;
pub const NUM: usize = 2;
pub const FLOAT: usize = 3;
pub const STR: usize = 4;
pub const ARR: usize = 5;
pub const OBJ: usize = 6;

/// `enum Json { Null, Bool(Bool), Num(Int), Float(Float), Str(Str), Arr(List<Json>), Obj(Map<Str, Json>) }`,
/// declared before the program's types, as the capability error enums are.
pub(crate) fn decl() -> Enum {
    let at = Span::default();
    let ty = |n: &str, args: Vec<Type>| Type {
        kind: TypeKind::Named(n.into(), args),
        span: at,
    };
    let case = |n: &str, fields: Vec<Type>| Variant {
        name: n.into(),
        fields,
        span: at,
    };
    Enum {
        public: true,
        name: "Json".into(),
        tparams: Vec::new(),
        variants: vec![
            case("Null", vec![]),
            case("Bool", vec![ty("Bool", vec![])]),
            case("Num", vec![ty("Int", vec![])]),
            case("Float", vec![ty("Float", vec![])]),
            case("Str", vec![ty("Str", vec![])]),
            case("Arr", vec![ty("List", vec![ty("Json", vec![])])]),
            case("Obj", vec![ty("Map", vec![ty("Str", vec![]), ty("Json", vec![])])]),
        ],
        span: at,
    }
}

/// `s.parse_json()`: a `Json` value of enum `json`, or the message of the first error, which
/// names its byte offset.
pub fn parse(s: &str, json: EnumId) -> Result<Value, String> {
    let p = Parser {
        s,
        b: s.as_bytes(),
        json: EnumRef::User(json),
    };
    let (v, i) = p.value(p.ws(0), 0)?;
    let end = p.ws(i);
    if end != s.len() {
        return Err(at("expected end of input", end));
    }
    Ok(v)
}

fn at(msg: &str, i: usize) -> String {
    format!("{msg} at byte {i}")
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    json: EnumRef,
}

type Parsed<T> = Result<(T, usize), String>;

impl Parser<'_> {
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

    fn make(&self, v: usize, payload: Value) -> Value {
        Value::Enum(self.json, v, vec![payload])
    }

    /// The value at `i`, inside `depth` containers, and the offset after it.
    fn value(&self, i: usize, depth: usize) -> Parsed<Value> {
        match self.byte(i) {
            Some(b'n') => self.literal(i, "null", Value::Enum(self.json, NULL, Vec::new())),
            Some(b't') => self.literal(i, "true", self.make(BOOL, Value::Bool(true))),
            Some(b'f') => self.literal(i, "false", self.make(BOOL, Value::Bool(false))),
            Some(b'"') => {
                let (s, j) = self.string(i)?;
                Ok((self.make(STR, Value::Str(s.into())), j))
            }
            Some(b'[') => self.array(i, depth + 1),
            Some(b'{') => self.object(i, depth + 1),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(i),
            _ => Err(at("expected a value", i)),
        }
    }

    fn literal(&self, i: usize, word: &str, v: Value) -> Parsed<Value> {
        match self.b[i..].starts_with(word.as_bytes()) {
            true => Ok((v, i + word.len())),
            false => Err(at(&format!("expected '{word}'"), i)),
        }
    }

    /// The offset after one or more digits at `i`.
    fn digits(&self, i: usize) -> Result<usize, String> {
        if !self.digit(i) {
            return Err(at("expected a digit", i));
        }
        let mut j = i;
        while self.digit(j) {
            j += 1;
        }
        Ok(j)
    }

    /// A number: `Num` without fraction or exponent if it fits `Int`, else `Float`.
    fn number(&self, i: usize) -> Parsed<Value> {
        let mut j = i + usize::from(self.byte(i) == Some(b'-'));
        if !self.digit(j) {
            return Err(at("expected a digit", j));
        }
        // A leading 0 ends the integer part: JSON has no leading zeros.
        j = if self.byte(j) == Some(b'0') {
            j + 1
        } else {
            self.digits(j)?
        };
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
        let text = &self.s[i..j];
        if !real {
            if let Ok(n) = text.parse::<i64>() {
                return Ok((self.make(NUM, Value::Int(n)), j));
            }
        }
        let x: f64 = text.parse().expect("JSON number grammar");
        if !x.is_finite() {
            return Err(at("number out of range", i));
        }
        Ok((self.make(FLOAT, Value::Float(x)), j))
    }

    /// The string whose opening quote is at `i`, and the offset after its closing quote.
    fn string(&self, i: usize) -> Parsed<String> {
        let mut out = String::new();
        let mut j = i + 1;
        loop {
            let run = j;
            while let Some(c) = self.byte(j) {
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                j += 1;
            }
            // Runs end at ASCII bytes, so on character boundaries.
            out.push_str(&self.s[run..j]);
            match self.byte(j) {
                None => return Err(at("unterminated string starting", i)),
                Some(b'"') => return Ok((out, j + 1)),
                Some(b'\\') => {}
                Some(_) => return Err(at("unescaped control character", j)),
            }
            if self.byte(j + 1) == Some(b'u') {
                let mut cp = self.hex4(j + 2)?;
                j += 6;
                if (0xd800..0xdc00).contains(&cp) {
                    if self.byte(j) != Some(b'\\') || self.byte(j + 1) != Some(b'u') {
                        return Err(at("lone surrogate", j - 6));
                    }
                    let lo = self.hex4(j + 2)?;
                    if !(0xdc00..0xe000).contains(&lo) {
                        return Err(at("lone surrogate", j - 6));
                    }
                    cp = 0x10000 + (cp - 0xd800) * 0x400 + (lo - 0xdc00);
                    j += 6;
                } else if (0xdc00..0xe000).contains(&cp) {
                    return Err(at("lone surrogate", j - 6));
                }
                out.push(char::from_u32(cp).expect("not a surrogate"));
            } else {
                let c = match self.byte(j + 1) {
                    Some(b'"') => '"',
                    Some(b'\\') => '\\',
                    Some(b'/') => '/',
                    Some(b'b') => '\u{8}',
                    Some(b'f') => '\u{c}',
                    Some(b'n') => '\n',
                    Some(b'r') => '\r',
                    Some(b't') => '\t',
                    _ => return Err(at("invalid escape", j)),
                };
                out.push(c);
                j += 2;
            }
        }
    }

    /// The four hex digits at `i`, as a number.
    fn hex4(&self, i: usize) -> Result<u32, String> {
        let mut n = 0;
        for k in 0..4 {
            match self.byte(i + k).and_then(|c| (c as char).to_digit(16)) {
                Some(d) => n = n * 16 + d,
                None => return Err(at("expected 4 hex digits", i)),
            }
        }
        Ok(n)
    }

    /// The array whose `[` is at `i`; `depth` includes it.
    fn array(&self, i: usize, depth: usize) -> Parsed<Value> {
        if depth > MAX_DEPTH {
            return Err(at(&format!("nesting deeper than {MAX_DEPTH}"), i));
        }
        let mut items = Vec::new();
        let mut j = self.ws(i + 1);
        if self.byte(j) == Some(b']') {
            return Ok((self.make(ARR, Value::List(Rc::new(items))), j + 1));
        }
        loop {
            let (v, k) = self.value(j, depth)?;
            items.push(v);
            j = self.ws(k);
            match self.byte(j) {
                Some(b']') => return Ok((self.make(ARR, Value::List(Rc::new(items))), j + 1)),
                Some(b',') => j = self.ws(j + 1),
                _ => return Err(at("expected ',' or ']'", j)),
            }
        }
    }

    /// The object whose `{` is at `i`; `depth` includes it. A repeated key keeps the last value.
    fn object(&self, i: usize, depth: usize) -> Parsed<Value> {
        if depth > MAX_DEPTH {
            return Err(at(&format!("nesting deeper than {MAX_DEPTH}"), i));
        }
        let mut fields = BTreeMap::new();
        let mut j = self.ws(i + 1);
        if self.byte(j) == Some(b'}') {
            return Ok((self.make(OBJ, Value::Map(Rc::new(fields))), j + 1));
        }
        loop {
            if self.byte(j) != Some(b'"') {
                return Err(at("expected a string key", j));
            }
            let (k, after) = self.string(j)?;
            j = self.ws(after);
            if self.byte(j) != Some(b':') {
                return Err(at("expected ':'", j));
            }
            let (v, after) = self.value(self.ws(j + 1), depth)?;
            fields.insert(Key::Str(k), v);
            j = self.ws(after);
            match self.byte(j) {
                Some(b'}') => return Ok((self.make(OBJ, Value::Map(Rc::new(fields))), j + 1)),
                Some(b',') => j = self.ws(j + 1),
                _ => return Err(at("expected ',' or '}'", j)),
            }
        }
    }
}

/// An array or object being encoded: the items left, and whether one was written.
enum Frame<'a> {
    Arr(std::slice::Iter<'a, Value>, bool),
    Obj(std::collections::btree_map::Iter<'a, Key, Value>, bool),
}

/// `j.to_json()` for a `Json` value: compact, object keys in `Map` order. A loop with its own
/// stack, not recursion, so a value nested deeper than `parse` allows still encodes.
pub fn encode(j: &Value) -> String {
    let mut out = String::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut next = Some(j);
    loop {
        if let Some(v) = next.take() {
            let Value::Enum(_, variant, payload) = v else {
                unreachable!("checked Json");
            };
            match (*variant, payload.first()) {
                (NULL, _) => out.push_str("null"),
                (BOOL, Some(Value::Bool(b))) => out.push_str(if *b { "true" } else { "false" }),
                (NUM, Some(Value::Int(n))) => out.push_str(&n.to_string()),
                (FLOAT, Some(Value::Float(x))) => float(&mut out, *x),
                (STR, Some(Value::Str(s))) => quote(&mut out, s),
                (ARR, Some(Value::List(xs))) => {
                    out.push('[');
                    stack.push(Frame::Arr(xs.iter(), false));
                }
                (OBJ, Some(Value::Map(m))) => {
                    out.push('{');
                    stack.push(Frame::Obj(m.iter(), false));
                }
                _ => unreachable!("checked Json"),
            }
        }
        match stack.last_mut() {
            None => return out,
            Some(Frame::Arr(items, written)) => match items.next() {
                Some(x) => {
                    if std::mem::replace(written, true) {
                        out.push(',');
                    }
                    next = Some(x);
                }
                None => {
                    out.push(']');
                    stack.pop();
                }
            },
            Some(Frame::Obj(items, written)) => match items.next() {
                Some((Key::Str(k), x)) => {
                    if std::mem::replace(written, true) {
                        out.push(',');
                    }
                    quote(&mut out, k);
                    out.push(':');
                    next = Some(x);
                }
                Some((Key::Int(_), _)) => unreachable!("checked Map<Str, Json>"),
                None => {
                    out.push('}');
                    stack.pop();
                }
            },
        }
    }
}

/// Rust's `{}`, with `.0` added when it has no `.`, `e` or `E`, so it parses back as a
/// `Float`; NaN and infinities have no JSON form and become `null`.
fn float(out: &mut String, x: f64) {
    if !x.is_finite() {
        out.push_str("null");
        return;
    }
    let t = x.to_string();
    out.push_str(&t);
    if !t.contains(['.', 'e', 'E']) {
        out.push_str(".0");
    }
}

/// A JSON string literal: `"` and `\` escaped, `\n` and `\t` by name, other control bytes as
/// `\u00XX`.
fn quote(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    let mut run = 0;
    for (i, c) in s.bytes().enumerate() {
        let esc: &str = match c {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\t' => "\\t",
            0..=0x1f => "",
            _ => continue,
        };
        out.push_str(&s[run..i]);
        run = i + 1;
        if esc.is_empty() {
            out.push_str("\\u00");
            out.push(HEX[usize::from(c >> 4)] as char);
            out.push(HEX[usize::from(c & 15)] as char);
        } else {
            out.push_str(esc);
        }
    }
    out.push_str(&s[run..]);
    out.push('"');
}
