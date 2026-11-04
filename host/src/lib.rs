//! Runs the guest component under Wasmtime. `dir` is backed by cap-std; `http` imports fail. Every import is counted.
use cap_std::ambient_authority;
use cap_std::fs::{Dir as RealDir, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::Result;
use wasmtime::{Engine, Store};
use wasmtime_wasi::{HostMonotonicClock, HostWallClock, WasiCtx, WasiCtxView, WasiView};

// Trappable imports let a host function stop the run by returning `Err`.
wasmtime::component::bindgen!({ world: "guest", path: "../caps.wit", imports: { default: trappable } });

const GUEST: &[u8] = include_bytes!("../../target/guest/wasm32-wasip2/release/guest.wasm");

/// The most one `Dir.read` returns; a larger file is `too-large`.
const READ_CAP: u64 = 10 << 20;

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
    call_limit: u32,
    /// Set by the call that exceeded `call_limit`.
    too_many_calls: bool,
    /// Bytes read plus written so far, and the most allowed.
    io: u64,
    io_limit: u64,
    /// Set by the call that exceeded `io_limit`.
    too_much_io: bool,
    #[cfg(feature = "test-hooks")]
    delay: Duration,
    #[cfg(feature = "test-hooks")]
    fail_writes: bool,
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
            call_limit: u32::MAX,
            too_many_calls: false,
            io: 0,
            io_limit: u64::MAX,
            too_much_io: false,
            #[cfg(feature = "test-hooks")]
            delay: Duration::ZERO,
            #[cfg(feature = "test-hooks")]
            fail_writes: false,
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
    /// Count one capability call; the call past `call_limit` stops the run.
    fn count_call(&mut self) -> Result<()> {
        self.calls = self.calls.saturating_add(1);
        if self.calls > self.call_limit {
            self.too_many_calls = true;
            return Err(wasmtime::Error::msg("too many host calls"));
        }
        Ok(())
    }

    /// Count `n` bytes of I/O; the call past `io_limit` stops the run.
    fn spend_io(&mut self, n: usize) -> Result<()> {
        self.io = self.io.saturating_add(n as u64);
        if self.io > self.io_limit {
            self.too_much_io = true;
            return Err(wasmtime::Error::msg("too much I/O"));
        }
        Ok(())
    }

    /// The directory behind a guest handle; `None` for a handle that is not a live `Granted`.
    fn dir(&mut self, h: &Resource<kio::caps::fs::Dir>) -> Option<&RealDir> {
        let g = self.table.get(&Resource::<Granted>::new_borrow(h.rep())).ok()?;
        Some(&g.0)
    }
}

impl kio::caps::fs::HostDir for State {
    fn read(&mut self, d: Resource<kio::caps::fs::Dir>, path: String) -> Result<Result<Vec<u8>, IoErr>> {
        self.count_call()?;
        #[cfg(feature = "test-hooks")]
        std::thread::sleep(self.delay);
        if let Err(e) = check_path(&path) {
            return Ok(Err(e));
        }
        let Some(dir) = self.dir(&d) else {
            return Ok(Err(IoErr::Denied));
        };
        // One byte past the cap tells a too-large file from an exact fit, even if it is growing.
        let mut data = Vec::new();
        let read = dir
            .open(&path)
            .and_then(|f| f.take(READ_CAP + 1).read_to_end(&mut data));
        // Charge what was read, even when it is refused.
        self.spend_io(data.len())?;
        Ok(match read {
            Ok(_) if data.len() as u64 > READ_CAP => Err(IoErr::TooLarge),
            Ok(_) => Ok(data),
            Err(e) => Err(io_err(&e)),
        })
    }

    fn create(&mut self, d: Resource<kio::caps::fs::Dir>, path: String, data: Vec<u8>) -> Result<Result<(), IoErr>> {
        self.count_call()?;
        // Charged before the file exists, so a refused call leaves nothing behind.
        self.spend_io(data.len())?;
        if let Err(e) = check_path(&path) {
            return Ok(Err(e));
        }
        #[cfg(feature = "test-hooks")]
        let fail_write = self.fail_writes;
        let Some(dir) = self.dir(&d) else {
            return Ok(Err(IoErr::Denied));
        };
        // `create_new` fails if the path exists, in the same call that creates it.
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        let mut file = match dir.open_with(&path, &opts) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(Err(IoErr::Denied)),
            Err(e) => return Ok(Err(io_err(&e))),
        };
        let written = file.write_all(&data);
        #[cfg(feature = "test-hooks")]
        let written = if fail_write {
            Err(io::Error::other("injected"))
        } else {
            written
        };
        if let Err(e) = written {
            // Remove the partial file so a retry is not refused as an overwrite.
            drop(file);
            let _ = dir.remove_file(&path);
            return Ok(Err(io_err(&e)));
        }
        Ok(Ok(()))
    }

    fn drop(&mut self, d: Resource<kio::caps::fs::Dir>) -> Result<()> {
        let _ = self.table.delete(Resource::<Granted>::new_own(d.rep()));
        Ok(())
    }
}

impl kio::caps::net::HostHttp for State {
    fn get(
        &mut self,
        _: Resource<kio::caps::net::Http>,
        _: String,
    ) -> Result<Result<Vec<u8>, kio::caps::net::HttpErr>> {
        self.count_call()?;
        Ok(Err(kio::caps::net::HttpErr::Denied))
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
    /// Capability calls.
    pub calls: u32,
    /// Bytes read plus written.
    pub io: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fuel: 1_000_000_000,
            timeout_secs: 10,
            stack: 2 << 20,
            memory: 256 << 20,
            calls: 10_000,
            io: 100 << 20,
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
        store.data_mut().call_limit = limits.calls;
        store.data_mut().io_limit = limits.io;
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
    /// Make every `Dir.create` fail after the file is created, to test the cleanup.
    #[cfg(feature = "test-hooks")]
    pub fn fail_writes(&mut self) {
        self.store.data_mut().fail_writes = true;
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
        if self.store.data().too_many_calls {
            return Some(format!(
                "too many host calls (limit {}; raise it with --calls)",
                l.calls
            ));
        }
        if self.store.data().too_much_io {
            return Some(format!("too much I/O (limit {} MB; raise it with --io)", l.io >> 20));
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
