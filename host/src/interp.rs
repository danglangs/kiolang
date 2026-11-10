//! Test only: the interpreter, run natively as the reference compiled code is tested against
//! (docs/compiler-todo.md). `Interp` has `Kio`'s API. Its capability calls go through the same
//! host functions as compiled code (`State`'s caps.wit traits, called directly instead of
//! through Wasm), so grants, quotas, the report check and the audit log are the same; only the
//! fuel, memory and stack limits do not apply.

use crate::kio::caps::clock::HostClock;
use crate::kio::caps::fs::{HostDir, IoErr};
use crate::kio::caps::kv::{HostKv, KvEntry, KvErr};
use crate::kio::caps::log::{HostLog, LogErr};
use crate::kio::caps::net::{HostHttp, HttpErr, HttpRequest, HttpResponse};
use crate::kio::caps::rng::HostRng;
use crate::kio::caps::web::{HostIncoming, HostOutgoing, WebErr};
use crate::kio::caps::ws::{HostWsMessage, HostWsSend, WsErr};
use crate::ws::{WsEvent, WsFrame};
use crate::{
    end_audit, grants, main_calls, named, on_run_stack, push_grants, single, start_audit, state_limit, web_grants,
    ws_grants, Diag, DirPaths, Handles, KvGrants, Limits, Origin, Outcome, Report, Request, Response, RunResult,
    Sources, State, Stopped,
};
use cap_std::fs::Dir as RealDir;
use lang::ir::{EnumId, EnumRef, StructId};
use lang::{CapTable, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::PoisonError;
use wasmtime::component::Resource;
use wasmtime::Result;

/// Runs programs in the native interpreter, with grants and an audit log as `Kio` takes them.
pub struct Interp {
    limits: Limits,
    audit: Option<(Box<dyn Write + Send>, DirPaths)>,
    kvs: KvGrants,
    sources: Sources,
    hidden: Option<(String, String)>,
    calls: u32,
}

impl Interp {
    pub fn new(limits: Limits) -> Result<Self> {
        Ok(Interp {
            limits,
            audit: None,
            kvs: vec![],
            sources: Sources::default(),
            hidden: None,
            calls: 0,
        })
    }

    /// Grant `sources`' `Rng`s, `Clock`s and `Log`s by name to the next run, as `Kio::grant_sources`.
    pub fn grant_sources(&mut self, sources: Sources) {
        self.sources = sources;
    }

    /// Write the audit log of the next run to `out`, as `Kio::audit`.
    pub fn audit(&mut self, out: impl Write + Send + 'static, dir_paths: DirPaths) {
        self.audit = Some((Box::new(out), dir_paths));
    }

    /// Grant `kvs` by name to the next run.
    pub fn grant_kvs(&mut self, kvs: KvGrants) {
        self.kvs = kvs;
    }

    /// Pretend the authority report lacks `cap.method`, as `Kio::hide_from_report`.
    pub fn hide_from_report(&mut self, cap: &str, method: &str) {
        self.hidden = Some((cap.into(), method.into()));
    }

    pub fn explain(&self, e: &wasmtime::Error) -> Option<String> {
        crate::explain(e)
    }

    /// Capability calls the last run made.
    pub fn import_calls(&self) -> u32 {
        self.calls
    }

    pub fn run(&mut self, source: &str, dirs: Vec<(String, RealDir)>) -> Result<RunResult> {
        self.run_with(source, dirs, vec![])
    }

    pub fn run_with(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
    ) -> Result<RunResult> {
        self.run_files(&[single(source)], dirs, https)
    }

    pub fn run_files(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
    ) -> Result<RunResult> {
        self.run_inner(files, dirs, https, |_| Ok(Handles::default()))
    }

    pub fn run_web(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        request: Request,
    ) -> Result<(RunResult, Option<Response>)> {
        let (deliver, mut response) = tokio::sync::oneshot::channel();
        let r = self.run_inner(&[single(source)], dirs, vec![], |s| {
            Ok(web_grants(s, request, deliver, ("req", "res"))?.1)
        })?;
        Ok((r, response.try_recv().ok()))
    }

    pub fn run_ws(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        event: WsEvent,
        closed: bool,
        names: &(String, String),
    ) -> Result<(RunResult, Vec<WsFrame>)> {
        let mut sends = None;
        let r = self.run_inner(&[single(source)], dirs, vec![], |s| {
            let (out, handles) = ws_grants(s, event, closed, names)?;
            sends = Some(out);
            Ok(handles)
        })?;
        let frames = sends.map_or(vec![], |s| {
            std::mem::take(&mut s.lock().unwrap_or_else(PoisonError::into_inner).frames)
        });
        Ok((r, frames))
    }

    fn run_inner(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        handles: impl FnOnce(&mut State) -> Result<Handles> + Send,
    ) -> Result<RunResult> {
        // The interpreter recurses on the native stack: up to 9 MB at its call limit in debug.
        on_run_stack(|| self.run_here(files, dirs, https, handles))
    }

    fn run_here(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        handles: impl FnOnce(&mut State) -> Result<Handles>,
    ) -> Result<RunResult> {
        let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        let caps = CapTable::v0();
        let checked = lang::check_files(&pairs, &caps);
        let mut s = State::limited(&self.limits);
        let handles = handles(&mut s)?;
        let (kvs, sources) = (std::mem::take(&mut self.kvs), std::mem::take(&mut self.sources));
        let bound = push_grants(&mut s, dirs, https, handles, kvs, sources, None)?;
        if let Some((out, paths)) = self.audit.take() {
            start_audit(&mut s, out, &paths, &self.limits, files, &bound)?;
        }
        let result = match checked {
            Err(diags) => Ok(Err(diags.into_iter().map(Diag::from).collect())),
            Ok(program) => {
                let report = lang::report(&program);
                s.allowed = main_calls(&report);
                if let Some(h) = &self.hidden {
                    s.allowed.remove(h);
                }
                let params = grants::main_params(&lang::lower::lower(&program, &caps));
                match grants::bind(&params, &named(&bound)) {
                    Err(diags) => Ok(Err(diags.into_iter().map(Diag::from).collect())),
                    Ok(_) => {
                        let grants = bound.iter().map(|b| (b.name.clone(), (b.kind, b.rep))).collect();
                        let mut native = Native {
                            s: &mut s,
                            grants,
                            entry: struct_id("KvEntry"),
                            response: struct_id("HttpResponse"),
                        };
                        let r = lang::run(&program, &mut native);
                        match (r, state_limit(&s, &self.limits)) {
                            // A host call refused by a quota, the report check or the deadline
                            // stops the run, as a trap does in compiled code.
                            (Err(_), Some(msg)) => Err(wasmtime::Error::msg(Stopped(msg))),
                            (Err(e), None) => Ok(Err(vec![Diag::from(lang::Diag {
                                span: e.span,
                                msg: e.msg,
                                hint: None,
                            })])),
                            (Ok(v), _) => {
                                // A failed main shows its error alone, so `Err("bad")` reads `bad`.
                                let failed = matches!(&v, Value::Enum(EnumRef::Result, 1, _));
                                let value = match &v {
                                    Value::Enum(EnumRef::Result, 1, payload) => program.display(&payload[0]),
                                    _ => program.display(&v),
                                };
                                Ok(Ok(Outcome {
                                    report: Report {
                                        lines: report.lines().map(String::from).collect(),
                                    },
                                    value,
                                    failed,
                                }))
                            }
                        }
                    }
                }
            }
        };
        self.calls = s.calls;
        let outcome = match &result {
            Ok(Ok(o)) if !o.failed => "ok",
            Ok(Ok(_)) => "failed",
            _ => "error",
        };
        end_audit(&mut s, 0, outcome)?;
        result
    }
}

/// The interpreter's capabilities: each grant's kind and handle in `s`'s table, by name.
struct Native<'a> {
    s: &'a mut State,
    grants: BTreeMap<String, (&'static str, u32)>,
    /// The program's ids for the built-in `KvEntry` (`scan` returns it) and `HttpResponse`
    /// (`send` and `send_all` do).
    entry: StructId,
    response: StructId,
}

impl Native<'_> {
    fn response(&self, r: HttpResponse) -> Value {
        let fields = vec![Value::Int(r.status.into()), strs(r.headers), bytes(r.body)];
        Value::Struct(self.response, fields)
    }
}

