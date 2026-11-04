wit_bindgen::generate!({ world: "guest", path: "../caps.wit" });

use kio::caps::fs::IoErr;
use kio::caps::net::HttpErr;
use lang::{CapTable, Value};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

struct Component;

impl Guest for Component {
    fn check(source: String) -> Result<Report, Vec<Diag>> {
        match lang::check(&source, &CapTable::v0()) {
            Ok(program) => Ok(Report {
                lines: lang::report(&program).lines().map(String::from).collect(),
            }),
            Err(diags) => Err(diags.into_iter().map(diag).collect()),
        }
    }

    fn run(source: String, dirs: Vec<DirGrant>, https: Vec<HttpGrant>) -> Result<(Report, String), Vec<Diag>> {
        let program =
            lang::check(&source, &CapTable::v0()).map_err(|ds| ds.into_iter().map(diag).collect::<Vec<_>>())?;
        let mut grants = Grants(BTreeMap::new());
        let mut errs = Vec::new();
        let names: Vec<String> = program.main_params().into_iter().map(|(n, _)| n).collect();
        let named = dirs
            .into_iter()
            .map(|g| (g.name, Grant::Dir(g.dir)))
            .chain(https.into_iter().map(|g| (g.name, Grant::Http(g.http))));
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
            Ok(v) => Ok((
                Report {
                    lines: lang::report(&program).lines().map(String::from).collect(),
                },
                format!("{v:?}"),
            )),
            Err(e) => Err(vec![Diag {
                start: e.span.start as u32,
                end: e.span.end as u32,
                message: e.msg,
                hint: None,
            }]),
        }
    }
}

enum Grant {
    Dir(Dir),
    Http(Http),
}

/// The handles granted for one run, by `main` parameter name.
struct Grants(BTreeMap<String, Grant>);

impl lang::Caps for Grants {
    fn has(&self, name: &str, ty: &str) -> bool {
        matches!(
            (self.0.get(name), ty),
            (Some(Grant::Dir(_)), "Dir") | (Some(Grant::Http(_)), "Http")
        )
    }

    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let grant = self.0.get(cap).ok_or_else(|| format!("no grant `{cap}`"))?;
        match (grant, method, args.as_slice()) {
            (Grant::Dir(d), "read", [Value::Str(path)]) => Ok(result(d.read(path).map(Value::Bytes).map_err(io_err))),
            (Grant::Dir(d), "create", [Value::Str(path), Value::Bytes(data)]) => {
                Ok(result(d.create(path, data).map(|()| Value::Unit).map_err(io_err)))
            }
            (Grant::Http(h), "get", [Value::Str(path)]) => Ok(result(h.get(path).map(Value::Bytes).map_err(http_err))),
            _ => Err(format!(
                "unsupported call `{cap}.{method}` with {} arguments",
                args.len()
            )),
        }
    }
}

/// `Result` is variant 0 for `Ok` and 1 for `Err`.
fn result(r: Result<Value, Value>) -> Value {
    match r {
        Ok(v) => Value::Enum("Result".into(), 0, vec![v]),
        Err(e) => Value::Enum("Result".into(), 1, vec![e]),
    }
}

/// A capability error enum value. Indexes come from the table generated from caps.wit, so
/// reordering a variant there cannot silently change which error kio code sees.
fn err_value(en: &str, case: &str, payload: Vec<Value>) -> Value {
    let table = CapTable::v0();
    let e = table.enums.iter().find(|e| e.name == en).expect("generated enum");
    let i = e
        .variants
        .iter()
        .position(|v| v.name == case)
        .expect("generated variant");
    Value::Enum(en.into(), i, payload)
}

fn io_err(e: IoErr) -> Value {
    let case = match e {
        IoErr::NotFound => "NotFound",
        IoErr::Denied => "Denied",
        IoErr::TooLarge => "TooLarge",
        IoErr::InvalidPath => "InvalidPath",
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

fn grant_diag(message: String, hint: String) -> Diag {
    Diag {
        start: 0,
        end: 0,
        message,
        hint: Some(hint),
    }
}

fn diag(d: lang::Diag) -> Diag {
    Diag {
        start: d.span.start as u32,
        end: d.span.end as u32,
        message: d.msg,
        hint: d.hint,
    }
}

export!(Component);
