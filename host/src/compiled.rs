//! Running kio: the host checks, lowers and compiles a program, then runs it in the `compiled`
//! world (caps.wit, docs/codegen.md). `Kio` compiles and runs one program per call (`kio run`);
//! `Server` compiles once and runs the program per request or WebSocket event (`--listen`).
//!
//! Every run gets a fresh store and instance, with the grant checks (grants.rs), the handle
//! table, every host function with its quotas, deadline and report check, and the audit log
//! (lib.rs).

use crate::bindings::{Compiled as World, CompiledImports, Grant};
use crate::ws::{WsEvent, WsFrame};
use crate::{
    audit_header, config, end_audit, grants, limit_message, main_calls, named, new_store, on_run_stack, push_grants,
    set_stack, single, spawn_ticker, start_audit, web_grants, write_line, ws_grants, Audit, Bound, Diag, DirPaths,
    Handles, KvGrants, Limits, Origin, Outcome, Report, Request, Response, RunResult, SharedLog, Sources, State,
    Stopped,
};
use cap_std::fs::Dir as RealDir;
use codegen::Site;
use std::collections::HashSet;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};
use wasmtime::component::{Component, HasSelf, InstancePre, Linker, Resource};
use wasmtime::{Engine, InstanceAllocationStrategy, PoolingAllocationConfig, Result, Store};

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

/// A program the host checked and compiled, ready to instantiate.
struct Program {
    files: Vec<(String, String)>,
    report: String,
    /// `(parameter, method)` pairs on `main`'s report line: every call the program may make.
    allowed: HashSet<(String, String)>,
    params: grants::Params,
    pre: InstancePre<State>,
    sites: Vec<Site>,
}

/// A program checked and lowered to a component, not yet compiled for an engine.
struct Lowered {
    report: String,
    params: grants::Params,
    wasm: codegen::Wasm,
}

/// Check, lower and compile `files` (root first) for `engine`: the program, or the checker's
/// diagnostics. Native code comes from `cache` when it is given. `edit` changes the lowered
/// program before codegen (tests only).
fn compile(
    engine: &Engine,
    files: &[(String, String)],
    limits: &Limits,
    cache: Option<&PathBuf>,
    edit: impl FnOnce(&mut lang::ir::Module),
) -> Result<std::result::Result<Program, Vec<Diag>>> {
    match lower(files, limits, edit)? {
        Ok(lowered) => Ok(Ok(link(engine, files, lowered, cache)?)),
        Err(diags) => Ok(Err(diags)),
    }
}

/// Check and lower `files` to a component, or the checker's diagnostics.
fn lower(
    files: &[(String, String)],
    limits: &Limits,
    edit: impl FnOnce(&mut lang::ir::Module),
) -> Result<std::result::Result<Lowered, Vec<Diag>>> {
    let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
    let caps = lang::CapTable::v0();
    let program = match lang::check_files(&pairs, &caps) {
        Ok(p) => p,
        Err(diags) => return Ok(Err(diags.into_iter().map(Diag::from).collect())),
    };
    let report = lang::report(&program);
    let mut module = lang::lower::lower(&program, &caps);
    edit(&mut module);
    let wasm = generate(&module)?;
    // Wasmtime's own refusal names pages ("memory minimum size of 18 pages exceeds memory
    // limits"), or the pool's requirements for a server.
    if wasm.memory > limits.memory {
        let msg = format!(
            "out of memory: the program starts with {} MB (limit {} MB; raise it with --memory)",
            wasm.memory.div_ceil(1 << 20),
            limits.memory >> 20
        );
        return Err(wasmtime::Error::msg(Stopped(msg)));
    }
    let params = grants::main_params(&module);
    Ok(Ok(Lowered { report, params, wasm }))
}

/// Compile `lowered` for `engine`, from `cache` when it is given.
fn link(engine: &Engine, files: &[(String, String)], lowered: Lowered, cache: Option<&PathBuf>) -> Result<Program> {
    let Lowered { report, params, wasm } = lowered;
    let component = match cache {
        Some(dir) => crate::cache::component(engine, &wasm.component, dir)?,
        None => Component::new(engine, &wasm.component)?,
    };
    let mut linker = Linker::new(engine);
    World::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)?;
    Ok(Program {
        files: files.to_vec(),
        allowed: main_calls(&report),
        report,
        params,
        pre: linker.instantiate_pre(&component)?,
        sites: wasm.sites,
    })
}