/// An `HttpRequest` argument's fields: method, path, headers, body and `timeout_ms`, which is
/// passed as `u32::MAX` outside `u32` (the host caps it at the grant's timeout, as any value
/// past it).
fn request(v: &Value) -> Result<HttpRequest, String> {
    match v {
        Value::Struct(_, fields) => match fields.as_slice() {
            [Value::Str(method), Value::Str(path), Value::List(headers), Value::Bytes(body), Value::Int(ms)] => {
                Ok(HttpRequest {
                    method: method.to_string(),
                    path: path.to_string(),
                    headers: texts(headers)?,
                    body: body.to_vec(),
                    timeout_ms: u32::try_from(*ms).unwrap_or(u32::MAX),
                })
            }
            _ => Err("expected an HttpRequest".into()),
        },
        _ => Err("expected an HttpRequest".into()),
    }
}

impl lang::Caps for Native<'_> {
    fn has(&self, name: &str, ty: &str) -> bool {
        self.grants.get(name).is_some_and(|(kind, _)| *kind == ty)
    }

    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let &(kind, rep) = self.grants.get(cap).ok_or_else(|| format!("no grant `{cap}`"))?;
        let s = &mut *self.s;
        // A host function's `Err` stops the run.
        let stop = |e: wasmtime::Error| format!("{e:#}");
        Ok(match (kind, method, args.as_slice()) {
            ("Dir", "read", [Value::Str(p)]) => result(
                HostDir::read(s, own(rep), p.to_string())
                    .map_err(stop)?
                    .map(bytes)
                    .map_err(io_err),
            ),
            ("Dir", "create", [Value::Str(p), Value::Bytes(b)]) => {
                let r = HostDir::create(s, own(rep), p.to_string(), b.to_vec()).map_err(stop)?;
                result(r.map(unit).map_err(io_err))
            }
            ("Dir", "list", [Value::Str(p)]) => result(
                HostDir::list(s, own(rep), p.to_string())
                    .map_err(stop)?
                    .map(strs)
                    .map_err(io_err),
            ),
            ("Dir", "write", [Value::Str(p), Value::Bytes(b)]) => {
                let r = HostDir::write(s, own(rep), p.to_string(), b.to_vec()).map_err(stop)?;
                result(r.map(unit).map_err(io_err))
            }
            ("Dir", "remove", [Value::Str(p)]) => result(
                HostDir::remove(s, own(rep), p.to_string())
                    .map_err(stop)?
                    .map(unit)
                    .map_err(io_err),
            ),
            ("Dir", "exists", [Value::Str(p)]) => {
                let r = HostDir::exists(s, own(rep), p.to_string()).map_err(stop)?;
                result(r.map(Value::Bool).map_err(io_err))
            }
            ("Http", "get", [Value::Str(p)]) => result(
                HostHttp::get(s, own(rep), p.to_string())
                    .map_err(stop)?
                    .map(bytes)
                    .map_err(http_err),
            ),
            ("Http", "send", [Value::Str(m), Value::Str(p), Value::List(hs), Value::Bytes(b)]) => {
                let r = HostHttp::send(s, own(rep), m.to_string(), p.to_string(), texts(hs)?, b.to_vec());
                result(r.map_err(stop)?.map(|r| self.response(r)).map_err(http_err))
            }
            ("Http", "send_all", [Value::List(reqs)]) => {
                let reqs = reqs.iter().map(request).collect::<Result<Vec<_>, _>>()?;
                let rs = HostHttp::send_all(s, own(rep), reqs).map_err(stop)?;
                let each = |r: Result<HttpResponse, HttpErr>| result(r.map(|r| self.response(r)).map_err(http_err));
                Value::List(rs.into_iter().map(each).collect::<Vec<_>>().into())
            }
            ("Incoming", "method", []) => Value::Str(HostIncoming::method(s, own(rep)).map_err(stop)?.into()),
            ("Incoming", "target", []) => Value::Str(HostIncoming::target(s, own(rep)).map_err(stop)?.into()),
            ("Incoming", "header_names", []) => strs(HostIncoming::header_names(s, own(rep)).map_err(stop)?),
            ("Incoming", "header_values", []) => strs(HostIncoming::header_values(s, own(rep)).map_err(stop)?),
            ("Incoming", "body", []) => bytes(HostIncoming::body(s, own(rep)).map_err(stop)?),
            ("Outgoing", "send", [Value::Int(status), Value::List(names), Value::List(values), Value::Bytes(body)]) => {
                // A status that is not a `u16` cannot be valid, so it never reaches the host.
                let Ok(status) = u16::try_from(*status) else {
                    return Ok(result(Err(err_value("WebErr", "Invalid", vec![]))));
                };
                let r = HostOutgoing::send(s, own(rep), status, texts(names)?, texts(values)?, body.to_vec());
                result(r.map_err(stop)?.map(unit).map_err(web_err))
            }
            ("Kv", "get", [Value::Str(k)]) => {
                let r = HostKv::get(s, own(rep), k.to_string()).map_err(stop)?;
                result(r.map(|v| option(v.map(bytes))).map_err(kv_err))
            }
            ("Kv", "set", [Value::Str(k), Value::Bytes(v)]) => {
                let r = HostKv::set(s, own(rep), k.to_string(), v.to_vec()).map_err(stop)?;
                result(r.map(unit).map_err(kv_err))
            }
            ("Kv", "delete", [Value::Str(k)]) => result(
                HostKv::delete(s, own(rep), k.to_string())
                    .map_err(stop)?
                    .map(unit)
                    .map_err(kv_err),
            ),
            ("Kv", "keys", [Value::Str(p)]) => result(
                HostKv::keys(s, own(rep), p.to_string())
                    .map_err(stop)?
                    .map(strs)
                    .map_err(kv_err),
            ),
            ("Kv", "swap", [Value::Str(k), old, new]) => {
                let (old, new) = (bytes_option(old)?, bytes_option(new)?);
                let r = HostKv::swap(s, own(rep), k.to_string(), old, new).map_err(stop)?;
                result(r.map(Value::Bool).map_err(kv_err))
            }
            ("Kv", "scan", [Value::Str(p), after, Value::Int(limit)]) => {
                // A limit that is not a `u32` is outside 1 to 1,000, so it never reaches the host.
                let Ok(limit) = u32::try_from(*limit) else {
                    return Ok(result(Err(err_value("KvErr", "TooLarge", vec![]))));
                };
                let r = HostKv::scan(s, own(rep), p.to_string(), str_option(after)?, limit).map_err(stop)?;
                let entry = |e: KvEntry| Value::Struct(self.entry, vec![Value::Str(e.key.into()), bytes(e.value)]);
                result(
                    r.map(|es| Value::List(es.into_iter().map(entry).collect::<Vec<_>>().into()))
                        .map_err(kv_err),
                )
            }
            ("Kv", "add", [Value::Str(k), Value::Int(n)]) => {
                let r = HostKv::add(s, own(rep), k.to_string(), *n).map_err(stop)?;
                result(r.map(Value::Int).map_err(kv_err))
            }
            ("WsMessage", "kind", []) => Value::Str(HostWsMessage::kind(s, own(rep)).map_err(stop)?.into()),
            // Ids count up from 1, so they fit an `Int`.
            ("WsMessage", "conn", []) => Value::Int(HostWsMessage::conn(s, own(rep)).map_err(stop)? as i64),
            ("WsMessage", "data", []) => bytes(HostWsMessage::data(s, own(rep)).map_err(stop)?),
            ("WsMessage", "target", []) => Value::Str(HostWsMessage::target(s, own(rep)).map_err(stop)?.into()),
            ("WsMessage", "header", [Value::Str(n)]) => {
                let v = HostWsMessage::header(s, own(rep), n.to_string()).map_err(stop)?;
                option(v.map(|v| Value::Str(v.into())))
            }
            ("WsSend", "text", [Value::Str(t)]) => result(
                HostWsSend::text(s, own(rep), t.to_string())
                    .map_err(stop)?
                    .map(unit)
                    .map_err(ws_err),
            ),
            ("WsSend", "binary", [Value::Bytes(b)]) => result(
                HostWsSend::binary(s, own(rep), b.to_vec())
                    .map_err(stop)?
                    .map(unit)
                    .map_err(ws_err),
            ),
            ("WsSend", "close", [Value::Int(code)]) => {
                // A code that is not a `u16` cannot be valid, so it never reaches the host.
                let Ok(code) = u16::try_from(*code) else {
                    return Ok(result(Err(err_value("WsErr", "Invalid", vec![]))));
                };
                result(
                    HostWsSend::close(s, own(rep), code)
                        .map_err(stop)?
                        .map(unit)
                        .map_err(ws_err),
                )
            }
            // `eval::cap_arg_error` has refused a count outside `u32`, so this never truncates.
            ("Rng", "bytes", [Value::Int(n)]) => {
                let n = u32::try_from(*n).unwrap_or(u32::MAX);
                bytes(HostRng::bytes(s, own(rep), n).map_err(stop)?)
            }
            ("Rng", "int", [Value::Int(lo), Value::Int(hi)]) => {
                Value::Int(HostRng::int(s, own(rep), *lo, *hi).map_err(stop)?)
            }
            ("Clock", "now_ms", []) => Value::Int(HostClock::now_ms(s, own(rep)).map_err(stop)?),
            ("Log", "write", [Value::Str(line)]) => result(
                HostLog::write(s, own(rep), line.to_string())
                    .map_err(stop)?
                    .map(unit)
                    .map_err(|LogErr::TooLarge| err_value("LogErr", "TooLarge", vec![])),
            ),
            _ => {
                return Err(format!(
                    "unsupported call `{cap}.{method}` with {} arguments",
                    args.len()
                ))
            }
        })
    }
}

