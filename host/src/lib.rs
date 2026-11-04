//! Runs the guest component under Wasmtime. `dir` is backed by cap-std; `http` imports fail. Every import is counted.
use cap_std::ambient_authority;
use cap_std::fs::{Dir as RealDir, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::Result;
use wasmtime::{Engine, Store};
use wasmtime_wasi::{HostMonotonicClock, HostWallClock, WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({ world: "guest", path: "../caps.wit" });

const GUEST: &[u8] = include_bytes!("../../target/guest/wasm32-wasip2/release/guest.wasm");

/// A granted directory in the resource table. Every guest path goes through it.
struct Granted(RealDir);

/// Open a directory to grant: the host's only ambient filesystem call, made once per `--dir`.
pub fn open_dir(path: &Path) -> io::Result<RealDir> {
    RealDir::open_ambient_dir(path, ambient_authority())
}

/// Store state. `calls` counts every capability import the guest made.
pub struct State {
    pub calls: u32,
    pub peak_memory: usize,
    memory_limit: usize,
    /// Set when the guest asked for more memory than `memory_limit`.
    out_of_memory: bool,
    #[cfg(feature = "test-hooks")]
    delay: Duration,
    wasi: WasiCtx,
    table: ResourceTable,
}

/// A clock that never moves, so a run cannot observe time.
struct Frozen;

impl HostWallClock for Frozen {
    fn resolution(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

impl HostMonotonicClock for Frozen {
    fn resolution(&self) -> u64 {
        1
    }
    fn now(&self) -> u64 {
        0
    }
}

impl State {
    /// WASI is linked only because Rust's std for wasm32-wasip2 imports it. It grants nothing:
    /// no preopens, env, args, network or stdio, and frozen clocks and fixed seeds so every run of
    /// the same program behaves the same. kio code reaches the host only through caps.wit.
    fn new() -> Self {
        let wasi = WasiCtx::builder()
            .wall_clock(Frozen)
            .monotonic_clock(Frozen)
            .insecure_random_seed(0)
            .build();
        State {
            calls: 0,
            peak_memory: 0,
            memory_limit: usize::MAX,
            out_of_memory: false,
            #[cfg(feature = "test-hooks")]
            delay: Duration::ZERO,
            wasi,
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Enforces the memory limit and records the peak.
impl wasmtime::ResourceLimiter for State {
    fn memory_growing(&mut self, _: usize, desired: usize, _: Option<usize>) -> Result<bool> {
        if desired > self.memory_limit {
            self.out_of_memory = true;
            return Ok(false);
        }
        self.peak_memory = self.peak_memory.max(desired);
        Ok(true)
    }
    fn table_growing(&mut self, _: usize, _: usize, _: Option<usize>) -> Result<bool> {
        Ok(true)
    }
}

use kio::caps::fs::IoErr;

impl kio::caps::fs::Host for State {}
impl kio::caps::net::Host for State {}

/// Map a cap-std error. cap-std reports an escape as a custom `PermissionDenied`; a real OS
/// permission error has no inner error. Anything unexpected is `denied`.
fn io_err(e: &io::Error) -> IoErr {
    match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => IoErr::NotFound,
        io::ErrorKind::PermissionDenied if e.get_ref().is_some() => IoErr::InvalidPath,
        _ => IoErr::Denied,
    }
}

/// Empty paths and NUL bytes never reach cap-std.
fn check_path(path: &str) -> Result<(), IoErr> {
    if path.is_empty() || path.contains('\0') {
        return Err(IoErr::InvalidPath);
    }
    Ok(())
}

impl State {
    /// The directory behind a guest handle; `None` for a handle that is not a live `Granted`.
    fn dir(&mut self, h: &Resource<kio::caps::fs::Dir>) -> Option<&RealDir> {
        let g = self.table.get(&Resource::<Granted>::new_borrow(h.rep())).ok()?;
        Some(&g.0)
    }
}

impl kio::caps::fs::HostDir for State {
    fn read(&mut self, d: Resource<kio::caps::fs::Dir>, path: String) -> Result<Vec<u8>, IoErr> {
        self.calls += 1;
        #[cfg(feature = "test-hooks")]
        std::thread::sleep(self.delay);
        check_path(&path)?;
        let dir = self.dir(&d).ok_or(IoErr::Denied)?;
        dir.read(&path).map_err(|e| io_err(&e))
    }

    fn create(&mut self, d: Resource<kio::caps::fs::Dir>, path: String, data: Vec<u8>) -> Result<(), IoErr> {
        self.calls += 1;
        check_path(&path)?;
        let dir = self.dir(&d).ok_or(IoErr::Denied)?;
        // `create_new` fails if the path exists, in the same call that creates it.
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        let mut file = dir.open_with(&path, &opts).map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => IoErr::Denied,
            _ => io_err(&e),
        })?;
        file.write_all(&data).map_err(|e| io_err(&e))
    }

    fn drop(&mut self, d: Resource<kio::caps::fs::Dir>) -> Result<()> {
        let _ = self.table.delete(Resource::<Granted>::new_own(d.rep()));
        Ok(())
    }
}

impl kio::caps::net::HostHttp for State {
    fn get(&mut self, _: Resource<kio::caps::net::Http>, _: String) -> Result<Vec<u8>, kio::caps::net::HttpErr> {
        self.calls += 1;
        Err(kio::caps::net::HttpErr::Denied)
    }

    fn drop(&mut self, _: Resource<kio::caps::net::Http>) -> Result<()> {
        Ok(())
    }
}

/// Per-run limits. `stack` is bytes of Wasm stack: 2 MB by default, since the deepest recursion
/// the depth budget allows needs about 900 KB in a release host and more in a debug one (G4).
#[derive(Clone, Copy)]
pub struct Limits {
    pub fuel: u64,
    pub timeout_secs: u64,
    pub stack: usize,
    /// Bytes of guest linear memory.
    pub memory: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fuel: 1_000_000_000,
            timeout_secs: 10,
            stack: 2 << 20,
            memory: 256 << 20,
        }
    }
}

/// A fresh instance of the embedded guest.
pub struct Kio {
    store: Store<State>,
    guest: Guest,
    stop: Arc<AtomicBool>,
    limits: Limits,
}

impl Drop for Kio {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

impl Kio {
    /// A guest under `limits`. Running out of stack, fuel or time traps, which the host reports.
    pub fn new(limits: Limits) -> Result<Self> {
        let mut config = wasmtime::Config::new();
        config.max_wasm_stack(limits.stack);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;
        // Wasmtime checks epochs only in guest code: a slow host call is not cut short (H4).
        let stop = Arc::new(AtomicBool::new(false));
        let (e, s) = (engine.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                e.increment_epoch();
            }
        });
        let component = Component::new(&engine, GUEST)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        Guest::add_to_linker::<State, wasmtime::component::HasSelf<State>>(&mut linker, |s| s)?;
        let mut store = Store::new(&engine, State::new());
        store.data_mut().memory_limit = limits.memory;
        store.limiter(|s| s);
        store.set_fuel(limits.fuel)?;
        store.set_epoch_deadline(limits.timeout_secs.saturating_mul(10));
        store.epoch_deadline_trap();
        let guest = Guest::instantiate(&mut store, &component, &linker)?;
        Ok(Kio {
            store,
            guest,
            stop,
            limits,
        })
    }

    /// Make every `Dir.read` sleep first, to test the wall clock against a slow host call.
    #[cfg(feature = "test-hooks")]
    pub fn slow_reads(&mut self, d: Duration) {
        self.store.data_mut().delay = d;
    }
    /// The message for a run that hit one of its limits, or `None` for any other error.
    pub fn explain(&self, e: &wasmtime::Error) -> Option<String> {
        let l = &self.limits;
        // A refused memory grow makes the guest's allocator abort, so the trap itself is generic.
        if self.store.data().out_of_memory {
            return Some(format!(
                "out of memory (limit {} MB; raise it with --memory)",
                l.memory >> 20
            ));
        }
        Some(match e.downcast_ref::<wasmtime::Trap>()? {
            wasmtime::Trap::OutOfFuel => format!("out of fuel (limit {}; raise it with --fuel)", l.fuel),
            wasmtime::Trap::Interrupt => format!("timed out after {} s (raise it with --timeout)", l.timeout_secs),
            wasmtime::Trap::StackOverflow => {
                format!("stack overflow (limit {} KB; raise it with --stack)", l.stack >> 10)
            }
            _ => return None,
        })
    }

    /// Parse and check `source`: the report lines, or the diagnostics.
    pub fn check(&mut self, source: &str) -> Result<std::result::Result<Report, Vec<Diag>>> {
        self.guest.call_check(&mut self.store, source)
    }

    /// The most linear memory the guest has asked for, in bytes.
    pub fn peak_memory(&self) -> usize {
        self.store.data().peak_memory
    }

    /// Capability imports the guest has called so far.
    pub fn import_calls(&self) -> u32 {
        self.store.data().calls
    }

    /// Check `source` and run `main` with `dirs` granted by name.
    pub fn run(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
    ) -> Result<std::result::Result<Outcome, Vec<Diag>>> {
        let mut grants = Vec::new();
        for (name, dir) in dirs {
            let h = self.store.data_mut().table.push(Granted(dir))?;
            grants.push(DirGrant {
                name,
                dir: Resource::new_own(h.rep()),
            });
        }
        self.guest.call_run(&mut self.store, source, &grants, &[])
    }
}
