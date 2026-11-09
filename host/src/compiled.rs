//! Compiled kio: the host checks, lowers and compiles the program, then runs it in the
//! `compiled` world (caps.wit, docs/codegen.md). The interpreter path (`Kio`) is unchanged.
//!
//! Both paths share the grant checks (grants.rs), the handle table, every host function with
//! its quotas, deadline and report check, and the audit log (lib.rs).

use crate::ws::{WsEvent, WsFrame};
use crate::{
    config, end_audit, explain, grants, main_calls, named, new_store, push_grants, set_stack, single, spawn_ticker,
    start_audit, web_grants, ws_grants, Bound, DirPaths, Handles, KvGrants, Limits, Origin, Request, Response, State,
};
use cap_std::fs::Dir as RealDir;
use codegen::Site;
use lang::Diag;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, PoisonError};
use wasmtime::component::{Component, HasSelf, Linker, Resource};
use wasmtime::{Engine, Result, Store};

mod bindings {
    wasmtime::component::bindgen!({
        world: "compiled",
        path: "../caps.wit",
        imports: { default: trappable },
        // The capability interfaces are the guest world's, so `State` implements them once.
        with: { "kio:caps/fs": crate::kio::caps::fs, "kio:caps/net": crate::kio::caps::net, "kio:caps/web": crate::kio::caps::web, "kio:caps/kv": crate::kio::caps::kv, "kio:caps/ws": crate::kio::caps::ws },
    });
}
use bindings::{Compiled as World, CompiledImports, Grant, Outcome};

/// The runtime's site for objects still alive when `run` ends, a codegen bug
/// (runtime/src/entry.rs `LEAKED`).
const LEAKED: u32 = u32::MAX - 1;

/// The runtime's site whose `trap` returns, passing the next error message's third value as
/// `a` (runtime/src/entry.rs `EXTRA`): `trap` has room for two.
const EXTRA: u32 = u32::MAX - 2;

/// A `trap` call: the run stops with the error at `site`.
#[derive(Debug)]
struct Trapped {
    site: u32,
    a: i64,
    b: i64,
    c: i64,
}

impl std::fmt::Display for Trapped {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "kio runtime error at site {}", self.site)
    }
}

impl std::error::Error for Trapped {}

impl CompiledImports for State {
    fn trap(&mut self, site: u32, a: i64, b: i64) -> Result<()> {
        if site == EXTRA {
            self.trap_c = a;
            return Ok(());
        }
        let c = self.trap_c;
        Err(wasmtime::Error::new(Trapped { site, a, b, c }))
    }
}

/// What a compiled run gives: main's value, or the checker's, the grants' or a runtime error.
pub type CompiledRun = std::result::Result<Outcome, Vec<Diag>>;

/// Runs compiled programs under `limits`, with grants and an audit log as `Kio` takes them.
/// Each run gets a fresh store, so its counters and deadline start over.
pub struct Compiled {
    limits: Limits,
    audit: Option<(Box<dyn Write + Send>, DirPaths)>,
    kvs: KvGrants,
    /// Capability imports the last run called.
    calls: u32,
    #[cfg(feature = "test-hooks")]
    hidden: Option<(String, String)>,
}

impl Compiled {
    pub fn new(limits: Limits) -> Self {
        Compiled {
            limits,
            audit: None,
            kvs: vec![],
            calls: 0,
            #[cfg(feature = "test-hooks")]
            hidden: None,
        }
    }

    /// Check `source`, compile it and run `main`, which takes no grants: its displayed value,
    /// the checker's diagnostics, or a runtime error with the interpreter's span and message. A
    /// run stopped by a limit is an `Err` whose message is `Kio::explain`'s. The runtime counts
    /// live objects and traps if any remain when the run ends: that is an `Err` too.
    pub fn run(source: &str, limits: Limits) -> Result<CompiledRun> {
        Compiled::new(limits).run_with(source, vec![], vec![])
    }

