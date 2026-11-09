//! Differential test: every program in the lang tests and examples that codegen supports gives
//! the same result compiled as interpreted (docs/codegen.md "Testing").

use kio_host::{Compiled, Kio, Limits, MAX_STACK};
use lang::CapTable;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

#[path = "../../lang/tests/common/corpus.rs"]
mod corpus;

/// What a run gave: the displayed value and failed flag, or the first error as (message, span).
type Run = Result<(String, bool), (String, usize, usize)>;

/// A finished run, or the diagnostic it gave, or a host error's text.
type Res<T> = Result<Result<T, (String, usize, usize)>, String>;

/// A run ended early: by a limit (decision 6: the two paths limit differently), or for any
/// other reason, which is a mismatch.
enum End {
    Done(Run),
    Limit,
}

fn is_limit(msg: &str) -> bool {
    ["out of fuel", "stack overflow", "evaluation is too deep"]
        .iter()
        .any(|l| msg.contains(l))
}

fn limits() -> Limits {
    Limits {
        fuel: 2_000_000_000,
        ..Limits::default()
    }
}

fn finish<T>(r: Res<T>, f: impl Fn(T) -> Run) -> End {
    match r {
        Ok(Ok(v)) => End::Done(f(v)),
        Ok(Err(d)) => match d {
            (m, _, _) if is_limit(&m) => End::Limit,
            (m, s, e) => End::Done(Err((m, s, e))),
        },
        Err(m) if is_limit(&m) => End::Limit,
        Err(m) => End::Done(Err((format!("host error: {m}"), 0, 0))),
    }
}

fn compiled(src: &str) -> End {
    let r = match Compiled::run(src, limits()) {
        Ok(Ok(o)) => Ok(Ok(o)),
        Ok(Err(d)) => Ok(Err((d[0].msg.clone(), d[0].span.start, d[0].span.end))),
        Err(e) => Err(format!("{e:#}")),
    };
    finish(r, |o| Ok((o.value, o.failed)))
}

fn interpreted(src: &str) -> End {
    let r = match Kio::new(limits()).unwrap().run(src, vec![]) {
        Ok(Ok(o)) => Ok(Ok(o)),
        Ok(Err(d)) => Ok(Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize))),
        Err(e) => Err(format!("{e:#}")),
    };
    finish(r, |o| Ok((o.value, o.failed)))
}

fn differential() {
    let caps = CapTable::v0();
    let mut ran = 0;
    let mut agreed = 0;
    let mut limited = 0;
    let mut capabilities = 0;
    let mut unsupported: BTreeMap<String, usize> = BTreeMap::new();
    let mut mismatches = Vec::new();
    let mut leaks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for files in corpus::sources(Path::new("../")) {
        let [(_, src)] = files.as_slice() else { continue };
        if !seen.insert(src.clone()) {
            continue;
        }
        let Ok(program) = lang::check(src, &caps) else { continue };
        let m = lang::lower::lower(&program, &caps);
        if m.funcs[m.main.0 as usize].params > 0 {
            capabilities += 1;
            continue;
        }
        let c = match catch_unwind(AssertUnwindSafe(|| compiled(src))) {
            Ok(c) => c,
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().unwrap_or_default();
                let node = msg.strip_prefix("not implemented: codegen: ").unwrap_or(&msg);
                *unsupported.entry(node.to_string()).or_default() += 1;
                continue;
            }
        };
        // The runtime traps when objects are left alive at the end of a run that did not trap.
        if let End::Done(Err((m, _, _))) = &c {
            if m.contains("compiled program leaked") {
                ran += 1;
                leaks.push(format!("{m}\n{src}"));
                continue;
            }
        }
        let i = interpreted(src);
        match (c, i) {
            (End::Done(c), End::Done(i)) => {
                ran += 1;
                if c == i {
                    agreed += 1;
                } else {
                    mismatches.push(format!("compiled: {c:?}\ninterpreted: {i:?}\n{src}"));
                }
            }
            _ => {
                ran += 1;
                limited += 1;
            }
        }
    }
    println!(
        "ran both ways {ran}, agreed {agreed}, excluded by limits {limited}, leaked {}, skipped: capabilities {capabilities}, unsupported by node kind {unsupported:?}",
        leaks.len()
    );
    assert!(leaks.is_empty(), "{} leaked:\n{}", leaks.len(), leaks.join("\n----\n"));
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n----\n")
    );
    assert!(agreed > 20, "only {agreed} programs agreed");
}

#[test]
fn compiled_agrees_with_the_interpreter() {
    // Wasm runs on this thread's stack, so it needs room above `--stack`, as `kio` gives it.
    let t = std::thread::Builder::new().stack_size(4 * MAX_STACK);
    // Codegen's `unimplemented!` panics are expected and counted, so keep them quiet.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = t.spawn(differential).unwrap().join();
    std::panic::set_hook(hook);
    if let Err(e) = r {
        std::panic::resume_unwind(e);
    }
}
