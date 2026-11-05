//! Runs the guest component under Wasmtime. `dir` is backed by cap-std; `http` is a ureq client fixed to one granted origin. Every import is counted.
use cap_std::ambient_authority;
use cap_std::fs::{Dir as RealDir, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::Result;
use wasmtime::{Engine, Store};
use wasmtime_wasi::{HostMonotonicClock, HostWallClock, WasiCtx, WasiCtxView, WasiView};

// Trappable imports let a host function stop the run by returning `Err`.
wasmtime::component::bindgen!({ world: "guest", path: "../caps.wit", imports: { default: trappable } });

mod config;

/// The guest, compiled to native code by `build.rs` with `config::compile_config`.
const GUEST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/guest.cwasm"));

/// The most one `Dir.read` returns; a larger file is `too-large`.
const READ_CAP: u64 = 10 << 20;

/// A granted directory in the resource table. Every guest path goes through it.
struct Granted(RealDir);

/// The one scheme, host and port an `Http` grant reaches. Guest input never changes it.
pub struct Origin {
    https: bool,
    host: String,
    port: u16,
}

impl Origin {
    /// Parse `host[:port]` (HTTPS), `http://host[:port]` or `https://host[:port]`; nothing else.
    pub fn parse(spec: &str) -> std::result::Result<Self, String> {
        let bad = |why: &str| format!("{spec:?}: {why}; expected host, http://host:port or https://host:port");
        let (https, rest) = match spec.split_once("://") {
            Some(("http", r)) => (false, r),
            Some(("https", r)) => (true, r),
            Some(_) => return Err(bad("unknown scheme")),
            None => (true, spec),
        };
        let (host, port) = match rest.strip_prefix('[') {
            Some(v6) => match v6.split_once(']') {
                Some((h, p)) if h.parse::<std::net::Ipv6Addr>().is_ok() => (format!("[{h}]"), p.strip_prefix(':')),
                _ => return Err(bad("bad IPv6 address")),
            },
            None => match rest.split_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (rest.to_string(), None),
            },
        };
        let hostname = host.trim_matches(['[', ']']);
        if hostname.is_empty() || !hostname.chars().all(|c| c.is_ascii_alphanumeric() || "-.:".contains(c)) {
            return Err(bad("bad host"));
        }
        let default = if https { 443 } else { 80 };
        let port = match port {
            None => default,
            Some(p) => p
                .parse::<u16>()
                .ok()
                .filter(|&p| p > 0)
                .ok_or_else(|| bad("bad port"))?,
        };
        Ok(Origin { https, host, port })
    }

    /// `scheme://host[:port]` followed by the guest's `path`, or `None` if the path could name
    /// another authority or smuggle a header: it must start with a single `/` and hold no
    /// backslash, `#`, space or control character.
    fn url(&self, path: &str) -> Option<String> {
        let ok = path.starts_with('/')
            && !path.starts_with("//")
            && path.len() <= PATH_MAX
            && !path.chars().any(|c| c.is_ascii_control() || " \\#".contains(c));
        let default = if self.https { 443 } else { 80 };
        let port = if self.port == default {
            String::new()
        } else {
            format!(":{}", self.port)
        };
        ok.then(|| {
            format!(
                "{}://{}{port}{path}",
                if self.https { "https" } else { "http" },
                self.host
            )
        })
    }
}

/// The longest wait for one request; the run deadline can cut it shorter.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// GET `url` with no redirects, no proxy and `timeout` for the whole exchange. A non-2xx status
/// is `Status`; the body is read through the same cap as `Dir.read`. Returns the bytes read too.
fn fetch(url: &str, timeout: Duration) -> (Vec<u8>, std::result::Result<(), kio::caps::net::HttpErr>) {
    use kio::caps::net::HttpErr;
    let timed_out = |e: &io::Error| e.kind() == io::ErrorKind::TimedOut;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None) // ureq's default reads proxy variables from the environment
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .into();
    let mut data = Vec::new();
    let resp = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Timeout(_)) => return (data, Err(HttpErr::Timeout)),
        Err(ureq::Error::Io(e)) if timed_out(&e) => return (data, Err(HttpErr::Timeout)),
        Err(_) => return (data, Err(HttpErr::Denied)),
    };
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return (data, Err(HttpErr::Status(status)));
    }
    let read = resp.into_body().into_reader().take(READ_CAP + 1).read_to_end(&mut data);
    let result = match read {
        Ok(_) if data.len() as u64 > READ_CAP => Err(HttpErr::TooLarge),
        Ok(_) => Ok(()),
        Err(e) if timed_out(&e) => Err(HttpErr::Timeout),
        Err(_) => Err(HttpErr::Denied),
    };
    (data, result)
}

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
    /// When the run's wall clock expires, and whether a host call was cut off by it.
    deadline: Instant,
    timed_out: bool,
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
            deadline: Instant::now(),
            timed_out: false,
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
/// Longest path the host accepts, so path handling cannot cost unbounded host memory or time.
const PATH_MAX: usize = 4096;

