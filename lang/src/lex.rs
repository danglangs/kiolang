use crate::{Diag, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Ident(String),
    Keyword(String),
    /// A Rust keyword kio does not use; the parser rejects it by name.
    Reserved(String),
    Int(i64),
    /// The bits of an `f64`, so that tokens stay `Eq`.
    Float(u64),
    Str(String),
    Sym(&'static str),
    /// An attribute, as `#[name]` or `#![name]` without its arguments: `#[derive(Debug)]` is
    /// `#[derive]`. The parser accepts `#[test]` only.
    Attr(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub span: Span,
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "if", "else", "while", "for", "in", "loop", "return", "match", "break", "continue", "true",
    "false", "struct", "enum", "mod", "pub", "const",
];

const RESERVED: &[&str] = &[
    "as", "async", "await", "box", "crate", "dyn", "extern", "impl", "move", "ref", "self", "Self", "static", "super",
    "trait", "type", "unsafe", "use", "where", "yield", "abstract", "become", "do", "final", "macro", "override",
    "priv", "typeof", "unsized", "virtual", "try", "gen", "union",
];

// Two-character symbols come first so that `==` wins over `=`.
const SYMBOLS: &[&str] = &[
    "::", "..", "->", "=>", "+=", "-=", "==", "!=", "<=", ">=", "&&", "||", "(", ")", "{", "}", "[", "]", ",", ";",
    ":", ".", "=", "+", "-", "*", "/", "%", "<", ">", "!", "?", "|",
];

const HINT_STR: &str = "add a closing `\"`";

fn err(start: usize, end: usize, msg: &str, hint: Option<&str>) -> Diag {
    Diag {
        span: Span { start, end, file: 0 },
        msg: msg.to_string(),
        hint: hint.map(str::to_string),
    }
}

/// Errors `lex_all` steps over at most; the next one ends lexing, so hostile input cannot make
/// the error list large.
const MAX_STEPPED: usize = 100;

fn step_over(errs: &mut Vec<Diag>, d: Diag) -> Result<(), Diag> {
    if errs.len() >= MAX_STEPPED {
        return Err(d);
    }
    errs.push(d);
    Ok(())
}

/// The index after the run of digits at `i`.
fn digits(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i
}

/// Whether `t` can end an operand, so that a `.` after it is a field access, as in `x.0`.
fn ends_operand(t: &Token) -> bool {
    match &t.kind {
        Kind::Ident(_) | Kind::Int(_) | Kind::Float(_) | Kind::Str(_) => true,
        Kind::Keyword(k) => k == "true" || k == "false",
        Kind::Sym(s) => matches!(*s, ")" | "]" | "}" | "?"),
        Kind::Reserved(_) | Kind::Attr(_) => false,
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, Diag> {
    let mut errs = Vec::new();
    let toks = lex_all(src, &mut errs);
    match errs.into_iter().next() {
        Some(d) => Err(d),
        None => toks,
    }
}

/// Like `lex`, stepping over the errors it can: `&` and `&mut` (dropped), a char literal (read
/// as a `Str`), a lifetime (dropped) and a block comment (skipped). Those are pushed to `errs`;
/// any other error ends lexing as `Err`.
pub fn lex_all(src: &str, errs: &mut Vec<Diag>) -> Result<Vec<Token>, Diag> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i];
        let kind = match c {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                step_over(errs, err(i, i + 2, "kio has no block comments", Some("use `//`")))?;
                i = src[i + 2..].find("*/").map_or(src.len(), |j| i + 2 + j + 2);
                continue;
            }
            b'&' if bytes.get(i + 1) != Some(&b'&') => {
                step_over(
                    errs,
                    err(
                        i,
                        i + 1,
                        "kio has no references",
                        Some("pass by value; use `Str` instead of `&str`"),
                    ),
                )?;
                i += 1;
                // `&mut x` reads on as `x`.
                let j = src.len() - src[i..].trim_start().len();
                let word_end = bytes
                    .get(j + 3)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_');
                if src[j..].starts_with("mut") && !word_end {
                    i = j + 3;
                }
                continue;
            }
            b'\'' => {
                step_over(errs, err(i, i + 1, "kio has no chars or lifetimes", Some("use `Str`")))?;
                let mut cs = src[i + 1..].chars();
                let lit = match (cs.next(), cs.next(), cs.next()) {
                    (Some('\\'), Some(c), Some('\'')) => Some(2 + c.len_utf8()),
                    (Some(c), Some('\''), _) if c != '\\' && c != '\'' => Some(c.len_utf8() + 1),
                    _ => None,
                };
                i += 1;
                let Some(n) = lit else {
                    // A lifetime `'a` is dropped.
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                    continue;
                };
                // A char literal reads on as a `Str`.
                i += n;
                Kind::Str(String::new())
            }
            b'.' if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) && !tokens.last().is_some_and(ends_operand) => {
                let end = digits(bytes, i + 1);
                let hint = format!("write `0{}`", &src[start..end]);
                return Err(err(
                    start,
                    end,
                    "a `Float` literal needs a digit before `.`",
                    Some(&hint),
                ));
            }
            b'0'..=b'9' => {
                i = digits(bytes, i);
                let mut float = false;
                if bytes.get(i) == Some(&b'.') {
                    match bytes.get(i + 1) {
                        Some(c) if c.is_ascii_digit() => {
                            i = digits(bytes, i + 1);
                            float = true;
                        }
                        // `0..n` is a range, `1.to_float()` a method call.
                        Some(c) if *c == b'.' || *c == b'_' || c.is_ascii_alphabetic() => {}
                        _ => {
                            let hint = format!("write `{}.0`", &src[start..i]);
                            let msg = "a `Float` literal needs a digit after `.`";
                            return Err(err(start, i + 1, msg, Some(&hint)));
                        }
                    }
                }
                if let Some(b'e' | b'E') = bytes.get(i) {
                    let sign = usize::from(matches!(bytes.get(i + 1), Some(b'+' | b'-')));
                    if bytes.get(i + 1 + sign).is_some_and(u8::is_ascii_digit) {
                        i = digits(bytes, i + 1 + sign);
                        float = true;
                    }
                }
                if float {
                    let x: f64 = src[start..i].parse().expect("a float literal");
                    if !x.is_finite() {
                        let hint = "Float is f64: at most about 1.8e308";
                        return Err(err(start, i, "float literal does not fit in `Float` (f64)", Some(hint)));
                    }
                    Kind::Float(x.to_bits())
                } else {
                    match src[start..i].parse::<i64>() {
                        Ok(n) => Kind::Int(n),
                        Err(_) => {
                            let hint = "Int is 64-bit: at most 9223372036854775807";
                            return Err(err(start, i, "integer literal does not fit in `Int` (i64)", Some(hint)));
                        }
                    }
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = &src[start..i];
                if KEYWORDS.contains(&word) {
                    Kind::Keyword(word.to_string())
                } else if RESERVED.contains(&word) {
                    Kind::Reserved(word.to_string())
                } else {
                    Kind::Ident(word.to_string())
                }
            }
            b'"' => {
                let mut text = String::new();
                let mut chars = src[i + 1..].chars();
                i += 1;
                loop {
                    let Some(ch) = chars.next() else {
                        return Err(err(start, src.len(), "unterminated string", Some(HINT_STR)));
                    };
                    i += ch.len_utf8();
                    match ch {
                        '"' => break,
                        '\\' => {
                            let Some(esc) = chars.next() else {
                                return Err(err(start, src.len(), "unterminated string", Some(HINT_STR)));
                            };
                            i += esc.len_utf8();
                            match esc {
                                'n' => text.push('\n'),
                                't' => text.push('\t'),
                                'r' => text.push('\r'),
                                '0' => text.push('\0'),
                                '"' => text.push('"'),
                                '\\' => text.push('\\'),
                                _ => {
                                    let msg = format!("unknown escape `\\{esc}`");
                                    return Err(err(
                                        i - 1 - esc.len_utf8(),
                                        i,
                                        &msg,
                                        Some("escapes are \\n \\r \\t \\0 \\\" \\\\"),
                                    ));
                                }
                            }
                        }
                        _ => text.push(ch),
                    }
                }
                tokens.push(Token {
                    kind: Kind::Str(text),
                    span: Span { start, end: i, file: 0 },
                });
                continue;
            }
            _ => match SYMBOLS.iter().find(|s| src[i..].starts_with(**s)) {
                Some(s) => {
                    i += s.len();
                    Kind::Sym(s)
                }
                None => {
                    let ch = src[i..].chars().next().unwrap_or(' ');
                    // An attribute `#[..]` or `#![..]` is one token, to its matching `]`.
                    let inner = bytes.get(i + 1) == Some(&b'!');
                    let open = i + 1 + usize::from(inner);
                    if ch != '#' || bytes.get(open) != Some(&b'[') {
                        let msg = format!("unexpected character `{ch}`");
                        return Err(err(
                            i,
                            i + ch.len_utf8(),
                            &msg,
                            Some("remove it, or put it inside a string"),
                        ));
                    }
                    let name: String = src[open + 1..]
                        .trim_start()
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    let mut depth = 0;
                    i = open;
                    while i < bytes.len() {
                        depth += i32::from(bytes[i] == b'[') - i32::from(bytes[i] == b']');
                        i += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Kind::Attr(format!("#{}[{name}]", if inner { "!" } else { "" }))
                }
            },
        };
        tokens.push(Token {
            kind,
            span: Span { start, end: i, file: 0 },
        });
    }
    Ok(tokens)
}
