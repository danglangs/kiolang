//! Runs the guest component under Wasmtime. `dir` is backed by cap-std; `http` is a ureq client fixed to one granted origin; `incoming` and `outgoing` hold one request and its first response. Every import is counted and, if asked, written to an audit log.
use cap_std::ambient_authority;
use cap_std::fs::{Dir as RealDir, OpenOptions};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, PoisonError};
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

/// The one scheme, host and port an `Http` grant reaches. Guest input never changes it. It also
/// holds the run's pinned DNS answer; clones share it.
#[derive(Clone, Debug)]
pub struct Origin {
    https: bool,
    host: String,
    port: u16,
    /// The host's addresses from its first resolution; every later request and hop reuses them.
    pinned: Arc<Mutex<Option<Vec<SocketAddr>>>>,
    /// Real lookups made (at most one per run).
    lookups: Arc<AtomicU32>,
    /// Non-public addresses are allowed: the grant is an IP literal.
    any_addr: bool,
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
        let any_addr = hostname.parse::<IpAddr>().is_ok();
        Ok(Origin {
            https,
            host,
            port,
            pinned: Arc::default(),
            lookups: Arc::default(),
            any_addr,
        })
    }

    /// `scheme://host:port`, for the audit log.
    fn target(&self) -> String {
        format!(
            "{}://{}:{}",
            if self.https { "https" } else { "http" },
            self.host,
            self.port
        )
    }

    /// Allow loopback and private addresses for a named host, so tests can use `localhost`.
    #[cfg(feature = "test-hooks")]
    pub fn allow_private_addrs(&mut self) {
        self.any_addr = true;
    }

    /// How many times the host name was actually looked up.
    #[cfg(feature = "test-hooks")]
    pub fn lookups(&self) -> u32 {
        self.lookups.load(Relaxed)
    }

    /// Resolve `location` against `current`; `None` unless the result is exactly this origin
    /// (scheme, host and port, no credentials), so a redirect can never leave the grant.
    fn follow(&self, current: &str, location: &str) -> Option<String> {
        let mut next = url::Url::parse(current).ok()?.join(location).ok()?;
        let same = (next.scheme() == "https") == self.https
            && next.host_str().is_some_and(|h| h.eq_ignore_ascii_case(&self.host))
            && next.port_or_known_default() == Some(self.port)
            && next.username().is_empty()
            && next.password().is_none();
        next.set_fragment(None);
        same.then(|| next.into())
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

/// Redirects followed per `Http.get`; the next one is `denied`.
const MAX_REDIRECTS: usize = 5;

/// Whether a connection may go to `ip`: not loopback, private, link-local, unspecified,
/// multicast or shared (CGNAT) space. IPv4-mapped and NAT64 IPv6 addresses are judged as the IPv4
/// address inside.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || o[0] == 0
                || (o[0] == 100 && o[1] & 0xc0 == 64))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(v4.into());
            }
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [a, b] = [s[6].to_be_bytes(), s[7].to_be_bytes()];
                return is_public(Ipv4Addr::new(a[0], a[1], b[0], b[1]).into());
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || s[0] & 0xfe00 == 0xfc00 // unique local
                || s[0] & 0xffc0 == 0xfe80) // link-local
        }
    }
}

/// Resolves only the grant's host, once per run: later requests and hops reuse the first answer,
/// so DNS rebinding cannot change where the run connects. Non-public addresses are dropped.
#[derive(Debug)]
struct Pinned(Origin);

impl ureq::unversioned::resolver::Resolver for Pinned {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let o = &self.0;
        if !uri.host().is_some_and(|h| h.eq_ignore_ascii_case(&o.host)) {
            return Err(ureq::Error::HostNotFound);
        }
        let mut pinned = o.pinned.lock().unwrap_or_else(PoisonError::into_inner);
        let addrs = match &*pinned {
            Some(addrs) => addrs,
            None => {
                o.lookups.fetch_add(1, Relaxed);
                let found = ureq::unversioned::resolver::DefaultResolver::default().resolve(uri, config, timeout)?;
                let kept: Vec<_> = found
                    .iter()
                    .copied()
                    .filter(|a| o.any_addr || is_public(a.ip()))
                    .collect();
                pinned.insert(kept) // an all-refused answer is pinned too
            }
        };
        if addrs.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        let mut out = self.empty();
        addrs.iter().for_each(|a| out.push(*a));
        Ok(out)
    }
}