fn check_path(path: &str) -> Result<(), IoErr> {
    if path.is_empty() || path.len() > PATH_MAX || path.contains('\0') {
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

    /// A private handle to the directory behind a guest handle, for a worker thread; `None` for a
    /// handle that is not a live `Granted` (or cannot be cloned).
    fn dir(&mut self, h: &Resource<kio::caps::fs::Dir>) -> Option<RealDir> {
        let g = self.table.get(&Resource::<Granted>::new_borrow(h.rep())).ok()?;
        g.0.try_clone().ok()
    }

    /// Run `work` on a detached thread and wait for it until the run's deadline; past it, stop the
    /// run. The thread finishes in the background and its result is dropped.
    fn within<T: Send + 'static>(&mut self, work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| match e {
                RecvTimeoutError::Timeout => {
                    self.timed_out = true;
                    wasmtime::Error::msg("host call timed out")
                }
                RecvTimeoutError::Disconnected => wasmtime::Error::msg("host call failed"),
            })
    }
}

impl kio::caps::fs::HostDir for State {
    fn read(&mut self, d: Resource<kio::caps::fs::Dir>, path: String) -> Result<Result<Vec<u8>, IoErr>> {
        self.count_call()?;
        if let Err(e) = check_path(&path) {
            return Ok(Err(e));
        }
        let Some(dir) = self.dir(&d) else {
            return Ok(Err(IoErr::Denied));
        };
        // One byte past the cap tells a too-large file from an exact fit, even if it is growing.
        let (data, read) = self.within(move || {
            let mut data = Vec::new();
            let read = dir
                .open(&path)
                .and_then(|f| f.take(READ_CAP + 1).read_to_end(&mut data));
            (data, read)
        })?;
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
        // After a timeout the worker may still create the file, if it was only slow to open it.
        self.within(move || {
            // `create_new` fails if the path exists, in the same call that creates it.
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            let mut file = match dir.open_with(&path, &opts) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(IoErr::Denied),
                Err(e) => return Err(io_err(&e)),
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
                return Err(io_err(&e));
            }
            Ok(())
        })
    }

    fn drop(&mut self, d: Resource<kio::caps::fs::Dir>) -> Result<()> {
        let _ = self.table.delete(Resource::<Granted>::new_own(d.rep()));
        Ok(())
    }
}

impl kio::caps::net::HostHttp for State {
    fn get(
        &mut self,
        h: Resource<kio::caps::net::Http>,
        path: String,
    ) -> Result<Result<Vec<u8>, kio::caps::net::HttpErr>> {
        use kio::caps::net::HttpErr;
        self.count_call()?;
        let url = match self.table.get(&Resource::<Origin>::new_borrow(h.rep())) {
            Ok(origin) => origin.url(&path),
            Err(_) => None,
        };
        let Some(url) = url else {
            return Ok(Err(HttpErr::Denied));
        };
        let timeout = HTTP_TIMEOUT.min(self.deadline.saturating_duration_since(Instant::now()));
        let (data, result) = self.within(move || fetch(&url, timeout))?;
        // Charge what was read, even when it is refused.
        self.spend_io(data.len())?;
        Ok(result.map(|()| data))
    }

    fn drop(&mut self, h: Resource<kio::caps::net::Http>) -> Result<()> {
        let _ = self.table.delete(Resource::<Origin>::new_own(h.rep()));
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

/// A fresh instance of the embedded guest, for one run: fuel, deadline, call and I/O counters
/// are not reset, so make a new `Kio` per run.
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
        let mut config = config::compile_config();
        config.max_wasm_stack(limits.stack);
        let engine = Engine::new(&config)?;
        // Wasmtime checks epochs only in guest code: host calls are bounded separately, by `State::within`.
        let stop = Arc::new(AtomicBool::new(false));
        let (e, s) = (engine.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                e.increment_epoch();
            }
        });
        // SAFETY: GUEST is the output of `precompile_component` from our own build script, not
        // untrusted input; Wasmtime still checks it matches this engine's version and settings.
        let component = unsafe { Component::deserialize(&engine, GUEST)? };
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        Guest::add_to_linker::<State, wasmtime::component::HasSelf<State>>(&mut linker, |s| s)?;
        let mut store = Store::new(&engine, State::new());
        store.data_mut().memory_limit = limits.memory;
        store.data_mut().call_limit = limits.calls;
        store.data_mut().io_limit = limits.io;
        store.limiter(|s| s);
        store.set_fuel(limits.fuel)?;
        // A timeout too large to represent caps at a year rather than panicking.
        let now = Instant::now();
        let year = Duration::from_secs(365 * 24 * 3600);
        let wait = Duration::from_secs(limits.timeout_secs).min(year);
        store.data_mut().deadline = now.checked_add(wait).unwrap_or(now);
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
        if self.store.data().timed_out {
            return Some(format!(
                "timed out after {} s (raise it with --timeout)",
                l.timeout_secs
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
        self.run_with(source, dirs, vec![])
    }

    /// Like `run`, with `https` granted by name too.
    pub fn run_with(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
    ) -> Result<std::result::Result<Outcome, Vec<Diag>>> {
        let mut grants = Vec::new();
        for (name, dir) in dirs {
            let h = self.store.data_mut().table.push(Granted(dir))?;
            grants.push(DirGrant {
                name,
                dir: Resource::new_own(h.rep()),
            });
        }
        let mut http_grants = Vec::new();
        for (name, origin) in https {
            let h = self.store.data_mut().table.push(origin)?;
            http_grants.push(HttpGrant {
                name,
                http: Resource::new_own(h.rep()),
            });
        }
        self.guest.call_run(&mut self.store, source, &grants, &http_grants)
    }
}