    /// Write the audit log of the next run to `out`, as `Kio::audit`.
    pub fn audit(&mut self, out: impl Write + Send + 'static, dir_paths: DirPaths) {
        self.audit = Some((Box::new(out), dir_paths));
    }

    /// Grant `kvs` by name to the next run.
    pub fn grant_kvs(&mut self, kvs: KvGrants) {
        self.kvs = kvs;
    }

    /// Test only: pretend the authority report lacks `cap.method`, as `Kio::hide_from_report`.
    #[cfg(feature = "test-hooks")]
    pub fn hide_from_report(&mut self, cap: &str, method: &str) {
        self.hidden = Some((cap.into(), method.into()));
    }

    /// Capability imports the last run called.
    pub fn import_calls(&self) -> u32 {
        self.calls
    }

    /// Like `run`, with `dirs` and `https` granted by name, and the `Kv` stores.
    pub fn run_with(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
    ) -> Result<CompiledRun> {
        self.run_inner(source, dirs, https, |_| Ok(Handles::default()), |_| {})
    }

    /// Run `main(req: Incoming, res: Outgoing, ..)` for one request, as `Kio::run_web`.
    pub fn run_web(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        request: Request,
    ) -> Result<(CompiledRun, Option<Response>)> {
        let mut sent = None;
        let r = self.run_inner(
            source,
            dirs,
            vec![],
            |store| {
                let (s, handles) = web_grants(store, request, ("req", "res"))?;
                sent = Some(s);
                Ok(handles)
            },
            |_| {},
        )?;
        let response = sent.and_then(|s| s.lock().unwrap_or_else(PoisonError::into_inner).take());
        Ok((r, response))
    }

    /// Run `main` for one WebSocket event, as `Kio::run_ws`.
    pub fn run_ws(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        event: WsEvent,
        closed: bool,
        names: &(String, String),
    ) -> Result<(CompiledRun, Vec<WsFrame>)> {
        let mut sends = None;
        let r = self.run_inner(
            source,
            dirs,
            vec![],
            |store| {
                let (s, handles) = ws_grants(store, event, closed, names)?;
                sends = Some(s);
                Ok(handles)
            },
            |_| {},
        )?;
        let frames = sends.map_or(vec![], |s| {
            std::mem::take(&mut s.lock().unwrap_or_else(PoisonError::into_inner).frames)
        });
        Ok((r, frames))
    }

    /// Test only: like `run_with`, with the lowered program changed by `edit` before it is
    /// compiled, to stand in for a codegen bug or a hostile module.
    #[cfg(feature = "test-hooks")]
    pub fn run_edited(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        edit: impl FnOnce(&mut lang::ir::Module),
    ) -> Result<CompiledRun> {
        self.run_inner(source, dirs, https, |_| Ok(Handles::default()), edit)
    }

    fn run_inner(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        handles: impl FnOnce(&mut Store<State>) -> Result<Handles>,
        edit: impl FnOnce(&mut lang::ir::Module),
    ) -> Result<CompiledRun> {
        let mut config = config::compile_config();
        set_stack(&mut config, self.limits.stack);
        let engine = Engine::new(&config)?;
        let caps = lang::CapTable::v0();
        // Checked and compiled before the store exists, so compiling is not charged to the run.
        let checked = match lang::check(source, &caps) {
            Ok(program) => {
                #[cfg_attr(not(feature = "test-hooks"), allow(unused_mut))]
                let mut allowed = main_calls(&lang::report(&program));
                #[cfg(feature = "test-hooks")]
                if let Some(h) = &self.hidden {
                    allowed.remove(h);
                }
                let mut module = lang::lower::lower(&program, &caps);
                edit(&mut module);
                let wasm = codegen::compile(&module).map_err(wasmtime::Error::from_anyhow)?;
                let component = Component::new(&engine, &wasm.component)?;
                Ok((allowed, grants::main_params(&module), wasm, component))
            }
            Err(diags) => Err(diags),
        };
        let _ticker = Ticker(spawn_ticker(&engine));
        let mut store = new_store(&engine, &self.limits)?;
        let handles = handles(&mut store)?;
        let bound = push_grants(&mut store, dirs, https, handles, std::mem::take(&mut self.kvs))?;
        let files = [single(source)];
        if let Some((out, paths)) = self.audit.take() {
            start_audit(store.data_mut(), out, &paths, &self.limits, &files, &bound)?;
        }
        let result = match checked {
            Err(diags) => Ok(Err(diags)),
            Ok((allowed, params, wasm, component)) => {
                store.data_mut().allowed = allowed;
                run_bound(
                    &mut store,
                    &engine,
                    &component,
                    &params,
                    &bound,
                    &wasm.sites,
                    &self.limits,
                )
            }
        };
        self.calls = store.data().calls;
        let outcome = match &result {
            Ok(Ok(o)) if !o.failed => "ok",
            Ok(Ok(_)) => "failed",
            _ => "error",
        };
        end_audit(&mut store, &self.limits, outcome)?;
        result
    }
}

