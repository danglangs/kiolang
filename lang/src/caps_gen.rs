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
    let list = |e: Type| Type {
        kind: TypeKind::Named("List".into(), vec![e]),
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
                methods: vec![
                    method(
                        "get",
                        vec![Ty::Str],
                        Ty::Result(Box::new(Ty::Bytes), Box::new(Ty::Enum("HttpErr".into(), vec![]))),
                    ),
                    method(
                        "send",
                        vec![Ty::Str, Ty::Str, Ty::List(Box::new(Ty::Str)), Ty::Bytes],
                        Ty::Result(
                            Box::new(Ty::Struct("HttpResponse".into(), vec![])),
                            Box::new(Ty::Enum("HttpErr".into(), vec![])),
                        ),
                    ),
                    method(
                        "send_all",
                        vec![Ty::List(Box::new(Ty::Struct("HttpRequest".into(), vec![])))],
                        Ty::List(Box::new(Ty::Result(
                            Box::new(Ty::Struct("HttpResponse".into(), vec![])),
                            Box::new(Ty::Enum("HttpErr".into(), vec![])),
                        ))),
                    ),
                ],
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
            CapType {
                name: "Rng".into(),
                methods: vec![
                    method("bytes", vec![Ty::Int], Ty::Bytes),
                    method("int", vec![Ty::Int, Ty::Int], Ty::Int),
                ],
            },
            CapType {
                name: "Clock".into(),
                methods: vec![method("now_ms", vec![], Ty::Int)],
            },
            CapType {
                name: "Log".into(),
                methods: vec![method(
                    "write",
                    vec![Ty::Str],
                    Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Enum("LogErr".into(), vec![]))),
                )],
            },
            CapType {
                name: "Db".into(),
                methods: vec![
                    method(
                        "query",
                        vec![Ty::Str, Ty::List(Box::new(Ty::Enum("SqlValue".into(), vec![])))],
                        Ty::Result(
                            Box::new(Ty::List(Box::new(Ty::List(Box::new(Ty::Enum(
                                "SqlValue".into(),
                                vec![],
                            )))))),
                            Box::new(Ty::Enum("DbErr".into(), vec![])),
                        ),
                    ),
                    method(
                        "exec",
                        vec![Ty::Str, Ty::List(Box::new(Ty::Enum("SqlValue".into(), vec![])))],
                        Ty::Result(Box::new(Ty::Int), Box::new(Ty::Enum("DbErr".into(), vec![]))),
                    ),
                    method(
                        "transaction",
                        vec![
                            Ty::List(Box::new(Ty::Str)),
                            Ty::List(Box::new(Ty::List(Box::new(Ty::Enum("SqlValue".into(), vec![]))))),
                        ],
                        Ty::Result(Box::new(Ty::Int), Box::new(Ty::Enum("DbErr".into(), vec![]))),
                    ),
                ],
            },
            CapType {
                name: "Hmac".into(),
                methods: vec![
                    method(
                        "sign",
                        vec![Ty::Bytes],
                        Ty::Result(Box::new(Ty::Bytes), Box::new(Ty::Enum("HmacErr".into(), vec![]))),
                    ),
                    method("verify", vec![Ty::Bytes, Ty::Bytes], Ty::Bool),
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
                    case("Invalid", vec![]),
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
            Enum {
                name: "LogErr".into(),
                tparams: vec![],
                variants: vec![case("TooLarge", vec![])],
                span: at,
                public: true,
            },
            Enum {
                name: "SqlValue".into(),
                tparams: vec![],
                variants: vec![
                    case("Null", vec![]),
                    case("Int", vec![ty("Int")]),
                    case("Float", vec![ty("Float")]),
                    case("Text", vec![ty("Str")]),
                    case("Blob", vec![ty("Bytes")]),
                ],
                span: at,
                public: true,
            },
            Enum {
                name: "DbErr".into(),
                tparams: vec![],
                variants: vec![
                    case("Invalid", vec![ty("Str")]),
                    case("Denied", vec![]),
                    case("Busy", vec![]),
                    case("TooLarge", vec![]),
                    case("Constraint", vec![ty("Str")]),
                ],
                span: at,
                public: true,
            },
            Enum {
                name: "HmacErr".into(),
                tparams: vec![],
                variants: vec![case("Denied", vec![])],
                span: at,
                public: true,
            },
        ],
        structs: vec![
            Struct {
                name: "HttpResponse".into(),
                tparams: vec![],
                fields: vec![
                    field("status", ty("Int")),
                    field("headers", list(ty("Str"))),
                    field("body", ty("Bytes")),
                ],
                span: at,
                public: true,
            },
            Struct {
                name: "HttpRequest".into(),
                tparams: vec![],
                fields: vec![
                    field("method", ty("Str")),
                    field("path", ty("Str")),
                    field("headers", list(ty("Str"))),
                    field("body", ty("Bytes")),
                    field("timeout_ms", ty("Int")),
                ],
                span: at,
                public: true,
            },
            Struct {
                name: "KvEntry".into(),
                tparams: vec![],
                fields: vec![field("key", ty("Str")), field("value", ty("Bytes"))],
                span: at,
                public: true,
            },
        ],
    }
}