/// GET `url` with no redirects, no proxy and `timeout` for the whole exchange, connecting only to
/// `origin`'s pinned addresses. A non-2xx status is `Status`; the body is read through the same cap
/// as `Dir.read`. Returns the bytes read, and the `Location` if the answer was a redirect.
fn fetch(
    origin: &Origin,
    url: &str,
    timeout: Duration,
) -> (
    Vec<u8>,
    std::result::Result<(), kio::caps::net::HttpErr>,
    Option<String>,
) {
    use kio::caps::net::HttpErr;
    let timed_out = |e: &io::Error| e.kind() == io::ErrorKind::TimedOut;
    let config = ureq::Agent::config_builder()
        .proxy(None) // ureq's default reads proxy variables from the environment
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build();
    let agent = ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::default(),
        Pinned(origin.clone()),
    );
    let mut data = Vec::new();
    let resp = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Timeout(_)) => return (data, Err(HttpErr::Timeout), None),
        Err(ureq::Error::Io(e)) if timed_out(&e) => return (data, Err(HttpErr::Timeout), None),
        Err(_) => return (data, Err(HttpErr::Denied), None),
    };
    let status = resp.status().as_u16();
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        // A redirect without a usable `Location` is refused.
        return match resp.headers().get("location").and_then(|v| v.to_str().ok()) {
            Some(loc) => (data, Err(HttpErr::Status(status)), Some(loc.to_string())),
            None => (data, Err(HttpErr::Denied), None),
        };
    }
    if !(200..300).contains(&status) {
        return (data, Err(HttpErr::Status(status)), None);
    }
    let read = resp.into_body().into_reader().take(READ_CAP + 1).read_to_end(&mut data);
    let result = match read {
        Ok(_) if data.len() as u64 > READ_CAP => Err(HttpErr::TooLarge),
        Ok(_) => Ok(()),
        Err(e) if timed_out(&e) => Err(HttpErr::Timeout),
        Err(_) => Err(HttpErr::Denied),
    };
    (data, result, None)
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
    audit: Option<Audit>,
    /// Set by a call missing from the authority report.
    unexpected: bool,
    /// `(capability parameter, method)` pairs on `main`'s report line; any other call is a checker
    /// bug. Set at the start of every run, whether or not a log is written.
    allowed: HashSet<(String, String)>,
    /// Grant name and kind of each handle.
    handles: HashMap<u32, (String, &'static str)>,
    audit_msg: String,
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
            audit: None,
            unexpected: false,
            allowed: HashSet::new(),
            handles: HashMap::new(),
            audit_msg: String::new(),
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
impl kio::caps::web::Host for State {}

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

/// The audit log (H8): one JSON line per host call. Sizes only, never contents.
struct Audit {
    out: Box<dyn Write + Send>,
    seq: u64,
}

impl Audit {
    /// Write one line and flush it. `State::call` does this after the work, so a crash of the host
    /// process inside a host call can lose that call's line.
    fn line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.out, "{line}")?;
        self.out.flush()
    }
}