/// Stops the epoch ticker when the run ends, however it ends.
struct Ticker(Arc<AtomicBool>);

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

/// Check the grants against `main`'s parameters, then run `main` with them in parameter order.
fn run_bound(
    store: &mut Store<State>,
    engine: &Engine,
    component: &Component,
    params: &grants::Params,
    bound: &[Bound],
    sites: &[Site],
    limits: &Limits,
) -> Result<CompiledRun> {
    let order = match grants::bind(params, &named(bound)) {
        Ok(order) => order,
        Err(diags) => return Ok(Err(diags)),
    };
    let grants: Vec<Grant> = order
        .into_iter()
        .map(|i| {
            let rep = bound[i].rep;
            match bound[i].kind {
                "Dir" => Grant::Dir(Resource::new_own(rep)),
                "Http" => Grant::Http(Resource::new_own(rep)),
                "Incoming" => Grant::Incoming(Resource::new_own(rep)),
                "Outgoing" => Grant::Outgoing(Resource::new_own(rep)),
                "Kv" => Grant::Kv(Resource::new_own(rep)),
                "WsMessage" => Grant::WsMessage(Resource::new_own(rep)),
                _ => Grant::WsSend(Resource::new_own(rep)),
            }
        })
        .collect();
    let mut linker = Linker::new(engine);
    World::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)?;
    let world = World::instantiate(&mut *store, component, &linker)?;
    match world.call_run(&mut *store, &grants) {
        Ok(Ok(outcome)) => Ok(Ok(outcome)),
        Ok(Err(())) => wasmtime::bail!("the compiled program refused its grants"),
        // The site comes from the guest, so it is checked like any guest value: a site
        // outside the table (the runtime's internal-error site, or a bug) is a host error,
        // never an index panic.
        Err(e) => match e.downcast_ref::<Trapped>() {
            Some(t) if t.site == LEAKED => {
                let msg = format!("compiled program leaked {} objects", t.a);
                Err(e.context(msg))
            }
            Some(t) => match sites.get(t.site as usize) {
                Some(site) => Ok(Err(vec![runtime_err(site, t.a, t.b, t.c)])),
                // The runtime's internal-error site: out of memory, when a grow was refused.
                None => match explain(store.data(), limits, &e) {
                    Some(msg) => Err(e.context(msg)),
                    None => Err(e.context("compiled program trapped at an unknown error site")),
                },
            },
            // A limit, such as deep recursion past `--stack`, or a host call refused by a quota
            // or the report check, reads as it does for `Kio`.
            None => match explain(store.data(), limits, &e) {
                Some(msg) => Err(e.context(msg)),
                None => Err(e),
            },
        },
    }
}

/// The interpreter's error for `trap(s, a, b)` (and `c` from `EXTRA`), as the diagnostic
/// `Kio::run` gives.
fn runtime_err(s: &Site, a: i64, b: i64, c: i64) -> Diag {
    Diag {
        span: s.span,
        msg: s.message(a, b, c),
        hint: None,
    }
}
