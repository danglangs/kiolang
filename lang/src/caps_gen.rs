// Generated from caps.wit by tools/capgen. Do not edit.
use crate::ast::*;
use crate::check::{CapTable, CapType, Ty};
use crate::Span;
pub fn v0() -> CapTable {
    let at = Span { start: 0, end: 0 };
    let ty = |n: &str| Type {
        kind: TypeKind::Named(n.into(), vec![]),
        span: at,
    };
    let case = |n: &str, fields: Vec<Type>| Variant {
        name: n.into(),
        fields,
        span: at,
    };
    let method = |n: &str, params, ret| (n.to_string(), params, ret);
    CapTable {
        caps: vec![
            CapType {
                name: "Dir".into(),
                methods: vec![
                    method(
                        "read",
                        vec![Ty::Str],
                        Ty::Result(Box::new(Ty::Bytes), Box::new(Ty::Enum("IoErr".into()))),
                    ),
                    method(
                        "create",
                        vec![Ty::Str, Ty::Bytes],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("IoErr".into()))),
                    ),
                ],
            },
            CapType {
                name: "Http".into(),
                methods: vec![method(
                    "get",
                    vec![Ty::Str],
                    Ty::Result(Box::new(Ty::Bytes), Box::new(Ty::Enum("HttpErr".into()))),
                )],
            },
            CapType {
                name: "Incoming".into(),
                methods: vec![
                    method("method", vec![], Ty::Str),
                    method("target", vec![], Ty::Str),
                    method("header_names", vec![], Ty::List(Box::new(Ty::Str))),
                    method("header_values", vec![], Ty::List(Box::new(Ty::Str))),
                    method("body", vec![], Ty::Bytes),
                ],
            },
            CapType {
                name: "Outgoing".into(),
                methods: vec![method(
                    "send",
                    vec![
                        Ty::Int,
                        Ty::List(Box::new(Ty::Str)),
                        Ty::List(Box::new(Ty::Str)),
                        Ty::Bytes,
                    ],
                    Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("WebErr".into()))),
                )],
            },
        ],
        enums: vec![
            Enum {
                public: true,
                name: "IoErr".into(),
                variants: vec![
                    case("NotFound", vec![]),
                    case("Denied", vec![]),
                    case("TooLarge", vec![]),
                    case("InvalidPath", vec![]),
                ],
                span: at,
            },
            Enum {
                public: true,
                name: "HttpErr".into(),
                variants: vec![
                    case("Denied", vec![]),
                    case("Timeout", vec![]),
                    case("TooLarge", vec![]),
                    case("Status", vec![ty("Int")]),
                ],
                span: at,
            },
            Enum {
                public: true,
                name: "WebErr".into(),
                variants: vec![case("AlreadySent", vec![]), case("Invalid", vec![])],
                span: at,
            },
        ],
    }
}