/// What one instance of `component` takes from the pooling allocator: `(core instances, memories,
/// tables)`, counted as Wasmtime counts them (every core module instantiated, and the memories
/// and tables those modules define). Wasmtime refuses the component at compile time if the
/// counts are short, so a wrong count fails startup, never a request.
fn pool_needs(component: &[u8]) -> Result<(u32, u32, u32)> {
    use wasmparser::{Instance, Parser, Payload};
    let mut modules: Vec<(u32, u32)> = Vec::new();
    let mut instances = Vec::new();
    // Depth 0 is the component; 1 is one of its core modules.
    let mut depth = 0;
    for payload in Parser::new(0).parse_all(component) {
        match payload? {
            Payload::ModuleSection { .. } => {
                modules.push((0, 0));
                depth += 1;
            }
            Payload::ComponentSection { .. } => wasmtime::bail!("pool sizing: nested components are not counted"),
            Payload::MemorySection(r) if depth == 1 => modules.last_mut().unwrap().0 += r.count(),
            Payload::TableSection(r) if depth == 1 => modules.last_mut().unwrap().1 += r.count(),
            Payload::InstanceSection(r) => {
                for instance in r {
                    if let Instance::Instantiate { module_index, .. } = instance? {
                        instances.push(module_index);
                    }
                }
            }
            Payload::End(_) if depth > 0 => depth -= 1,
            _ => {}
        }
    }
    let (mut memories, mut tables) = (0, 0);
    for i in &instances {
        let Some((m, t)) = modules.get(*i as usize) else {
            wasmtime::bail!("pool sizing: core module {i} is not defined in the component");
        };
        memories += m;
        tables += t;
    }
    Ok((instances.len() as u32, memories, tables))
}

/// Codegen for `module`. IR that codegen does not handle yet panics there (`unimplemented!`);
/// that is an error naming what is missing, never a host crash.
fn generate(module: &lang::ir::Module) -> Result<codegen::Wasm> {
    match catch_unwind(AssertUnwindSafe(|| codegen::compile(module))) {
        // `{e:#}` keeps the cause (a Wasm limit, say), which the top context alone hides.
        Ok(wasm) => wasm.map_err(|e| wasmtime::Error::msg(format!("{e:#}"))),
        Err(panic) => {
            let msg = (panic.downcast_ref::<String>().cloned())
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            match msg.strip_prefix("not implemented: codegen: ") {
                Some(what) => wasmtime::bail!("not compiled yet: {what}"),
                None => wasmtime::bail!("internal compiler error: {msg}"),
            }
        }
    }
}

/// An instance and the grants for its `run`, in parameter order.
type Started = (World, Vec<Grant>);

/// Bind `bound` to `main`'s parameters, then instantiate `program` in `store`: the instance and
/// the grants in parameter order, or the grant errors. `Err` means no instance could be made.
fn instantiate(
    store: &mut Store<State>,
    program: &Program,
    bound: &[Bound],
) -> Result<std::result::Result<Started, Vec<Diag>>> {
    let order = match grants::bind(&program.params, &named(bound)) {
        Ok(order) => order,
        Err(diags) => return Ok(Err(diags.into_iter().map(Diag::from).collect())),
    };
    let grants = order
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
                "Rng" => Grant::Rng(Resource::new_own(rep)),
                "Clock" => Grant::Clock(Resource::new_own(rep)),
                "Log" => Grant::Log(Resource::new_own(rep)),
                _ => Grant::WsSend(Resource::new_own(rep)),
            }
        })
        .collect();
    let instance = program.pre.instantiate(&mut *store)?;
    Ok(Ok((World::new(&mut *store, &instance)?, grants)))
}