/// A JSON string literal.
fn json(s: &str) -> String {
    let mut o = String::from('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if c < ' ' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The `(parameter, method)` pairs on `main`'s line of an authority report, such as
/// `main : input read, out create`.
fn main_calls(report: &str) -> HashSet<(String, String)> {
    let line = report
        .lines()
        .find_map(|l| l.strip_prefix("main")?.trim_start().strip_prefix(':'));
    let mut pairs = HashSet::new();
    for part in line.map(str::trim).filter(|&l| l != "none").unwrap_or("").split(',') {
        let mut words = part.split_whitespace();
        if let Some(cap) = words.next() {
            pairs.extend(words.map(|m| (cap.to_string(), m.to_string())));
        }
    }
    pairs
}

/// For calls that cannot fail.
fn ok<T>(_: &T) -> String {
    "ok".into()
}

/// `ok`, or the error variant as caps.wit spells it: `TooLarge` is `too-large`, `Status(404)`
/// is `status(404)`.
fn outcome<T, E: std::fmt::Debug>(r: &std::result::Result<T, E>) -> String {
    let Err(e) = r else { return "ok".into() };
    let mut out = String::new();
    let name = format!("{e:?}");
    let name = name.rsplit("::").next().unwrap_or_default();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

impl State {
    /// Run one host function `f` as a counted, checked call. Every host function goes through
    /// here, so none can skip the quota, the report check or the log. `arg` is the path (or the
    /// method name when there is none); `result` names the call's outcome. With a log, the call's
    /// line is written after the work and before this returns, so a crash of the host process
    /// inside a host call can lose that one line. A call refused by a quota, the deadline or the
    /// report check is logged with `stopped:<why>` before the run stops.
    fn call<T>(
        &mut self,
        handle: u32,
        method: &str,
        arg: &str,
        f: impl FnOnce(&mut Self) -> Result<T>,
        result: impl FnOnce(&T) -> String,
    ) -> Result<T> {
        let (start, io) = (Instant::now(), self.io);
        let r = self
            .count_call()
            .and_then(|()| self.check_report(handle, method))
            .and_then(|()| f(self));
        let Some(mut a) = self.audit.take() else { return r };
        let (cap, kind) = self.handles.get(&handle).cloned().unwrap_or(("?".into(), "?"));
        let why = match &r {
            Ok(v) => result(v),
            Err(_) if self.unexpected => "stopped:unexpected".into(),
            Err(_) if self.too_many_calls => "stopped:calls".into(),
            Err(_) if self.too_much_io => "stopped:io".into(),
            Err(_) if self.timed_out => "stopped:time".into(),
            Err(_) => "stopped:error".into(),
        };
        a.seq += 1;
        let line = format!(
            r#"{{"event":"call","seq":{},"cap":{},"kind":"{kind}","method":{},"arg":{},"bytes":{},"result":{}{},"ms":{:.1}}}"#,
            a.seq,
            json(&cap),
            json(method),
            json(&arg.chars().take(256).collect::<String>()),
            self.io - io,
            json(&why),
            if self.unexpected { r#","unexpected":true"# } else { "" },
            start.elapsed().as_secs_f64() * 1000.0
        );
        let written = a.line(&line);
        self.audit = Some(a);
        let r = r?;
        written.map_err(|e| wasmtime::Error::msg(format!("audit: cannot write the log: {e}")))?;
        Ok(r)
    }

    /// Stop a call whose `(capability parameter, method)` is not on `main`'s
    /// report line: the checker was wrong.
    fn check_report(&mut self, handle: u32, method: &str) -> Result<()> {
        let cap = self.handles.get(&handle).map_or("?", |h| h.0.as_str());
        if self.allowed.contains(&(cap.to_string(), method.to_string())) {
            return Ok(());
        }
        self.unexpected = true;
        self.audit_msg = format!("audit: call `{cap}.{method}` not in the authority report (a checker bug)");
        Err(wasmtime::Error::msg(self.audit_msg.clone()))
    }

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

    /// The request behind a guest handle.
    fn request(&self, h: &Resource<kio::caps::web::Incoming>) -> Result<&Request> {
        Ok(self.table.get(&Resource::<Request>::new_borrow(h.rep()))?)
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
        let arg = path.clone();
        self.call(d.rep(), "read", &arg, |s| s.read_file(&d, path), outcome)
    }

    fn create(&mut self, d: Resource<kio::caps::fs::Dir>, path: String, data: Vec<u8>) -> Result<Result<(), IoErr>> {
        let arg = path.clone();
        self.call(d.rep(), "create", &arg, |s| s.create_file(&d, path, data), outcome)
    }

    fn drop(&mut self, d: Resource<kio::caps::fs::Dir>) -> Result<()> {
        let _ = self.table.delete(Resource::<Granted>::new_own(d.rep()));
        Ok(())
    }
}

impl State {
    fn read_file(&mut self, d: &Resource<kio::caps::fs::Dir>, path: String) -> Result<Result<Vec<u8>, IoErr>> {
        if let Err(e) = check_path(&path) {
            return Ok(Err(e));
        }
        let Some(dir) = self.dir(d) else {
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

    fn create_file(
        &mut self,
        d: &Resource<kio::caps::fs::Dir>,
        path: String,
        data: Vec<u8>,
    ) -> Result<Result<(), IoErr>> {
        // Charged before the file exists, so a refused call leaves nothing behind.
        self.spend_io(data.len())?;
        if let Err(e) = check_path(&path) {
            return Ok(Err(e));
        }
        #[cfg(feature = "test-hooks")]
        let fail_write = self.fail_writes;
        let Some(dir) = self.dir(d) else {
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
}

impl kio::caps::net::HostHttp for State {
    fn get(
        &mut self,
        h: Resource<kio::caps::net::Http>,
        path: String,
    ) -> Result<Result<Vec<u8>, kio::caps::net::HttpErr>> {
        let arg = path.clone();
        self.call(h.rep(), "get", &arg, |s| s.get_url(&h, path), outcome)
    }

    fn drop(&mut self, h: Resource<kio::caps::net::Http>) -> Result<()> {
        let _ = self.table.delete(Resource::<Origin>::new_own(h.rep()));
        Ok(())
    }
}

impl State {
    fn get_url(
        &mut self,
        h: &Resource<kio::caps::net::Http>,
        path: String,
    ) -> Result<Result<Vec<u8>, kio::caps::net::HttpErr>> {
        use kio::caps::net::HttpErr;
        let Ok(origin) = self.table.get(&Resource::<Origin>::new_borrow(h.rep())).cloned() else {
            return Ok(Err(HttpErr::Denied));
        };
        let Some(mut url) = origin.url(&path) else {
            return Ok(Err(HttpErr::Denied));
        };
        for hop in 0..=MAX_REDIRECTS {
            if hop > 0 {
                self.count_call()?; // each followed redirect is a call
            }
            let timeout = HTTP_TIMEOUT.min(self.deadline.saturating_duration_since(Instant::now()));
            let (o, u) = (origin.clone(), url.clone());
            let (data, result, location) = self.within(move || fetch(&o, &u, timeout))?;
            // Charge what was read, even when it is refused.
            self.spend_io(data.len())?;
            let Some(location) = location else {
                return Ok(result.map(|()| data));
            };
            match origin.follow(&url, &location) {
                Some(next) if hop < MAX_REDIRECTS => url = next,
                _ => break,
            }
        }
        Ok(Err(HttpErr::Denied))
    }
}

/// One HTTP request as the host parsed it. `Kio::run_web` grants it as `req`.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub header_names: Vec<String>,
    pub header_values: Vec<String>,
    pub body: Vec<u8>,
}

/// The first response a run sent. `Kio::run_web` grants the sender as `res`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What `run` returns: the outcome, or the diagnostics.
type RunResult = std::result::Result<Outcome, Vec<Diag>>;

/// Where `outgoing.send` records the response; shared so the host can read it after the run.
type Sent = Arc<Mutex<Option<Response>>>;

/// Whether `s` could split a header line.
fn has_crlf(s: &str) -> bool {
    s.contains(['\r', '\n'])
}

impl kio::caps::web::HostIncoming for State {
    fn method(&mut self, r: Resource<kio::caps::web::Incoming>) -> Result<String> {
        self.call(r.rep(), "method", "method", |s| Ok(s.request(&r)?.method.clone()), ok)
    }

    fn target(&mut self, r: Resource<kio::caps::web::Incoming>) -> Result<String> {
        self.call(r.rep(), "target", "target", |s| Ok(s.request(&r)?.target.clone()), ok)
    }

    fn header_names(&mut self, r: Resource<kio::caps::web::Incoming>) -> Result<Vec<String>> {
        self.call(
            r.rep(),
            "header_names",
            "header_names",
            |s| Ok(s.request(&r)?.header_names.clone()),
            ok,
        )
    }

    fn header_values(&mut self, r: Resource<kio::caps::web::Incoming>) -> Result<Vec<String>> {
        self.call(
            r.rep(),
            "header_values",
            "header_values",
            |s| Ok(s.request(&r)?.header_values.clone()),
            ok,
        )
    }

    fn body(&mut self, r: Resource<kio::caps::web::Incoming>) -> Result<Vec<u8>> {
        self.call(
            r.rep(),
            "body",
            "body",
            |s| {
                let body = s.request(&r)?.body.clone();
                s.spend_io(body.len())?;
                Ok(body)
            },
            ok,
        )
    }

    fn drop(&mut self, r: Resource<kio::caps::web::Incoming>) -> Result<()> {
        let _ = self.table.delete(Resource::<Request>::new_own(r.rep()));
        Ok(())
    }
}

impl kio::caps::web::HostOutgoing for State {
    fn send(
        &mut self,
        r: Resource<kio::caps::web::Outgoing>,
        status: u16,
        header_names: Vec<String>,
        header_values: Vec<String>,
        body: Vec<u8>,
    ) -> Result<Result<(), kio::caps::web::WebErr>> {
        self.call(
            r.rep(),
            "send",
            "send",
            |s| s.send_response(&r, status, header_names, header_values, body),
            outcome,
        )
    }

    fn drop(&mut self, r: Resource<kio::caps::web::Outgoing>) -> Result<()> {
        let _ = self.table.delete(Resource::<Sent>::new_own(r.rep()));
        Ok(())
    }
}

impl State {
    fn send_response(
        &mut self,
        r: &Resource<kio::caps::web::Outgoing>,
        status: u16,
        header_names: Vec<String>,
        header_values: Vec<String>,
        body: Vec<u8>,
    ) -> Result<Result<(), kio::caps::web::WebErr>> {
        use kio::caps::web::WebErr;
        self.spend_io(body.len())?;
        let sent = self.table.get(&Resource::<Sent>::new_borrow(r.rep()))?.clone();
        let mut slot = sent.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return Ok(Err(WebErr::AlreadySent));
        }
        // An invalid response is not recorded, so the guest may send a valid one.
        let valid = (100..=599).contains(&status)
            && header_names.len() == header_values.len()
            && header_names.iter().chain(&header_values).all(|s| !has_crlf(s));
        if !valid {
            return Ok(Err(WebErr::Invalid));
        }
        let headers = header_names.into_iter().zip(header_values).collect();
        *slot = Some(Response { status, headers, body });
        Ok(Ok(()))
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

/// `(name, path)` of each `Dir` grant, for the audit header.
pub type DirPaths = Vec<(String, String)>;

/// A fresh instance of the embedded guest, for one run: fuel, deadline, call and I/O counters
/// are not reset, so make a new `Kio` per run.
pub struct Kio {
    store: Store<State>,
    guest: Guest,
    stop: Arc<AtomicBool>,
    limits: Limits,
    /// The audit log and the `(name, path)` of each `Dir` grant, until a run starts.
    audit: Option<(Box<dyn Write + Send>, DirPaths)>,
    /// Test only: a `(parameter, method)` pair to leave out of the authority report.
    #[cfg(feature = "test-hooks")]
    hidden: Option<(String, String)>,
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
            audit: None,
            #[cfg(feature = "test-hooks")]
            hidden: None,
        })
    }

    /// Write the audit log (H8) of the next run to `out`: a header, one line per host call, a
    /// trailer. `dir_paths` gives the `(name, path)` of each `Dir` grant for the header, since a
    /// handle does not know its path. Sizes only, never contents.
    pub fn audit(&mut self, out: impl Write + Send + 'static, dir_paths: DirPaths) {
        self.audit = Some((Box::new(out), dir_paths));
    }

    /// Test only: pretend the authority report lacks `cap.method`, so the call is "unexpected".
    #[cfg(feature = "test-hooks")]
    pub fn hide_from_report(&mut self, cap: &str, method: &str) {
        self.hidden = Some((cap.into(), method.into()));
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
        if self.store.data().unexpected {
            return Some(self.store.data().audit_msg.clone());
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
        self.check_files(&[single(source)])
    }

    /// Like `check` for `(name, source)` files, root first; see `lang::check_files`.
    pub fn check_files(&mut self, files: &[(String, String)]) -> Result<std::result::Result<Report, Vec<Diag>>> {
        self.guest.call_check(&mut self.store, &file_list(files))
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
        self.run_files(&[single(source)], dirs, https)
    }

    /// Like `run_with` for `(name, source)` files, root first.
    pub fn run_files(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
    ) -> Result<std::result::Result<Outcome, Vec<Diag>>> {
        self.run_all(files, dirs, https, vec![], vec![])
    }

    /// Run `main(req: Incoming, res: Outgoing, ..)` for one request, with `dirs` granted by name.
    /// Returns the outcome and the first response `main` sent, if any.
    pub fn run_web(
        &mut self,
        source: &str,
        dirs: Vec<(String, RealDir)>,
        request: Request,
    ) -> Result<(RunResult, Option<Response>)> {
        let sent = Sent::default();
        let req = self.store.data_mut().table.push(request)?;
        let res = self.store.data_mut().table.push(sent.clone())?;
        let incomings = vec![("req".to_string(), Resource::new_own(req.rep()))];
        let outgoings = vec![("res".to_string(), Resource::new_own(res.rep()))];
        let outcome = self.run_all(&[single(source)], dirs, vec![], incomings, outgoings)?;
        let response = sent.lock().unwrap_or_else(PoisonError::into_inner).take();
        Ok((outcome, response))
    }

    fn run_all(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        incomings: Vec<(String, Resource<kio::caps::web::Incoming>)>,
        outgoings: Vec<(String, Resource<kio::caps::web::Outgoing>)>,
    ) -> Result<std::result::Result<Outcome, Vec<Diag>>> {
        self.arm_report(files);
        self.start_audit(files, &dirs, &https, &incomings, &outgoings)?;
        let result = self.run_granted(files, dirs, https, incomings, outgoings);
        self.end_audit(&result)?;
        result
    }

    /// Set the `(parameter, method)` pairs a call may use. The host checks the files itself, so
    /// they do not come from the guest.
    fn arm_report(&mut self, files: &[(String, String)]) {
        let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        let report = lang::check_files(&pairs, &lang::CapTable::v0()).map(|p| lang::report(&p));
        #[cfg_attr(not(feature = "test-hooks"), allow(unused_mut))]
        let mut allowed = main_calls(report.as_deref().unwrap_or(""));
        #[cfg(feature = "test-hooks")]
        if let Some(h) = &self.hidden {
            allowed.remove(h);
        }
        self.store.data_mut().allowed = allowed;
    }

    /// Write the audit header and arm the per-call log, if `audit` was called.
    fn start_audit(
        &mut self,
        files: &[(String, String)],
        dirs: &[(String, RealDir)],
        https: &[(String, Origin)],
        incomings: &[(String, Resource<kio::caps::web::Incoming>)],
        outgoings: &[(String, Resource<kio::caps::web::Outgoing>)],
    ) -> Result<()> {
        let Some((out, dir_paths)) = self.audit.take() else {
            return Ok(());
        };
        let mut grant_json = Vec::new();
        let mut grant = |name: &str, kind: &'static str, target: Option<String>| {
            let t = target.map_or("null".into(), |t| json(&t));
            grant_json.push(format!(r#"{{"name":{},"kind":"{kind}","target":{t}}}"#, json(name)));
        };
        for (name, _) in dirs {
            let path = dir_paths.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone());
            grant(name, "Dir", path);
        }
        for (name, o) in https {
            grant(name, "Http", Some(o.target()));
        }
        for (name, _) in incomings {
            grant(name, "Incoming", None);
        }
        for (name, _) in outgoings {
            grant(name, "Outgoing", None);
        }
        let l = &self.limits;
        let mut hasher = std::hash::DefaultHasher::new();
        files
            .iter()
            .for_each(|(_, src)| std::hash::Hash::hash(src, &mut hasher));
        let header = format!(
            r#"{{"event":"start","program_hash":"{:016x}","grants":[{}],"limits":{{"fuel":{},"timeout_secs":{},"stack":{},"memory":{},"calls":{},"io":{}}}}}"#,
            std::hash::Hasher::finish(&hasher),
            grant_json.join(","),
            l.fuel,
            l.timeout_secs,
            l.stack,
            l.memory,
            l.calls,
            l.io
        );
        let mut audit = Audit { out, seq: 0 };
        audit.line(&header)?;
        self.store.data_mut().audit = Some(audit);
        Ok(())
    }

    /// Write the audit trailer, even when the run stopped.
    fn end_audit(&mut self, result: &Result<std::result::Result<Outcome, Vec<Diag>>>) -> Result<()> {
        let fuel_used = self.limits.fuel.saturating_sub(self.store.get_fuel().unwrap_or(0));
        let d = self.store.data_mut();
        let Some(a) = d.audit.as_mut() else { return Ok(()) };
        let outcome = match result {
            Ok(Ok(o)) if !o.failed => "ok",
            Ok(Ok(_)) => "failed",
            _ => "error",
        };
        let line = format!(
            r#"{{"event":"end","outcome":"{outcome}","calls":{},"io":{},"peak_memory":{},"fuel_used":{fuel_used}}}"#,
            d.calls, d.io, d.peak_memory
        );
        a.line(&line)?;
        Ok(())
    }

    fn run_granted(
        &mut self,
        files: &[(String, String)],
        dirs: Vec<(String, RealDir)>,
        https: Vec<(String, Origin)>,
        incomings: Vec<(String, Resource<kio::caps::web::Incoming>)>,
        outgoings: Vec<(String, Resource<kio::caps::web::Outgoing>)>,
    ) -> Result<std::result::Result<Outcome, Vec<Diag>>> {
        let mut grants = Vec::new();
        for (name, dir) in dirs {
            let h = self.store.data_mut().table.push(Granted(dir))?;
            self.name_handle(h.rep(), &name, "Dir");
            grants.push(DirGrant {
                name,
                dir: Resource::new_own(h.rep()),
            });
        }
        let mut http_grants = Vec::new();
        for (name, origin) in https {
            let h = self.store.data_mut().table.push(origin)?;
            self.name_handle(h.rep(), &name, "Http");
            http_grants.push(HttpGrant {
                name,
                http: Resource::new_own(h.rep()),
            });
        }
        for (name, r) in &incomings {
            self.name_handle(r.rep(), name, "Incoming");
        }
        for (name, r) in &outgoings {
            self.name_handle(r.rep(), name, "Outgoing");
        }
        let incomings: Vec<_> = incomings
            .into_iter()
            .map(|(name, incoming)| IncomingGrant { name, incoming })
            .collect();
        let outgoings: Vec<_> = outgoings
            .into_iter()
            .map(|(name, outgoing)| OutgoingGrant { name, outgoing })
            .collect();
        self.guest.call_run(
            &mut self.store,
            &file_list(files),
            &grants,
            &http_grants,
            &incomings,
            &outgoings,
        )
    }
}

impl Kio {
    /// Record which grant a handle is, for the report check and the log.
    fn name_handle(&mut self, rep: u32, name: &str, kind: &'static str) {
        self.store.data_mut().handles.insert(rep, (name.to_string(), kind));
    }
}

/// A lone root file, for callers with one source.
fn single(source: &str) -> (String, String) {
    ("main".into(), source.into())
}

fn file_list(files: &[(String, String)]) -> Vec<File> {
    files
        .iter()
        .map(|(name, source)| File {
            name: name.clone(),
            source: source.clone(),
        })
        .collect()
}
