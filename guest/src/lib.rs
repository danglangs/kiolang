// The generated `run` export lowers to sixteen arguments: a string and seven lists.
#![allow(clippy::too_many_arguments)]
wit_bindgen::generate!({ world: "guest", path: "../caps.wit" });

use kio::caps::fs::IoErr;
use kio::caps::kv::KvErr;
use kio::caps::net::HttpErr;
use kio::caps::web::WebErr;
use kio::caps::ws::WsErr;
use lang::ir::{EnumId, EnumRef};
use lang::{CapTable, Value};
use std::cell::RefCell;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

thread_local! {
    /// The files `init` checked and their program, until a `run` on the same files takes it.
    static CHECKED: RefCell<Option<(Vec<File>, lang::Program)>> = const { RefCell::new(None) };
}

struct Component;

impl Guest for Component {
    fn check(files: Vec<File>) -> Result<Report, Vec<Diag>> {
        check_files(&files).map(|program| Report {
            lines: lang::report(&program).lines().map(String::from).collect(),
        })
    }

    fn init(files: Vec<File>) -> Result<(), Vec<Diag>> {
        let program = check_files(&files)?;
        CHECKED.set(Some((files, program)));
        Ok(())
    }

    fn run(
        files: Vec<File>,
        dirs: Vec<DirGrant>,
        https: Vec<HttpGrant>,
        incomings: Vec<IncomingGrant>,
        outgoings: Vec<OutgoingGrant>,
        kvs: Vec<KvGrant>,
        messages: Vec<WsMessageGrant>,
        senders: Vec<WsSendGrant>,
    ) -> Result<Outcome, Vec<Diag>> {
        let program = match CHECKED.take() {
            Some((checked, program)) if same_files(&checked, &files) => program,
            _ => check_files(&files)?,
        };
        let mut grants = Grants(BTreeMap::new());
        let mut errs = Vec::new();
        let names: Vec<String> = program.main_params().into_iter().map(|(n, _)| n).collect();
        let named = dirs
            .into_iter()
            .map(|g| (g.name, Grant::Dir(g.dir)))
            .chain(https.into_iter().map(|g| (g.name, Grant::Http(g.http))))
            .chain(incomings.into_iter().map(|g| (g.name, Grant::Incoming(g.incoming))))
            .chain(outgoings.into_iter().map(|g| (g.name, Grant::Outgoing(g.outgoing))))
            .chain(kvs.into_iter().map(|g| (g.name, Grant::Kv(g.kv))))
            .chain(messages.into_iter().map(|g| (g.name, Grant::WsMessage(g.message))))
            .chain(senders.into_iter().map(|g| (g.name, Grant::WsSend(g.sender))));
        for (name, grant) in named {
            if !names.contains(&name) {
                let hint = format!("grant only `main`'s parameters: {}", names.join(", "));
                errs.push(grant_diag(format!("grant `{name}` is not a parameter of `main`"), hint));
            } else if let Entry::Vacant(slot) = grants.0.entry(name.clone()) {
                slot.insert(grant);
            } else {
                let hint = "give each grant one name".to_string();
                errs.push(grant_diag(format!("grant `{name}` is given more than once"), hint));
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        match lang::run(&program, &mut grants) {
            Ok(v) => Ok(Outcome {
                report: Report {
                    lines: lang::report(&program).lines().map(String::from).collect(),
                },
                // A failed main sends its error alone, so `Err("bad")` reads `bad`, as Rust's Display would.
                failed: matches!(&v, Value::Enum(EnumRef::Result, 1, _)),
                value: match &v {
                    Value::Enum(EnumRef::Result, 1, payload) => program.display(&payload[0]),
                    _ => program.display(&v),
                },
            }),
            Err(e) => Err(vec![Diag {
                start: e.span.start as u32,
                end: e.span.end as u32,
                hint: grant_hint(&program, &e.msg),
                message: e.msg,
                file: e.span.file,
            }]),
        }
    }
}

enum Grant {
    Dir(Dir),
    Http(Http),
    Incoming(Incoming),
    Outgoing(Outgoing),
    Kv(Kv),
    WsMessage(WsMessage),
    WsSend(WsSend),
}

/// The handles granted for one run, by `main` parameter name.
struct Grants(BTreeMap<String, Grant>);

impl lang::Caps for Grants {
    fn has(&self, name: &str, ty: &str) -> bool {
        matches!(
            (self.0.get(name), ty),
            (Some(Grant::Dir(_)), "Dir")
                | (Some(Grant::Http(_)), "Http")
                | (Some(Grant::Incoming(_)), "Incoming")
                | (Some(Grant::Outgoing(_)), "Outgoing")
                | (Some(Grant::Kv(_)), "Kv")
                | (Some(Grant::WsMessage(_)), "WsMessage")
                | (Some(Grant::WsSend(_)), "WsSend")
        )
    }

    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let grant = self.0.get(cap).ok_or_else(|| format!("no grant `{cap}`"))?;
        match (grant, method, args.as_slice()) {
            (Grant::Dir(d), "read", [Value::Str(path)]) => {
                Ok(result(d.read(path).map(|b| Value::Bytes(b.into())).map_err(io_err)))
            }
            (Grant::Dir(d), "create", [Value::Str(path), Value::Bytes(data)]) => {
                Ok(result(d.create(path, data).map(|()| Value::Unit).map_err(io_err)))
            }
            (Grant::Dir(d), "list", [Value::Str(path)]) => Ok(result(d.list(path).map(strs).map_err(io_err))),
            (Grant::Dir(d), "write", [Value::Str(path), Value::Bytes(data)]) => {
                Ok(result(d.write(path, data).map(|()| Value::Unit).map_err(io_err)))
            }
            (Grant::Dir(d), "remove", [Value::Str(path)]) => {
                Ok(result(d.remove(path).map(|()| Value::Unit).map_err(io_err)))
            }
            (Grant::Dir(d), "exists", [Value::Str(path)]) => {
                Ok(result(d.exists(path).map(Value::Bool).map_err(io_err)))
            }
            (Grant::Http(h), "get", [Value::Str(path)]) => {
                Ok(result(h.get(path).map(|b| Value::Bytes(b.into())).map_err(http_err)))
            }
            (Grant::Incoming(i), "method", []) => Ok(Value::Str(i.method().into())),
            (Grant::Incoming(i), "target", []) => Ok(Value::Str(i.target().into())),
            (Grant::Incoming(i), "header_names", []) => Ok(strs(i.header_names())),
            (Grant::Incoming(i), "header_values", []) => Ok(strs(i.header_values())),
            (Grant::Incoming(i), "body", []) => Ok(Value::Bytes(i.body().into())),
            (
                Grant::Outgoing(o),
                "send",
                [Value::Int(status), Value::List(names), Value::List(values), Value::Bytes(body)],
            ) => {
                // A status that is not a `u16` cannot be valid, so it never reaches the host.
                let Ok(status) = u16::try_from(*status) else {
                    return Ok(result(Err(err_value("WebErr", "Invalid", vec![]))));
                };
                Ok(result(
                    o.send(status, &texts(names)?, &texts(values)?, body)
                        .map(|()| Value::Unit)
                        .map_err(web_err),
                ))
            }
            (Grant::Kv(k), "get", [Value::Str(key)]) => Ok(result(
                k.get(key)
                    .map(|v| option(v.map(|b| Value::Bytes(b.into()))))
                    .map_err(kv_err),
            )),
            (Grant::Kv(k), "set", [Value::Str(key), Value::Bytes(value)]) => {
                Ok(result(k.set(key, value).map(|()| Value::Unit).map_err(kv_err)))
            }
            (Grant::Kv(k), "delete", [Value::Str(key)]) => {
                Ok(result(k.delete(key).map(|()| Value::Unit).map_err(kv_err)))
            }
            (Grant::Kv(k), "keys", [Value::Str(prefix)]) => Ok(result(k.keys(prefix).map(strs).map_err(kv_err))),
            (Grant::Kv(k), "swap", [Value::Str(key), old, new]) => {
                let (old, new) = (bytes_option(old)?, bytes_option(new)?);
                Ok(result(
                    k.swap(key, old.as_deref(), new.as_deref())
                        .map(Value::Bool)
                        .map_err(kv_err),
                ))
            }
            (Grant::WsMessage(m), "kind", []) => Ok(Value::Str(m.kind().into())),
            // Ids count up from 1, so they fit an `Int`.
            (Grant::WsMessage(m), "conn", []) => Ok(Value::Int(m.conn() as i64)),
            (Grant::WsMessage(m), "data", []) => Ok(Value::Bytes(m.data().into())),
            (Grant::WsSend(w), "text", [Value::Str(s)]) => Ok(result(w.text(s).map(|()| Value::Unit).map_err(ws_err))),
            (Grant::WsSend(w), "binary", [Value::Bytes(b)]) => {
                Ok(result(w.binary(b).map(|()| Value::Unit).map_err(ws_err)))
            }
            (Grant::WsSend(w), "close", [Value::Int(code)]) => {
                // A code that is not a `u16` cannot be valid, so it never reaches the host.
                let Ok(code) = u16::try_from(*code) else {
                    return Ok(result(Err(err_value("WsErr", "Invalid", vec![]))));
                };
                Ok(result(w.close(code).map(|()| Value::Unit).map_err(ws_err)))
            }
            _ => Err(format!(
                "unsupported call `{cap}.{method}` with {} arguments",
                args.len()
            )),
        }
    }
}

fn strs(v: Vec<String>) -> Value {
    Value::List(v.into_iter().map(|s| Value::Str(s.into())).collect::<Vec<_>>().into())
}

fn texts(v: &[Value]) -> Result<Vec<String>, String> {
    let text = |v: &Value| match v {
        Value::Str(s) => Ok(s.to_string()),
        _ => Err("expected a list of Str".to_string()),
    };
    v.iter().map(text).collect()
}

/// `Option` is variant 0 for `None` and 1 for `Some`.
fn option(v: Option<Value>) -> Value {
    match v {
        None => Value::Enum(EnumRef::Option, 0, vec![]),
        Some(v) => Value::Enum(EnumRef::Option, 1, vec![v]),
    }
}

fn bytes_option(v: &Value) -> Result<Option<Vec<u8>>, String> {
    match v {
        Value::Enum(EnumRef::Option, 0, _) => Ok(None),
        Value::Enum(EnumRef::Option, 1, p) => match p.as_slice() {
            [Value::Bytes(b)] => Ok(Some(b.to_vec())),
            _ => Err("expected Option<Bytes>".into()),
        },
        _ => Err("expected Option<Bytes>".into()),
    }
}

/// `Result` is variant 0 for `Ok` and 1 for `Err`.
fn result(r: Result<Value, Value>) -> Value {
    match r {
        Ok(v) => Value::Enum(EnumRef::Result, 0, vec![v]),
        Err(e) => Value::Enum(EnumRef::Result, 1, vec![e]),
    }
}

/// A capability error enum value. Indexes come from the table generated from caps.wit, so
/// reordering a variant there cannot silently change which error kio code sees. The lowered
/// program numbers these enums first, in table order.
fn err_value(en: &str, case: &str, payload: Vec<Value>) -> Value {
    let table = CapTable::v0();
    let id = table.enums.iter().position(|e| e.name == en).expect("generated enum");
    let e = &table.enums[id];
    let i = e
        .variants
        .iter()
        .position(|v| v.name == case)
        .expect("generated variant");
    Value::Enum(EnumRef::User(EnumId(id as u32)), i, payload)
}

fn io_err(e: IoErr) -> Value {
    let case = match e {
        IoErr::NotFound => "NotFound",
        IoErr::Denied => "Denied",
        IoErr::TooLarge => "TooLarge",
        IoErr::InvalidPath => "InvalidPath",
        IoErr::Exists => "Exists",
    };
    err_value("IoErr", case, vec![])
}

fn http_err(e: HttpErr) -> Value {
    match e {
        HttpErr::Denied => err_value("HttpErr", "Denied", vec![]),
        HttpErr::Timeout => err_value("HttpErr", "Timeout", vec![]),
        HttpErr::TooLarge => err_value("HttpErr", "TooLarge", vec![]),
        HttpErr::Status(n) => err_value("HttpErr", "Status", vec![Value::Int(n.into())]),
    }
}

fn web_err(e: WebErr) -> Value {
    let case = match e {
        WebErr::AlreadySent => "AlreadySent",
        WebErr::Invalid => "Invalid",
    };
    err_value("WebErr", case, vec![])
}

fn kv_err(e: KvErr) -> Value {
    let case = match e {
        KvErr::TooLarge => "TooLarge",
        KvErr::Full => "Full",
    };
    err_value("KvErr", case, vec![])
}

fn ws_err(e: WsErr) -> Value {
    let case = match e {
        WsErr::Closed => "Closed",
        WsErr::TooLarge => "TooLarge",
        WsErr::Invalid => "Invalid",
    };
    err_value("WsErr", case, vec![])
}

/// The flag that grants the parameter a "no grant for" error names.
fn grant_hint(program: &lang::Program, msg: &str) -> Option<String> {
    let (name, ty) = program
        .main_params()
        .into_iter()
        .find(|(n, t)| msg == format!("no grant for `{n}: {t}`"))?;
    Some(match ty.as_str() {
        "Dir" => format!("grant it: `--dir {name}=path`"),
        "Http" => format!("grant it: `--http {name}=host`"),
        "Kv" => format!("grant it: `--kv {name}=path` or `--kv {name}=:memory:`"),
        "WsMessage" | "WsSend" => "`--listen` grants WsMessage and WsSend".into(),
        _ => "`--listen` grants Incoming and Outgoing".into(),
    })
}

fn grant_diag(message: String, hint: String) -> Diag {
    Diag {
        start: 0,
        end: 0,
        message,
        hint: Some(hint),
        file: 0,
    }
}

fn diag(d: lang::Diag) -> Diag {
    Diag {
        start: d.span.start as u32,
        end: d.span.end as u32,
        message: d.msg,
        hint: d.hint,
        file: d.span.file,
    }
}

fn same_files(a: &[File], b: &[File]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.name == y.name && x.source == y.source)
}

fn check_files(files: &[File]) -> Result<lang::Program, Vec<Diag>> {
    let pairs: Vec<(&str, &str)> = files.iter().map(|f| (f.name.as_str(), f.source.as_str())).collect();
    lang::check_files(&pairs, &CapTable::v0()).map_err(|ds| ds.into_iter().map(diag).collect())
}

export!(Component);