/// Run `main` with `grants`: its outcome, or a runtime error with the interpreter's span and
/// message. A run stopped by a limit is an `Err` carrying `Stopped`. The runtime counts live
/// objects and traps if any remain when the run ends: that is an `Err` too.
fn call(
    store: &mut Store<State>,
    world: &World,
    grants: &[Grant],
    program: &Program,
    limits: &Limits,
) -> Result<RunResult> {
    let stopped = |e: wasmtime::Error, s: &State| match limit_message(s, limits, &e) {
        Some(msg) => Err(e.context(Stopped(msg))),
        None => Err(e),
    };
    match world.call_run(&mut *store, grants) {
        Ok(Ok(o)) => Ok(Ok(Outcome {
            report: Report {
                lines: program.report.lines().map(String::from).collect(),
            },
            value: o.value,
            failed: o.failed,
        })),
        Ok(Err(())) => wasmtime::bail!("the compiled program refused its grants"),
        // The site comes from the guest, so it is checked like any guest value: a site
        // outside the table (the runtime's internal-error site, or a bug) is a host error,
        // never an index panic.
        Err(e) => match e.downcast_ref::<Trapped>() {
            Some(t) if t.site == LEAKED => {
                let msg = format!("compiled program leaked {} objects", t.a);
                Err(e.context(msg))
            }
            Some(t) => match program.sites.get(t.site as usize) {
                Some(site) => Ok(Err(vec![Diag {
                    start: site.span.start as u32,
                    end: site.span.end as u32,
                    message: site.message(t.a, t.b, t.c),
                    hint: None,
                    file: site.span.file,
                }])),
                // The runtime's internal-error site: out of memory, when a grow was refused.
                None => match limit_message(store.data(), limits, &e) {
                    Some(msg) => Err(e.context(Stopped(msg))),
                    None => Err(e.context("compiled program trapped at an unknown error site")),
                },
            },
            // A limit, such as deep recursion past `--stack`, or a host call refused by a quota
            // or the report check.
            None => stopped(e, store.data()),
        },
    }
}

/// Stops an epoch ticker when its owner drops.
struct Ticker(Arc<AtomicBool>);

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

/// Compiles and runs programs under `limits`, with grants and an audit log. Each run gets a
/// fresh store, so its counters and deadline start over; compiling is not charged to it.
pub struct Kio {
    engine: Engine,
    limits: Limits,
    _ticker: Ticker,
    /// The audit log and the `(name, path)` of each `Dir` grant, for the next run.
    audit: Option<(Box<dyn Write + Send>, DirPaths)>,
    /// `Kv` grants for the next run.
    kvs: KvGrants,
    /// `Rng`, `Clock` and `Log` grants for the next run.
    sources: Sources,
    /// Where compiled programs are kept between runs (cache.rs), if anywhere.
    cache: Option<PathBuf>,
    /// Capability imports and peak memory of the last run.
    calls: u32,
    peak_memory: usize,
    /// Test only: a `(parameter, method)` pair to leave out of the authority report.
    #[cfg(feature = "test-hooks")]
    hidden: Option<(String, String)>,
    #[cfg(feature = "test-hooks")]
    fail_writes: bool,
}

impl Kio {
    /// A runner under `limits`. Running out of stack, fuel or time stops a run, with an error
    /// `explain` names.
    pub fn new(limits: Limits) -> Result<Self> {
        let mut config = config::compile_config();
        set_stack(&mut config, limits.stack);
        let engine = Engine::new(&config)?;
        let ticker = Ticker(spawn_ticker(&engine));
        Ok(Kio {
            engine,
            limits,
            _ticker: ticker,
            audit: None,
            kvs: vec![],
            sources: Sources::default(),
            cache: None,
            calls: 0,
            peak_memory: 0,
            #[cfg(feature = "test-hooks")]
            hidden: None,
            #[cfg(feature = "test-hooks")]
            fail_writes: false,
        })
    }

    /// Write the audit log (H8) of the next run to `out`: a header, one line per host call, a
    /// trailer. `dir_paths` gives the `(name, path)` of each `Dir` grant for the header, since a
    /// handle does not know its path. Sizes only, never contents.
    pub fn audit(&mut self, out: impl Write + Send + 'static, dir_paths: DirPaths) {
        self.audit = Some((Box::new(out), dir_paths));
    }

    /// Grant `kvs` by name to the next run, with the other grants `run_*` is given.
    pub fn grant_kvs(&mut self, kvs: KvGrants) {
        self.kvs = kvs;
    }

    /// Grant `sources`' `Rng`s, `Clock`s and `Log`s by name to the next run.
    pub fn grant_sources(&mut self, sources: Sources) {
        self.sources = sources;
    }

    /// Keep compiled programs in `dir` and reuse them (cache.rs).
    pub fn cache(&mut self, dir: PathBuf) {
        self.cache = Some(dir);
    }

