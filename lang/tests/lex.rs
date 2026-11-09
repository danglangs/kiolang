use lang::lex::{lex, Kind};
use lang::Span;

fn kinds(src: &str) -> Vec<Kind> {
    lex(src).unwrap().into_iter().map(|t| t.kind).collect()
}

fn ident(s: &str) -> Kind {
    Kind::Ident(s.to_string())
}

fn kw(s: &str) -> Kind {
    Kind::Keyword(s.to_string())
}

#[test]
fn identifiers() {
    assert_eq!(
        kinds("a _b C9 x_1"),
        [ident("a"), ident("_b"), ident("C9"), ident("x_1")]
    );
}

#[test]
fn keywords() {
    let src = "fn let mut if else while for in return match break continue true false struct enum";
    let toks = kinds(src);
    assert_eq!(toks.len(), 16);
    assert!(toks.iter().all(|k| matches!(k, Kind::Keyword(_))));
    assert_eq!(toks[0], kw("fn"));
    assert_eq!(toks[15], kw("enum"));
    assert_eq!(kinds("mod pub loop"), [kw("mod"), kw("pub"), kw("loop")]);
}

#[test]
fn reserved_words() {
    let src = "as async await box const crate dyn extern impl move ref self Self \
               static super trait type unsafe use where yield abstract become do final macro \
               override priv typeof unsized virtual try gen union";
    let toks = kinds(src);
    assert_eq!(toks.len(), 34);
    assert!(toks.iter().all(|k| matches!(k, Kind::Reserved(_))));
    assert_eq!(kinds("impl"), [Kind::Reserved("impl".into())]);
    assert_eq!(kinds("implx"), [ident("implx")]);
}

#[test]
fn int_literals() {
    assert_eq!(
        kinds("0 42 9223372036854775807"),
        [Kind::Int(0), Kind::Int(42), Kind::Int(i64::MAX)]
    );
}

#[test]
fn int_too_large() {
    let d = lex("9223372036854775808").unwrap_err();
    assert_eq!(
        d.span,
        Span {
            start: 0,
            end: 19,
            file: 0
        }
    );
    assert!(d.msg.contains("does not fit"));
    assert_eq!(d.hint.as_deref(), Some("Int is 64-bit: at most 9223372036854775807"));
}