/// The handle `rep` as the resource type a host function takes.
fn own<T: 'static>(rep: u32) -> Resource<T> {
    Resource::new_own(rep)
}

fn unit(_: ()) -> Value {
    Value::Unit
}

fn bytes(b: Vec<u8>) -> Value {
    Value::Bytes(b.into())
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

fn str_option(v: &Value) -> Result<Option<String>, String> {
    match v {
        Value::Enum(EnumRef::Option, 0, _) => Ok(None),
        Value::Enum(EnumRef::Option, 1, p) => match p.as_slice() {
            [Value::Str(s)] => Ok(Some(s.to_string())),
            _ => Err("expected Option<Str>".into()),
        },
        _ => Err("expected Option<Str>".into()),
    }
}

/// `Result` is variant 0 for `Ok` and 1 for `Err`.
fn result(r: Result<Value, Value>) -> Value {
    match r {
        Ok(v) => Value::Enum(EnumRef::Result, 0, vec![v]),
        Err(e) => Value::Enum(EnumRef::Result, 1, vec![e]),
    }
}

/// A capability error enum value. Indexes come from the table generated from caps.wit; the
/// lowered program numbers these enums first, in table order.
fn err_value(en: &str, case: &str, payload: Vec<Value>) -> Value {
    let table = CapTable::v0();
    let id = table.enums.iter().position(|e| e.name == en).expect("generated enum");
    let i = (table.enums[id].variants.iter())
        .position(|v| v.name == case)
        .expect("generated variant");
    Value::Enum(EnumRef::User(EnumId(id as u32)), i, payload)
}

/// A capability struct's id: the lowered program numbers these structs first, in table order.
fn struct_id(name: &str) -> StructId {
    let table = CapTable::v0();
    let id = table
        .structs
        .iter()
        .position(|s| s.name == name)
        .expect("generated struct");
    StructId(id as u32)
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
        HttpErr::Invalid => err_value("HttpErr", "Invalid", vec![]),
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
        KvErr::NotInt => "NotInt",
        KvErr::Overflow => "Overflow",
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