    /// Test only: pretend the authority report lacks `cap.method`, so the call is "unexpected".
    #[cfg(feature = "test-hooks")]
    pub fn hide_from_report(&mut self, cap: &str, method: &str) {
        self.hidden = Some((cap.into(), method.into()));
    }

    /// Test only: make every `Dir.create` and `write` fail after the file is created.
    #[cfg(feature = "test-hooks")]
    pub fn fail_writes(&mut self) {
        self.fail_writes = true;
    }

    /// The message for a run that hit one of its limits, or `None` for any other error.
    pub fn explain(&self, e: &wasmtime::Error) -> Option<String> {
        crate::explain(e)
    }

    /// Parse and check `source` in the host: the report lines, or the diagnostics.
    pub fn check(&mut self, source: &str) -> Result<std::result::Result<Report, Vec<Diag>>> {
        self.check_files(&[single(source)])
    }

    /// Like `check` for `(name, source)` files, root first; see `lang::check_files`.
    pub fn check_files(&mut self, files: &[(String, String)]) -> Result<std::result::Result<Report, Vec<Diag>>> {
        Ok(crate::check_files(files))
    }

    /// The most linear memory the last run asked for, in bytes.
    pub fn peak_memory(&self) -> usize {
        self.peak_memory
    }

    /// Capability imports the last run called.
    pub fn import_calls(&self) -> u32 {
        self.calls
    }

    /// Check and compile `source`, then run `main` with `dirs` granted by name.
    pub fn run(&mut self, source: &str, dirs: Vec<(String, RealDir)>) -> Result<RunResult> {
        self.run_with(source, dirs, vec![])
    }

    /// Like `run`, with `https` granted by name too.
    pub fn run_with(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
    ) -> Result<RunResult> {
        self.run_files(&[single(source)], dirs, https)
    }

    /// Like `run_with` for `(name, source)` files, root first.
    pub fn run_files(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
    ) -> Result<RunResult> {
        self.run_inner(files, dirs, https, |_| Ok(Handles::default()), |_| {})
    }

    /// Run `main(req: Incoming, res: Outgoing, ..)` for one request, with `dirs` granted by name.
    /// Returns the outcome and the first response `main` sent, if any.
    pub fn run_web(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        request: Request,
    ) -> Result<(RunResult, Option<Response>)> {
        let mut sent = None;
        let r = self.run_inner(
            &[single(source)],
            dirs,
            vec![],
            |s| {
                let (slot, handles) = web_grants(s, request, ("req", "res"))?;
                sent = Some(slot);
                Ok(handles)
            },
            |_| {},
        )?;
        let response = sent.and_then(|s| s.lock().unwrap_or_else(PoisonError::into_inner).take());
        Ok((r, response))
    }

