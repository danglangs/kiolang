//! Generates `lang/src/caps_gen.rs`, the checker's capability table, from `caps.wit`.
use anyhow::{bail, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use wit_parser::{FunctionKind, Resolve, Type, TypeDefKind};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn pascal(s: &str) -> String {
    s.split('-').map(|w| w[..1].to_uppercase() + &w[1..]).collect()
}

/// The Rust expression for the kio type of a WIT type.
fn kio(r: &Resolve, t: &Option<Type>) -> Result<String> {
    let Some(t) = t else { return Ok("Ty::Unit".into()) };
    let boxed = |t| Ok::<_, anyhow::Error>(format!("Box::new({})", kio(r, &Some(t))?));
    Ok(match t {
        Type::String => "Ty::Str".into(),
        Type::Bool => "Ty::Bool".into(),
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => "Ty::Int".into(),
        Type::S8 | Type::S16 | Type::S32 | Type::S64 => "Ty::Int".into(),
        Type::Id(id) => match &r.types[*id].kind {
            TypeDefKind::List(Type::U8) => "Ty::Bytes".into(),
            TypeDefKind::List(t) => format!("Ty::List({})", boxed(*t)?),
            TypeDefKind::Option(t) => format!("Ty::Option({})", boxed(*t)?),
            TypeDefKind::Result(x) => {
                let ok = x.ok.map_or(Ok("Ty::Unit".into()), |t| kio(r, &Some(t)))?;
                let Some(err) = x.err else {
                    bail!("unsupported WIT type `result<T>` without an error type")
                };
                format!("Ty::Result(Box::new({ok}), {})", boxed(err)?)
            }
            TypeDefKind::Variant(_) | TypeDefKind::Enum(_) => {
                format!(
                    "Ty::Enum(\"{}\".into(), vec![])",
                    pascal(r.types[*id].name.as_ref().unwrap())
                )
            }
            TypeDefKind::Record(_) => {
                format!(
                    "Ty::Struct(\"{}\".into(), vec![])",
                    pascal(r.types[*id].name.as_ref().unwrap())
                )
            }
            k => bail!("unsupported WIT type `{}`", k.as_str()),
        },
        t => bail!("unsupported WIT type `{t:?}`"),
    })
}

fn main() -> Result<()> {
    let mut r = Resolve::new();
    r.push_file(format!("{ROOT}/caps.wit"))?;
    let (mut caps, mut enums, mut structs) = (String::new(), String::new(), String::new());
    // A scalar, `Str` or `Bytes` as an AST type: a variant's payload or a record's field, which
    // may also be a `List` of one of those.
    let plain = |t: &Type, what: &str| -> Result<String> {
        let scalar = |t: &Type| match kio(&r, &Some(*t))?.strip_prefix("Ty::") {
            Some(n) if !n.contains('(') => Ok(format!("ty(\"{n}\")")),
            _ => bail!("unsupported type on `{what}`: only scalars, strings, bytes and lists of those"),
        };
        match t {
            Type::Id(id) => match &r.types[*id].kind {
                TypeDefKind::List(e) if !matches!(e, Type::U8) => Ok(format!("list({})", scalar(e)?)),
                _ => scalar(t),
            },
            _ => scalar(t),
        }
    };
    for (_, i) in &r.interfaces {
        for (name, &id) in &i.types {
            let name = pascal(name);
            match &r.types[id].kind {
                TypeDefKind::Resource => {
                    let mut ms = String::new();
                    for (f, func) in &i.functions {
                        let FunctionKind::Method(m) = func.kind else {
                            bail!("unsupported function `{f}`: not a resource method")
                        };
                        if m != id {
                            continue;
                        }
                        let ps = func.params.iter().skip(1).map(|p| kio(&r, &Some(p.ty)));
                        let ps = ps.collect::<Result<Vec<_>>>()?.join(", ");
                        let ret = kio(&r, &func.result)?;
                        let f = f.rsplit('.').next().unwrap().replace('-', "_");
                        ms += &format!("method(\"{f}\", vec![{ps}], {ret}),");
                    }
                    caps += &format!("CapType {{ name: \"{name}\".into(), methods: vec![{ms}] }},");
                }
                TypeDefKind::Variant(v) => {
                    let mut cs = String::new();
                    for c in &v.cases {
                        let fields = match &c.ty {
                            None => String::new(),
                            Some(t) => plain(t, &c.name)?,
                        };
                        cs += &format!("case(\"{}\", vec![{fields}]),", pascal(&c.name));
                    }
                    enums +=
                        &format!("Enum {{ name: \"{name}\".into(), tparams: vec![], variants: vec![{cs}], span: at, public: true }},");
                }
                TypeDefKind::Enum(e) => {
                    let mut cs = String::new();
                    for c in &e.cases {
                        cs += &format!("case(\"{}\", vec![]),", pascal(&c.name));
                    }
                    enums +=
                        &format!("Enum {{ name: \"{name}\".into(), tparams: vec![], variants: vec![{cs}], span: at, public: true }},");
                }
                // A record is a built-in struct: the program can name it, read its fields and build one.
                TypeDefKind::Record(rec) => {
                    let mut fs = String::new();
                    for f in &rec.fields {
                        let ty = plain(&f.ty, &f.name)?;
                        fs += &format!("field(\"{}\", {ty}),", f.name.replace('-', "_"));
                    }
                    structs +=
                        &format!("Struct {{ name: \"{name}\".into(), tparams: vec![], fields: vec![{fs}], span: at, public: true }},");
                }
                k => bail!("unsupported WIT declaration `{name}`: {}", k.as_str()),
            }
        }
    }
    let code = format!(
        "// Generated from caps.wit by tools/capgen. Do not edit.\n\
         use crate::ast::*;\nuse crate::check::{{CapTable, CapType, Ty}};\nuse crate::Span;\n\
         pub fn v0() -> CapTable {{\n\
         let at = Span {{ start: 0, end: 0, file: 0 }};\n\
         let ty = |n: &str| Type {{ kind: TypeKind::Named(n.into(), vec![]), span: at }};\n\
         let case = |n: &str, fields: Vec<Type>| Variant {{ name: n.into(), fields, span: at }};\n\
         let field = |n: &str, ty: Type| Param {{ name: n.into(), ty, span: at }};\n\
         let list = |e: Type| Type {{ kind: TypeKind::Named(\"List\".into(), vec![e]), span: at }};\n\
         let method = |n: &str, params, ret| (n.to_string(), params, ret);\n\
         CapTable {{ caps: vec![{caps}], enums: vec![{enums}], structs: vec![{structs}] }}\n}}\n"
    );
    let mut fmt = Command::new("rustfmt")
        .args(["--edition", "2021"])
        .stdin(Stdio::piped())
        .stdout(std::fs::File::create(format!("{ROOT}/lang/src/caps_gen.rs"))?)
        .spawn()?;
    fmt.stdin.take().unwrap().write_all(code.as_bytes())?;
    if !fmt.wait()?.success() {
        bail!("rustfmt failed")
    }
    Ok(())
}
