//! Binding grants to `main`'s parameters, for compiled runs (`Kio`, `Server`) and the test
//! oracle (`Interp`). The host checks every grant here before a run starts; `lang::run`'s own
//! checks give the same diagnostics and are never reached.
use lang::ir::{Module, Ty};
use lang::{Diag, Span};
use std::collections::HashSet;

/// `main`'s parameters: name, capability type (`Dir`) and span, in order.
pub(crate) type Params = Vec<(String, String, Span)>;

/// The parameters of `m`'s `main`.
pub(crate) fn main_params(m: &Module) -> Params {
    let main = &m.funcs[m.main.0 as usize];
    main.locals[..main.params as usize]
        .iter()
        .map(|l| {
            let Ty::Cap(c) = l.ty else {
                unreachable!("main takes capabilities only")
            };
            (l.name.clone(), m.caps[c.0 as usize].name.clone(), l.span)
        })
        .collect()
}

/// For each parameter, the index in `grants` (`(name, kind)` each) of the grant bound to it.
/// Errors: every grant that is not a parameter or is given twice, or
/// else the first parameter without a grant of its type.
pub(crate) fn bind(params: &Params, grants: &[(String, &str)]) -> Result<Vec<usize>, Vec<Diag>> {
    let names: Vec<&str> = params.iter().map(|(n, _, _)| n.as_str()).collect();
    let mut seen = HashSet::new();
    let mut errs = Vec::new();
    for (name, _) in grants {
        if !names.contains(&name.as_str()) {
            let hint = format!("grant only `main`'s parameters: {}", names.join(", "));
            errs.push(diag(format!("grant `{name}` is not a parameter of `main`"), hint));
        } else if !seen.insert(name) {
            let hint = "give each grant one name".to_string();
            errs.push(diag(format!("grant `{name}` is given more than once"), hint));
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    params
        .iter()
        .map(|(name, ty, span)| {
            grants.iter().position(|(n, k)| n == name && k == ty).ok_or_else(|| {
                vec![Diag {
                    span: *span,
                    msg: format!("no grant for `{name}: {ty}`"),
                    hint: Some(hint(name, ty)),
                }]
            })
        })
        .collect()
}

/// The flag that grants parameter `name: ty`.
fn hint(name: &str, ty: &str) -> String {
    match ty {
        "Dir" => format!("grant it: `--dir {name}=path`"),
        "Http" => format!("grant it: `--http {name}=host`"),
        "Kv" => format!("grant it: `--kv {name}=path` or `--kv {name}=:memory:`"),
        "Rng" => format!("grant it: `--rng {name}` (the OS's randomness) or `--rng {name}=seed:N`"),
        "Clock" => {
            format!("grant it: `--clock {name}` (the system clock) or `--clock {name}=fixed:<ms or RFC 3339 time>`")
        }
        "Log" => format!("grant it: `--log {name}=path` (appended) or `--log {name}=-` (stderr)"),
        "Db" => {
            format!("grant it: `--db {name}=path` (a SQLite file), `--db {name}=path:ro` or `--db {name}=:memory:`")
        }
        "WsMessage" | "WsSend" => "`--listen` grants WsMessage and WsSend".into(),
        _ => "`--listen` grants Incoming and Outgoing".into(),
    }
}

/// A grant error, which has no place in the source.
fn diag(msg: String, hint: String) -> Diag {
    Diag {
        span: Span {
            start: 0,
            end: 0,
            file: 0,
        },
        msg,
        hint: Some(hint),
    }
}
