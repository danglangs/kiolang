// Generated from caps.wit by tools/capgen. Do not edit.
use crate::ast::*;
use crate::check::{CapTable, CapType, Ty};
use crate::Span;
pub fn v0() -> CapTable {
    let at = Span {
        start: 0,
        end: 0,
        file: 0,
    };
    let ty = |n: &str| Type {
        kind: TypeKind::Named(n.into(), vec![]),
        span: at,
    };
    let case = |n: &str, fields: Vec<Type>| Variant {
        name: n.into(),
        fields,
        span: at,
    };
    let field = |n: &str, ty: Type| Param {
        name: n.into(),
        ty,
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
                        Ty::Result(Box::new(Ty::Bytes), Box::new(Ty::Enum("IoErr".into(), vec![]))),
                    ),
                    method(
                        "create",
                        vec![Ty::Str, Ty::Bytes],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("IoErr".into(), vec![]))),
                    ),
                    method(
                        "list",
                        vec![Ty::Str],
                        Ty::Result(
                            Box::new(Ty::List(Box::new(Ty::Str))),
                            Box::new(Ty::Enum("IoErr".into(), vec![])),
                        ),
                    ),
                    method(
                        "write",
                        vec![Ty::Str, Ty::Bytes],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("IoErr".into(), vec![]))),
                    ),
                    method(
                        "remove",
                        vec![Ty::Str],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("IoErr".into(), vec![]))),
                    ),
                    method(
                        "exists",
                        vec![Ty::Str],
                        Ty::Result(Box::new(Ty::Bool), Box::new(Ty::Enum("IoErr".into(), vec![]))),
                    ),
                ],
            },
            CapType {
                name: "Http".into(),
                methods: vec![method(
                    "get",
                    vec![Ty::Str],
                    Ty::Result(Box::new(Ty::Bytes), Box::new(Ty::Enum("HttpErr".into(), vec![]))),
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
                    Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("WebErr".into(), vec![]))),
                )],
            },
            CapType {
                name: "Kv".into(),
                methods: vec![
                    method(
                        "get",
                        vec![Ty::Str],
                        Ty::Result(
                            Box::new(Ty::Option(Box::new(Ty::Bytes))),
                            Box::new(Ty::Enum("KvErr".into(), vec![])),
                        ),
                    ),
                    method(
                        "set",
                        vec![Ty::Str, Ty::Bytes],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("KvErr".into(), vec![]))),
                    ),
                    method(
                        "delete",
                        vec![Ty::Str],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("KvErr".into(), vec![]))),
                    ),
                    method(
                        "keys",
                        vec![Ty::Str],
                        Ty::Result(
                            Box::new(Ty::List(Box::new(Ty::Str))),
                            Box::new(Ty::Enum("KvErr".into(), vec![])),
                        ),
                    ),
                    method(
                        "swap",
                        vec![
                            Ty::Str,
                            Ty::Option(Box::new(Ty::Bytes)),
                            Ty::Option(Box::new(Ty::Bytes)),
                        ],
                        Ty::Result(Box::new(Ty::Bool), Box::new(Ty::Enum("KvErr".into(), vec![]))),
                    ),
                    method(
                        "scan",
                        vec![Ty::Str, Ty::Option(Box::new(Ty::Str)), Ty::Int],
                        Ty::Result(
                            Box::new(Ty::List(Box::new(Ty::Struct("KvEntry".into(), vec![])))),
                            Box::new(Ty::Enum("KvErr".into(), vec![])),
                        ),
                    ),
                    method(
                        "add",
                        vec![Ty::Str, Ty::Int],
                        Ty::Result(Box::new(Ty::Int), Box::new(Ty::Enum("KvErr".into(), vec![]))),
                    ),
                ],
            },
            CapType {
                name: "WsMessage".into(),
                methods: vec![
                    method("kind", vec![], Ty::Str),
                    method("conn", vec![], Ty::Int),
                    method("data", vec![], Ty::Bytes),
                    method("target", vec![], Ty::Str),
                    method("header", vec![Ty::Str], Ty::Option(Box::new(Ty::Str))),
                ],
            },
            CapType {
                name: "WsSend".into(),
                methods: vec![
                    method(
                        "text",
                        vec![Ty::Str],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("WsErr".into(), vec![]))),
                    ),
                    method(
                        "binary",
                        vec![Ty::Bytes],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("WsErr".into(), vec![]))),
                    ),
                    method(
                        "close",
                        vec![Ty::Int],
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("WsErr".into(), vec![]))),
                    ),
                ],
            },
        ],
        enums: vec![
            Enum {
                name: "IoErr".into(),
                tparams: vec![],
                variants: vec![
                    case("NotFound", vec![]),
                    case("Denied", vec![]),
                    case("TooLarge", vec![]),
                    case("InvalidPath", vec![]),
                    case("Exists", vec![]),
                ],
                span: at,
                public: true,
            },
            Enum {
                name: "HttpErr".into(),
                tparams: vec![],
                variants: vec![
                    case("Denied", vec![]),
                    case("Timeout", vec![]),
                    case("TooLarge", vec![]),
                    case("Status", vec![ty("Int")]),
                ],
                span: at,
                public: true,
            },
            Enum {
                name: "WebErr".into(),
                tparams: vec![],
                variants: vec![case("AlreadySent", vec![]), case("Invalid", vec![])],
                span: at,
                public: true,
            },
            Enum {
                name: "KvErr".into(),
                tparams: vec![],
                variants: vec![
                    case("TooLarge", vec![]),
                    case("Full", vec![]),
                    case("NotInt", vec![]),
                    case("Overflow", vec![]),
                ],
                span: at,
                public: true,
            },
            Enum {
                name: "WsErr".into(),
                tparams: vec![],
                variants: vec![
                    case("Closed", vec![]),
                    case("TooLarge", vec![]),
                    case("Invalid", vec![]),
                ],
                span: at,
                public: true,
            },
        ],
        structs: vec![Struct {
            name: "KvEntry".into(),
            tparams: vec![],
            fields: vec![field("key", ty("Str")), field("value", ty("Bytes"))],
            span: at,
            public: true,
        }],
    }
}