#[test]
fn str_literals() {
    assert_eq!(
        kinds(r#""" "a b" "x\ny\t\"\\" "é" "\r\0""#),
        [
            Kind::Str("".into()),
            Kind::Str("a b".into()),
            Kind::Str("x\ny\t\"\\".into()),
            Kind::Str("é".into()),
            Kind::Str("\r\0".into()),
        ]
    );
}

#[test]
fn bad_escape() {
    let d = lex(r#""a\qb""#).unwrap_err();
    assert_eq!(
        d.span,
        Span {
            start: 2,
            end: 4,
            file: 0
        }
    );
    assert_eq!(d.msg, "unknown escape `\\q`");
    assert_eq!(d.hint.as_deref(), Some("escapes are \\n \\r \\t \\0 \\\" \\\\"));
    let d = lex(r#""\u{41}""#).unwrap_err();
    assert_eq!(d.msg, "unknown escape `\\u`");
}

#[test]
fn unterminated_string() {
    let d = lex("let s = \"abc").unwrap_err();
    assert_eq!(
        d.span,
        Span {
            start: 8,
            end: 12,
            file: 0
        }
    );
    assert_eq!(d.msg, "unterminated string");
    assert_eq!(d.hint.as_deref(), Some("add a closing `\"`"));
    let d = lex("\"abc\\").unwrap_err();
    assert_eq!(d.msg, "unterminated string");
    assert_eq!(d.hint.as_deref(), Some("add a closing `\"`"));
}

#[test]
fn symbols() {
    let src = "( ) { } [ ] , ; : :: . .. -> => = += -= + - * / % == != < <= > >= && || ! ?";
    let toks = kinds(src);
    assert_eq!(toks.len(), 32);
    assert!(toks.iter().all(|k| matches!(k, Kind::Sym(_))));
    let names: Vec<_> = toks
        .iter()
        .map(|k| if let Kind::Sym(s) = k { *s } else { "" })
        .collect();
    assert_eq!(names.join(" "), src);
}

#[test]
fn symbols_without_spaces() {
    assert_eq!(
        kinds("a<=b==c::d"),
        [
            ident("a"),
            Kind::Sym("<="),
            ident("b"),
            Kind::Sym("=="),
            ident("c"),
            Kind::Sym("::"),
            ident("d")
        ]
    );
}

#[test]
fn range_is_int_dotdot_ident() {
    assert_eq!(kinds("0..n"), [Kind::Int(0), Kind::Sym(".."), ident("n")]);
}

fn float(x: f64) -> Kind {
    Kind::Float(x.to_bits())
}

#[test]
fn float_literals() {
    assert_eq!(kinds("1.5 2.0 0.25"), [float(1.5), float(2.0), float(0.25)]);
    assert_eq!(
        kinds("1e9 1.5e-3 2E+4 7e0"),
        [float(1e9), float(1.5e-3), float(2e4), float(7.0)]
    );
    assert_eq!(kinds("1e-400"), [float(0.0)]);
    assert_eq!(kinds("-1.5"), [Kind::Sym("-"), float(1.5)]);
    // Not floats: a range, a method on an `Int`, a field after a name, an `e` that is a name.
    assert_eq!(kinds("1..2"), [Kind::Int(1), Kind::Sym(".."), Kind::Int(2)]);
    assert_eq!(
        kinds("1.to_float()")[..3],
        [Kind::Int(1), Kind::Sym("."), ident("to_float")]
    );
    assert_eq!(kinds("x.5"), [ident("x"), Kind::Sym("."), Kind::Int(5)]);
    assert_eq!(kinds("xs.0"), [ident("xs"), Kind::Sym("."), Kind::Int(0)]);
    assert_eq!(kinds("1e"), [Kind::Int(1), ident("e")]);
    assert_eq!(kinds("1.5.to_str"), [float(1.5), Kind::Sym("."), ident("to_str")]);
}

#[test]
fn float_literal_errors() {
    let cases = [
        ("1.", 0, 2, "a `Float` literal needs a digit after `.`", "write `1.0`"),
        (
            "f(12.)",
            2,
            5,
            "a `Float` literal needs a digit after `.`",
            "write `12.0`",
        ),
        (".5", 0, 2, "a `Float` literal needs a digit before `.`", "write `0.5`"),
        (
            "x = .25;",
            4,
            7,
            "a `Float` literal needs a digit before `.`",
            "write `0.25`",
        ),
        (
            "1e309",
            0,
            5,
            "float literal does not fit in `Float` (f64)",
            "Float is f64: at most about 1.8e308",
        ),
    ];
    for (src, start, end, msg, hint) in cases {
        let d = lex(src).unwrap_err();
        assert_eq!((d.span.start, d.span.end), (start, end), "{src}");
        assert_eq!(d.msg, msg, "{src}");
        assert_eq!(d.hint.as_deref(), Some(hint), "{src}");
    }
}

#[test]
fn whitespace_and_comments() {
    assert_eq!(kinds("a // x \"y\n\t b // end"), [ident("a"), ident("b")]);
    assert_eq!(kinds("a / b"), [ident("a"), Kind::Sym("/"), ident("b")]);
}

#[test]
fn spans_are_byte_offsets() {
    let toks = lex("\"é\" x").unwrap();
    assert_eq!(
        toks[0].span,
        Span {
            start: 0,
            end: 4,
            file: 0
        }
    );
    assert_eq!(
        toks[1].span,
        Span {
            start: 5,
            end: 6,
            file: 0
        }
    );
}

#[track_caller]
fn rejects(src: &str, span: (usize, usize), msg: &str, hint: Option<&str>) {
    let d = lex(src).unwrap_err();
    assert_eq!(
        d.span,
        Span {
            start: span.0,
            end: span.1,
            file: 0
        }
    );
    assert_eq!(d.msg, msg);
    assert_eq!(d.hint.as_deref(), hint);
}

#[test]
fn rejects_reference() {
    rejects(
        "x: &Str",
        (3, 4),
        "kio has no references",
        Some("pass by value; use `Str` instead of `&str`"),
    );
}

/// `|` is a symbol; the parser decides whether it starts a closure. `||` stays one token.
#[test]
fn bar_and_double_bar() {
    use Kind::Sym;
    assert_eq!(kinds("|x| ||"), [Sym("|"), ident("x"), Sym("|"), Sym("||")]);
    assert_eq!(kinds("| |"), [Sym("|"), Sym("|")]);
}

#[test]
fn rejects_char_or_lifetime() {
    rejects("'a'", (0, 1), "kio has no chars or lifetimes", Some("use `Str`"));
}

#[test]
fn rejects_block_comment() {
    rejects("a /* x */", (2, 4), "kio has no block comments", Some("use `//`"));
}

#[test]
fn rejects_other_character() {
    let hint = Some("remove it, or put it inside a string");
    rejects("a @ b", (2, 3), "unexpected character `@`", hint);
    rejects("a é", (2, 4), "unexpected character `é`", hint);
}