    /// Run `main(m: WsMessage, w: WsSend, ..)` for one event, with the two named `names` and
    /// `dirs` granted by name; `closed` makes every send `closed`. Returns the outcome and the
    /// frames `main` sent.
    pub fn run_ws(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        event: WsEvent,
        closed: bool,
        names: &(String, String),
    ) -> Result<(RunResult, Vec<WsFrame>)> {
        let mut sends = None;
        let r = self.run_inner(
            &[single(source)],
            dirs,
            vec![],
            |s| {
                let (out, handles) = ws_grants(s, event, closed, names)?;
                sends = Some(out);
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
        edit: impl FnOnce(&mut lang::ir::Module) + Send,
    ) -> Result<RunResult> {
        self.run_inner(&[single(source)], dirs, https, |_| Ok(Handles::default()), edit)
    }

    /// Check, compile and run on a thread with `RUN_STACK`, so the caller's stack does not matter.
    fn run_inner(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        handles: impl FnOnce(&mut State) -> Result<Handles> + Send,
        edit: impl FnOnce(&mut lang::ir::Module) + Send,
    ) -> Result<RunResult> {
        on_run_stack(|| self.run_here(files, dirs, https, handles, edit))
    }

    fn run_here(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        handles: impl FnOnce(&mut State) -> Result<Handles>,
        edit: impl FnOnce(&mut lang::ir::Module),
    ) -> Result<RunResult> {
        // Compiled before the store exists, so compiling is not charged to the run.
        let checked = compile(&self.engine, files, &self.limits, self.cache.as_ref(), edit)?;
        let mut store = new_store(&self.engine, &self.limits)?;
        #[cfg(feature = "test-hooks")]
        {
            store.data_mut().fail_writes = self.fail_writes;
        }
        let handles = handles(store.data_mut())?;
        let (kvs, sources) = (std::mem::take(&mut self.kvs), std::mem::take(&mut self.sources));
        let bound = push_grants(store.data_mut(), dirs, https, handles, kvs, sources, None)?;
        if let Some((out, paths)) = self.audit.take() {
            start_audit(store.data_mut(), out, &paths, &self.limits, files, &bound)?;
        }
        let result = match &checked {
            Err(diags) => Ok(Err(diags.clone())),
            Ok(program) => {
                #[cfg_attr(not(feature = "test-hooks"), allow(unused_mut))]
                let mut allowed = program.allowed.clone();
                #[cfg(feature = "test-hooks")]
                if let Some(h) = &self.hidden {
                    allowed.remove(h);
                }
                store.data_mut().allowed = allowed;
                match instantiate(&mut store, program, &bound)? {
                    Err(diags) => Ok(Err(diags)),
                    Ok((world, grants)) => call(&mut store, &world, &grants, program, &self.limits),
                }
            }
        };
        self.calls = store.data().calls;
        self.peak_memory = store.data().peak_memory;
        let outcome = match &result {
            Ok(Ok(o)) if !o.failed => "ok",
            Ok(Ok(_)) => "failed",
            _ => "error",
        };
        let fuel_used = self.limits.fuel.saturating_sub(store.get_fuel().unwrap_or(0));
        end_audit(store.data_mut(), fuel_used, outcome)?;
        result
    }
}

/// Serves one program: it is checked and compiled once, and every request or WebSocket event
/// gets a fresh instance from the pooling allocator (web decisions 3-5, 9). `Send + Sync`, so
/// threads may share `handle`; call it on threads with `RUN_STACK` (`listen` does).
pub struct Server {
    engine: Engine,
    program: Program,
    limits: Limits,
    _ticker: Ticker,
    /// Shared by every request.
    kvs: KvGrants,
    /// `Rng`, `Clock` and `Log` grants; every request gets fresh handles (a seeded `Rng`'s
    /// stream and a `Log` line's prefix carry the request number).
    sources: Sources,
    /// `main`'s `WsMessage` and `WsSend` parameter names, for a WebSocket server.
    ws: Option<(String, String)>,
    /// `main`'s `Incoming` and `Outgoing` parameter names, for an HTTP server.
    http: (String, String),
    /// The audit log every request writes to, if any, and how many runs have been numbered
    /// (requests and WebSocket events, from 1).
    audit: Option<SharedLog>,
    requests: AtomicU64,
}

/// Requests `--listen` runs at once by default (`ListenLimits::workers`): two per core.
pub fn default_workers() -> usize {
    2 * std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Component instances the pool holds at once, at least one per worker, so a request with a
/// worker is never refused for want of an instance.
fn pool_instances() -> u32 {
    u32::try_from(default_workers()).unwrap_or(u32::MAX).max(64)
}

impl Server {
    /// Check and compile `files` (root first) once, or return the diagnostics. Every request
    /// runs under `limits`; the pool reserves address space for `limits.memory` per memory,
    /// but touches only what the program uses.
    pub fn new(files: Vec<(String, String)>, limits: Limits) -> Result<std::result::Result<Self, Vec<Diag>>> {
        let lowered = match lower(&files, &limits, |_| {})? {
            Ok(l) => l,
            Err(diags) => return Ok(Err(diags)),
        };
        // Every pool limit is the component's own count times the instances the pool holds.
        let (core, memories, tables) = pool_needs(&lowered.wasm.component)?;
        let n = pool_instances();
        let mut pool = PoolingAllocationConfig::new();
        pool.total_component_instances(n)
            .max_core_instances_per_component(core)
            .total_core_instances(core.saturating_mul(n))
            .max_memories_per_component(memories)
            .total_memories(memories.saturating_mul(n))
            .max_tables_per_component(tables)
            .total_tables(tables.saturating_mul(n))
            .max_memory_size(limits.memory);
        let mut config = config::compile_config();
        set_stack(&mut config, limits.stack);
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        // One thread compiles: a parallel compile's peak memory stays resident on macOS while the
        // server idles (W13d): 28 MB idle RSS against 48 MB, for startup in about 160 ms, not 30.
        config.parallel_compilation(false);
        let engine = Engine::new(&config)?;
        let program = link(&engine, &files, lowered, None)?;
        let param = |ty: &str| {
            (program.params.iter())
                .find(|(_, t, _)| t == ty)
                .map(|(n, _, _)| n.clone())
        };
        let ws = param("WsMessage").zip(param("WsSend"));
        let http = (
            param("Incoming").unwrap_or("req".into()),
            param("Outgoing").unwrap_or("res".into()),
        );
        let ticker = Ticker(spawn_ticker(&engine));
        Ok(Ok(Server {
            engine,
            program,
            limits,
            _ticker: ticker,
            kvs: vec![],
            sources: Sources::default(),
            ws,
            http,
            audit: None,
            requests: AtomicU64::new(0),
        }))
    }

    /// Write one audit log (H8) for the server to `out`: the header now, then each request's calls
    /// and trailer, every line starting with the request's number (`"req":N`, from 1) and, for a
    /// WebSocket event, its connection (`"conn":N`). Grant the `Kv` stores first: the header lists
    /// them. `dir_paths` gives each `Dir` grant's path.
    pub fn audit(&mut self, out: impl Write + Send + 'static, dir_paths: DirPaths) -> std::io::Result<()> {
        let mut grants: Vec<(String, &str, Option<String>)> = Vec::new();
        grants.extend(dir_paths.into_iter().map(|(n, p)| (n, "Dir", Some(p))));
        let handles = match &self.ws {
            Some((m, w)) => [(m, "WsMessage"), (w, "WsSend")],
            None => [(&self.http.0, "Incoming"), (&self.http.1, "Outgoing")],
        };
        grants.extend(handles.into_iter().map(|(n, k)| (n.clone(), k, None)));
        grants.extend(self.kvs.iter().map(|(n, s)| (n.clone(), "Kv", Some(s.label()))));
        let src = &self.sources;
        grants.extend(src.rngs.iter().map(|(n, r)| (n.clone(), "Rng", Some(r.label()))));
        grants.extend(src.clocks.iter().map(|(n, c)| (n.clone(), "Clock", Some(c.label()))));
        grants.extend(src.logs.iter().map(|(n, l)| (n.clone(), "Log", Some(l.label.clone()))));
        let out: SharedLog = Arc::new(Mutex::new(Box::new(out)));
        write_line(&out, &audit_header(&self.program.files, &self.limits, &grants))?;
        self.audit = Some(out);
        Ok(())
    }

    /// Grant `kvs` by name to every request; all requests share each store. Panics after
    /// `audit`, whose header would then miss the stores.
    pub fn grant_kvs(&mut self, kvs: KvGrants) {
        assert!(
            self.audit.is_none(),
            "Server::grant_kvs after Server::audit: the audit header lists Kv grants"
        );
        self.kvs = kvs;
    }

    /// Grant `sources`' `Rng`s, `Clock`s and `Log`s by name to every request. Panics after
    /// `audit`, as `grant_kvs`.
    pub fn grant_sources(&mut self, sources: Sources) {
        assert!(
            self.audit.is_none(),
            "Server::grant_sources after Server::audit: the audit header lists the grants"
        );
        self.sources = sources;
    }

    /// The program's files, root first.
    pub fn files(&self) -> &[(String, String)] {
        &self.program.files
    }

    /// The authority report, as `kio check` prints it.
    pub fn report(&self) -> &str {
        &self.program.report
    }

    /// Run `main` on `request` in a fresh instance, with `dirs` granted by name. A program that
    /// sends no response, or traps, gets a 500 with an empty body and the cause on stderr. `Err`
    /// means no instance could be made (for example the pool is full).
    pub fn handle(&self, request: Request, dirs: Vec<(String, RealDir)>) -> std::result::Result<Response, String> {
        let mut sent = None;
        let result = self.run_fresh(dirs, None, |s| {
            let (slot, handles) = web_grants(s, request, (&self.http.0, &self.http.1))?;
            sent = Some(slot);
            Ok(handles)
        })?;
        let response = sent.and_then(|s| s.lock().unwrap_or_else(PoisonError::into_inner).take());
        match (result, response) {
            (Ok(_), Some(r)) => Ok(r),
            (Err(why), _) => {
                eprintln!("request failed: {why}");
                Ok(Response::internal_error())
            }
            _ => {
                eprintln!("request failed: no response sent");
                Ok(Response::internal_error())
            }
        }
    }

    /// The names of `main`'s `WsMessage` and `WsSend` parameters, if it takes both: the program
    /// is then a WebSocket server.
    pub fn ws_params(&self) -> Option<&(String, String)> {
        self.ws.as_ref()
    }

    /// Run `main` on one WebSocket event in a fresh instance, with `dirs` granted by name;
    /// `closed` makes every send `closed`. The frames to write, in order, or `None` if the run
    /// failed (a trap, a limit or `Err` from `main`; the cause goes to stderr). `Err` means no
    /// instance could be made.
    pub fn handle_ws(
        &self,
        event: WsEvent,
        closed: bool,
        dirs: Vec<(String, RealDir)>,
    ) -> std::result::Result<Option<Vec<WsFrame>>, String> {
        let names = self.ws.as_ref().ok_or("main takes no WsMessage")?;
        let mut sends = None;
        let conn = event.conn;
        let result = self.run_fresh(dirs, Some(conn), |s| {
            let (out, handles) = ws_grants(s, event, closed, names)?;
            sends = Some(out);
            Ok(handles)
        })?;
        let why = match result {
            Ok(o) if !o.failed => {
                let frames = sends.map_or(vec![], |s| {
                    std::mem::take(&mut s.lock().unwrap_or_else(PoisonError::into_inner).frames)
                });
                return Ok(Some(frames));
            }
            Ok(o) => format!("main returned Err: {}", o.value),
            Err(why) => why,
        };
        eprintln!("websocket event failed: {why}");
        Ok(None)
    }

    /// Run `main` once in a fresh instance with `dirs`, the shared `Kv` stores and the handles
    /// `grant` adds; with an audit log, as the next request (of WebSocket connection `conn`). The
    /// outcome, or why the run failed; `Err` means no instance could be made.
    fn run_fresh(
        &self,
        dirs: Vec<(String, RealDir)>,
        conn: Option<u64>,
        grant: impl FnOnce(&mut State) -> Result<Handles>,
    ) -> std::result::Result<std::result::Result<Outcome, String>, String> {
        let start = |e: wasmtime::Error| format!("cannot start instance: {e:#}");
        let mut store = new_store(&self.engine, &self.limits).map_err(start)?;
        store.data_mut().allowed = self.program.allowed.clone();
        let handles = grant(store.data_mut()).map_err(start)?;
        // Numbered before its grants: a seeded `Rng`'s stream and a `Log` line's prefix carry the
        // number. A run whose instance cannot be made (the pool is full) takes a number the
        // audit log then lacks.
        let req = self.requests.fetch_add(1, Relaxed) + 1;
        let (kvs, sources) = (self.kvs.clone(), self.sources.clone());
        let bound = push_grants(store.data_mut(), dirs, vec![], handles, kvs, sources, Some(req)).map_err(start)?;
        let diags = |d: Vec<Diag>| d.iter().map(|d| d.message.as_str()).collect::<Vec<_>>().join("; ");
        let started = instantiate(&mut store, &self.program, &bound).map_err(start)?;
        if let Some(out) = &self.audit {
            let tag = match conn {
                Some(c) => format!(r#""req":{req},"conn":{c},"#),
                None => format!(r#""req":{req},"#),
            };
            let out = out.clone();
            store.data_mut().audit = Some(Audit { out, seq: 0, tag });
        }
        let result = match started {
            Err(d) => Err(diags(d)),
            Ok((world, grants)) => match call(&mut store, &world, &grants, &self.program, &self.limits) {
                Ok(Ok(o)) => Ok(o),
                Err(e) => Err(crate::explain(&e).unwrap_or_else(|| format!("{e:#}"))),
                Ok(Err(d)) => Err(diags(d)),
            },
        };
        let outcome = match &result {
            Ok(o) if !o.failed => "ok",
            Ok(_) => "failed",
            Err(_) => "error",
        };
        let fuel_used = self.limits.fuel.saturating_sub(store.get_fuel().unwrap_or(0));
        if let Err(e) = end_audit(store.data_mut(), fuel_used, outcome) {
            return Ok(Err(format!("audit: cannot write the log: {e:#}")));
        }
        Ok(result)
    }
}
