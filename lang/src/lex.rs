use crate::{Diag, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Ident(String),
    Keyword(String),
    /// A Rust keyword kio does not use; the parser rejects it by name.
    Reserved(String),
    Int(i64),
    Str(String),
    Sym(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub span: Span,
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "if", "else", "while", "for", "in", "return", "match", "break", "continue", "true", "false",
    "struct", "enum",
];

const RESERVED: &[&str] = &[
    "as", "async", "await", "box", "const", "crate", "dyn", "extern", "impl", "loop", "mod", "move", "pub", "ref",
    "self", "Self", "static", "super", "trait", "type", "unsafe", "use", "where", "yield", "abstract", "become", "do",
    "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "try", "gen", "union",
];

// Two-character symbols come first so that `==` wins over `=`.
const SYMBOLS: &[&str] = &[
    "::", "..", "->", "=>", "+=", "-=", "==", "!=", "<=", ">=", "&&", "||", "(", ")", "{", "}", "[", "]", ",", ";",
    ":", ".", "=", "+", "-", "*", "/", "%", "<", ">", "!", "?",
];

const HINT_STR: &str = "add a closing `\"`";

fn err(start: usize, end: usize, msg: &str, hint: Option<&str>) -> Diag {
    Diag {
        span: Span { start, end },
        msg: msg.to_string(),
        hint: hint.map(str::to_string),
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, Diag> {
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
                return Err(err(i, i + 2, "kio has no block comments", Some("use `//`")));
            }
            b'&' if bytes.get(i + 1) != Some(&b'&') => {
                return Err(err(
                    i,
                    i + 1,
                    "kio has no references",
                    Some("pass by value; use `Str` instead of `&str`"),
                ));
            }
            b'|' if bytes.get(i + 1) != Some(&b'|') => {
                return Err(err(i, i + 1, "kio has no closures", Some("use a `for` loop")));
            }
            b'\'' => {
                return Err(err(i, i + 1, "kio has no chars or lifetimes", Some("use `Str`")));
            }
            b'0'..=b'9' => {
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    return Err(err(start, i + 1, "kio has no floats", Some("use `Int`")));
                }
                match src[start..i].parse::<i64>() {
                    Ok(n) => Kind::Int(n),
                    Err(_) => {
                        let hint = "Int is 64-bit: at most 9223372036854775807";
                        return Err(err(start, i, "integer literal does not fit in `Int` (i64)", Some(hint)));
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
                                '"' => text.push('"'),
                                '\\' => text.push('\\'),
                                _ => {
                                    let msg = format!("unknown escape `\\{esc}`");
                                    return Err(err(
                                        i - 1 - esc.len_utf8(),
                                        i,
                                        &msg,
                                        Some("escapes are \\n \\t \\\" \\\\"),
                                    ));
                                }
                            }
                        }
                        _ => text.push(ch),
                    }
                }
                tokens.push(Token {
                    kind: Kind::Str(text),
                    span: Span { start, end: i },
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
                    let msg = format!("unexpected character `{ch}`");
                    let hint = "remove it, or put it inside a string";
                    return Err(err(i, i + ch.len_utf8(), &msg, Some(hint)));
                }
            },
        };
        tokens.push(Token {
            kind,
            span: Span { start, end: i },
        });
    }
    Ok(tokens)
}
