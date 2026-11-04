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
        ],
        enums: vec![
            Enum {
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
                name: "HttpErr".into(),
                variants: vec![
                    case("Denied", vec![]),
                    case("Timeout", vec![]),
                    case("TooLarge", vec![]),
                    case("Status", vec![ty("Int")]),
                ],
                span: at,
            },
        ],
    }
}
