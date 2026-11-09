//! Runs compiled kio under Wasmtime. The host checks, lowers and compiles a program (compiled.rs),
//! then runs it in the `compiled` world of caps.wit. `dir` is backed by cap-std; `http` is a ureq
//! client fixed to one granted origin; `incoming` and `outgoing` hold one request and its first
//! response; `kv` is a store shared across runs (kv.rs); `ws-message` and `ws-send` hold one
//! WebSocket event and its sends (ws.rs). Every import is counted and, if asked, written to an
//! audit log.
use cap_std::ambient_authority;
use cap_std::fs::{Dir as RealDir, OpenOptions, OpenOptionsExt};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use wasmtime::component::{Resource, ResourceTable};
use wasmtime::Result;
use wasmtime::{Engine, Store};

mod bindings {
    // Trappable imports let a host function stop the run by returning `Err`.
    wasmtime::component::bindgen!({ world: "compiled", path: "../caps.wit", imports: { default: trappable } });
}
use bindings::kio;

pub mod cache;
mod compiled;
mod config;
mod grants;
#[cfg(feature = "test-hooks")]
mod interp;
pub mod kv;
pub use compiled::{Kio, Server};
pub use config::{MAX_STACK, RUN_STACK};
#[cfg(feature = "test-hooks")]
pub use interp::Interp;
pub mod listen;
pub mod ws;

/// A checker, grant or runtime error: byte span into source file number `file`, message and an
/// optional fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub start: u32,
    pub end: u32,
    pub message: String,
    pub hint: Option<String>,
    pub file: u32,
}

impl From<lang::Diag> for Diag {
    fn from(d: lang::Diag) -> Self {
        Diag {
            start: d.span.start as u32,
            end: d.span.end as u32,
            message: d.msg,
            hint: d.hint,
            file: d.span.file,
        }
    }
}

/// The authority report, one line per function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<String>,
}

/// What a run produced: the report, main's value printed, and whether main returned `Err`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub report: Report,
    pub value: String,
    pub failed: bool,
}

/// A run's outcome, or the checker's, the grants' or a runtime error.
pub type RunResult = std::result::Result<Outcome, Vec<Diag>>;

/// A run stopped by one of its limits; the message names the limit and its flag.
#[derive(Debug)]
pub struct Stopped(pub String);

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The message of a run stopped by a limit, or `None` for any other error.
pub fn explain(e: &wasmtime::Error) -> Option<String> {
    e.downcast_ref::<Stopped>().map(|s| s.0.clone())
}

/// Check `files` (root first) in the host: the report, or the diagnostics.
pub fn check_files(files: &[(String, String)]) -> std::result::Result<Report, Vec<Diag>> {
    let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
    match lang::check_files(&pairs, &lang::CapTable::v0()) {
        Ok(p) => Ok(Report {
            lines: lang::report(&p).lines().map(String::from).collect(),
        }),
        Err(diags) => Err(diags.into_iter().map(Diag::from).collect()),
    }
}

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

/// Most detached host-call workers alive at once.
const MAX_WORKERS: usize = 64;
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
    /// Set by a write to a read-only `Kv` store.
    read_only_write: bool,
    /// `(capability parameter, method)` pairs on `main`'s report line; any other call is a checker
    /// bug. Set at the start of every run, whether or not a log is written.
    allowed: HashSet<(String, String)>,
    /// Grant name and kind of each handle.
    handles: HashMap<u32, (String, &'static str)>,
    audit_msg: String,
    /// Compiled code: the third value of the next runtime error's message (compiled.rs).
    trap_c: i64,
    table: ResourceTable,
}

impl State {
    /// State for a run under `limits`, its deadline counted from now. Nothing but caps.wit is
    /// linked: compiled code has no clock, randomness or environment.
    fn limited(limits: &Limits) -> Self {
        // A timeout too large to represent caps at a year rather than panicking.
        let now = Instant::now();
        let year = Duration::from_secs(365 * 24 * 3600);
        let wait = Duration::from_secs(limits.timeout_secs).min(year);
        State {
            calls: 0,
            peak_memory: 0,
            memory_limit: limits.memory,
            out_of_memory: false,
            call_limit: limits.calls,
            too_many_calls: false,
            io: 0,
            io_limit: limits.io,
            too_much_io: false,
            deadline: now.checked_add(wait).unwrap_or(now),
            timed_out: false,
            #[cfg(feature = "test-hooks")]
            fail_writes: false,
            audit: None,
            unexpected: false,
            read_only_write: false,
            allowed: HashSet::new(),
            handles: HashMap::new(),
            audit_msg: String::new(),
            trap_c: 0,
            table: ResourceTable::new(),
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

/// Open `path` (already confined by cap-std) and refuse anything but a regular file. `O_NONBLOCK`
/// keeps opening a FIFO from waiting for a peer; the check then refuses it.
fn open_regular(dir: &RealDir, path: &str, opts: &mut OpenOptions) -> io::Result<cap_std::fs::File> {
    let file = dir.open_with(path, opts.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32))?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

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

/// The most names one `Dir.list` reads; a larger directory is `too-large`.
const LIST_CAP: usize = 10_000;

/// What `path` names, not following a final symlink; `None` if missing. A symlink that leads out
/// of the root is `invalid-path`, as for `read`.
fn kind(dir: &RealDir, path: &str) -> Result<Option<cap_std::fs::FileType>, IoErr> {
    match dir.symlink_metadata(path) {
        Ok(m) if m.is_symlink() => match dir.metadata(path) {
            Err(e) if matches!(io_err(&e), IoErr::InvalidPath) => Err(IoErr::InvalidPath),
            _ => Ok(Some(m.file_type())),
        },
        Ok(m) => Ok(Some(m.file_type())),
        Err(e) => match io_err(&e) {
            IoErr::NotFound => Ok(None),
            e => Err(e),
        },
    }
}

/// A name for `write`'s temporary file that a program cannot predict: a counter and the time,
/// hashed with std's per-process random keys. Created with `create_new`, so it never replaces
/// a file.
fn temp_name() -> String {
    use std::hash::BuildHasher;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seed = (NEXT.fetch_add(1, Relaxed), std::time::SystemTime::now());
    let h = std::collections::hash_map::RandomState::new().hash_one(seed);
    format!(".kio-{h:016x}.tmp")
}

/// The audit log (H8): one JSON line per host call. Sizes only, never contents. Under `--listen`
/// every request writes to one shared log, and `tag` (`"req":3,"conn":1,`) names the request and
/// its WebSocket connection at the start of each of its lines.
struct Audit {
    out: SharedLog,
    seq: u64,
    tag: String,
}

/// An audit log that runs share.
type SharedLog = Arc<Mutex<Box<dyn Write + Send>>>;

impl Audit {
    /// Write one line (a JSON object) with the tag and flush it. `State::call` does this after the
    /// work, so a crash of the host process inside a host call can lose that call's line.
    fn line(&mut self, line: &str) -> io::Result<()> {
        let tagged = format!("{{{}{}", self.tag, line.strip_prefix('{').unwrap_or(line));
        write_line(&self.out, &tagged)
    }
}

/// Write `line` and a newline to `out` in one write under its lock, then flush: lines from
/// concurrent requests never interleave.
fn write_line(out: &SharedLog, line: &str) -> io::Result<()> {
    let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
    out.write_all(format!("{line}\n").as_bytes())?;
    out.flush()
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
            Err(_) if self.read_only_write => "stopped:read-only".into(),
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
        // Timed-out workers linger; past MAX_WORKERS live ones, refuse as a timeout.
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        let timeout = |s: &mut Self| {
            s.timed_out = true;
            wasmtime::Error::msg("host call timed out")
        };
        if LIVE.fetch_add(1, Relaxed) >= MAX_WORKERS {
            LIVE.fetch_sub(1, Relaxed);
            return Err(timeout(self));
        }
        // Releases the slot when the worker ends, even if `work` panics.
        struct Slot;
        impl Drop for Slot {
            fn drop(&mut self) {
                LIVE.fetch_sub(1, Relaxed);
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _slot = Slot;
            let _ = tx.send(work());
        });
        rx.recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| match e {
                RecvTimeoutError::Timeout => timeout(self),
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

    fn list(&mut self, d: Resource<kio::caps::fs::Dir>, path: String) -> Result<Result<Vec<String>, IoErr>> {
        let arg = path.clone();
        self.call(d.rep(), "list", &arg, |s| s.list_dir(&d, path), outcome)
    }

    fn write(&mut self, d: Resource<kio::caps::fs::Dir>, path: String, data: Vec<u8>) -> Result<Result<(), IoErr>> {
        let arg = path.clone();
        self.call(d.rep(), "write", &arg, |s| s.write_file(&d, path, data), outcome)
    }

    fn remove(&mut self, d: Resource<kio::caps::fs::Dir>, path: String) -> Result<Result<(), IoErr>> {
        let arg = path.clone();
        let work = |dir: &RealDir, path: &str| match kind(dir, path)? {
            _ if path.ends_with('/') => Err(IoErr::Denied),
            None => Err(IoErr::NotFound),
            Some(t) if t.is_file() => dir.remove_file(path).map_err(|e| io_err(&e)),
            Some(_) => Err(IoErr::Denied),
        };
        self.call(d.rep(), "remove", &arg, |s| s.on_dir(&d, path, work), outcome)
    }

    fn exists(&mut self, d: Resource<kio::caps::fs::Dir>, path: String) -> Result<Result<bool, IoErr>> {
        let arg = path.clone();
        // Follows a symlink inside the root, as `read` does.
        let work = |dir: &RealDir, path: &str| match dir.metadata(path) {
            Ok(m) => Ok(m.is_file() || m.is_dir()),
            Err(e) => match io_err(&e) {
                IoErr::NotFound => Ok(false),
                e => Err(e),
            },
        };
        self.call(d.rep(), "exists", &arg, |s| s.on_dir(&d, path, work), outcome)
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
            // Test only: stands in for a read that blocks, since a FIFO is now refused at once.
            #[cfg(feature = "test-hooks")]
            if path == "pipe" {
                std::thread::sleep(Duration::from_secs(10));
            }
            let mut data = Vec::new();
            let read = open_regular(&dir, &path, OpenOptions::new().read(true))
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

    /// Validate `path`, then run `work` on the granted directory under the run's deadline.
    fn on_dir<T: Send + 'static>(
        &mut self,
        d: &Resource<kio::caps::fs::Dir>,
        path: String,
        work: impl FnOnce(&RealDir, &str) -> Result<T, IoErr> + Send + 'static,
    ) -> Result<Result<T, IoErr>> {
        if let Err(e) = check_path(&path) {
            return Ok(Err(e));
        }
        let Some(dir) = self.dir(d) else {
            return Ok(Err(IoErr::Denied));
        };
        self.within(move || work(&dir, &path))
    }

    fn list_dir(&mut self, d: &Resource<kio::caps::fs::Dir>, path: String) -> Result<Result<Vec<String>, IoErr>> {
        let path = if path.is_empty() { ".".into() } else { path };
        let names = self.on_dir(d, path, |dir, path| {
            let sub = match dir.open_dir(path) {
                Ok(s) => s,
                Err(e) if e.kind() == io::ErrorKind::NotADirectory && dir.metadata(path).is_ok() => {
                    return Err(IoErr::Denied)
                }
                Err(e) => return Err(io_err(&e)),
            };
            let mut names = Vec::new();
            // Every entry counts toward the cap, kept or not, so the host's work is bounded too.
            for (i, entry) in sub.entries().map_err(|e| io_err(&e))?.enumerate() {
                if i == LIST_CAP {
                    return Err(IoErr::TooLarge);
                }
                let entry = entry.map_err(|e| io_err(&e))?;
                let mut t = entry.file_type().map_err(|e| io_err(&e))?;
                // Some filesystems (XFS without ftype, NFS, FUSE) report an unknown type: ask.
                if !t.is_dir() && !t.is_file() && !t.is_symlink() {
                    t = entry.metadata().map_err(|e| io_err(&e))?.file_type();
                }
                // A name that is not UTF-8 cannot cross as a WIT string; it is left out.
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if t.is_dir() {
                    names.push(name + "/");
                } else if t.is_file() {
                    names.push(name);
                }
            }
            names.sort();
            Ok(names)
        })?;
        match &names {
            Ok(names) => self.spend_io(names.iter().map(String::len).sum())?,
            // The host still read the entries: charge a byte each, so repeating it is bounded.
            Err(IoErr::TooLarge) => self.spend_io(LIST_CAP)?,
            Err(_) => {}
        }
        Ok(names)
    }

    /// Write a temp file beside `path`, then rename it over `path`, so a reader sees the old or
    /// the new file. The rename replaces whatever is at `path` then: if a directory took its place
    /// since the check, the rename fails; if a symlink did, the link itself is replaced. Either
    /// way nothing outside the root is touched.
    fn write_file(
        &mut self,
        d: &Resource<kio::caps::fs::Dir>,
        path: String,
        data: Vec<u8>,
    ) -> Result<Result<(), IoErr>> {
        self.spend_io(data.len())?;
        #[cfg(feature = "test-hooks")]
        let fail_write = self.fail_writes;
        self.on_dir(d, path, move |dir, path| {
            // `a/` can only name a directory; `kind` would report a symlink-to-directory as missing
            // (`ENOTDIR` on macOS) and the rename would then replace the symlink itself.
            if path.ends_with('/') || kind(dir, path)?.is_some_and(|t| !t.is_file()) {
                return Err(IoErr::Denied);
            }
            let temp = match path.rsplit_once('/') {
                Some((parent, _)) => format!("{parent}/{}", temp_name()),
                None => temp_name(),
            };
            let mut file =
                open_regular(dir, &temp, OpenOptions::new().write(true).create_new(true)).map_err(|e| io_err(&e))?;
            let written = file.write_all(&data);
            #[cfg(feature = "test-hooks")]
            let written = if fail_write {
                Err(io::Error::other("injected"))
            } else {
                written
            };
            drop(file);
            let done = written.and_then(|()| dir.rename(&temp, dir, path));
            if let Err(e) = done {
                let _ = dir.remove_file(&temp);
                return Err(io_err(&e));
            }
            Ok(())
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
            let mut file = match open_regular(&dir, &path, &mut opts) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(IoErr::Exists),
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

/// The `Incoming` and `Outgoing` grants for one request, named `names`, and the slot the
/// `Outgoing` fills.
fn web_grants(s: &mut State, request: Request, names: (&str, &str)) -> Result<(Sent, Handles)> {
    let sent = Sent::default();
    let req = s.table.push(request)?;
    let res = s.table.push(sent.clone())?;
    let handles = Handles {
        incomings: vec![(names.0.to_string(), Resource::new_own(req.rep()))],
        outgoings: vec![(names.1.to_string(), Resource::new_own(res.rep()))],
        ..Handles::default()
    };
    Ok((sent, handles))
}

/// The `WsMessage` and `WsSend` grants for one event, named `names`, and the outbox the
/// sends fill. `closed` makes every send `closed`.
fn ws_grants(
    s: &mut State,
    event: ws::WsEvent,
    closed: bool,
    names: &(String, String),
) -> Result<(ws::Sends, Handles)> {
    let sends = ws::Sends::default();
    sends.lock().unwrap_or_else(PoisonError::into_inner).closed = closed;
    let msg = s.table.push(event)?;
    let conn = s.table.push(sends.clone())?;
    let handles = Handles {
        messages: vec![(names.0.clone(), Resource::new_own(msg.rep()))],
        senders: vec![(names.1.clone(), Resource::new_own(conn.rep()))],
        ..Handles::default()
    };
    Ok((sends, handles))
}

/// One request's or event's grants, already in the store's table, by `main` parameter name.
#[derive(Default)]
struct Handles {
    incomings: Vec<(String, Resource<kio::caps::web::Incoming>)>,
    outgoings: Vec<(String, Resource<kio::caps::web::Outgoing>)>,
    messages: Vec<(String, Resource<kio::caps::ws::WsMessage>)>,
    senders: Vec<(String, Resource<kio::caps::ws::WsSend>)>,
}

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
        let valid = (200..=599).contains(&status)
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

/// Per-run limits. `stack` is bytes of Wasm stack, 2 MB by default: about 43,000 calls of a small
/// function. Fuel is Wasm instructions: compiled code spends 10 to 60 times less than the
/// interpreter did per program, so the defaults are a tenth of the interpreter's (C5), which keeps
/// at least the headroom typical programs had.
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
            fuel: 10_000_000_000,
            timeout_secs: 10,
            stack: 2 << 20,
            memory: 256 << 20,
            calls: 10_000,
            io: 100 << 20,
        }
    }
}

impl Limits {
    /// Per-request limits for serving (web decision 9): 10 million fuel, 64 MB, 5 s.
    pub fn web() -> Self {
        Limits {
            fuel: 10_000_000,
            timeout_secs: 5,
            memory: 64 << 20,
            ..Limits::default()
        }
    }
}

/// `(name, path)` of each `Dir` grant, for the audit header.
pub type DirPaths = Vec<(String, String)>;

/// Starts the thread that advances `engine`'s epoch every 100 ms, until the returned flag is set.
/// Wasmtime checks epochs only in guest code: host calls are bounded separately, by `State::within`.
fn spawn_ticker(engine: &Engine) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let (e, s) = (engine.clone(), stop.clone());
    std::thread::spawn(move || {
        while !s.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            e.increment_epoch();
        }
    });
    stop
}

/// The run's stack limit, at most `MAX_STACK`. Compiled code runs on the calling thread's stack,
/// which must be `RUN_STACK`.
fn set_stack(config: &mut wasmtime::Config, bytes: usize) {
    let bytes = bytes.min(config::MAX_STACK);
    config.max_wasm_stack(bytes);
    // Nothing runs async (the feature is off), but Wasmtime still refuses a `max_wasm_stack` above
    // `async_stack_size`, 2 MB by default; nothing is reserved for it.
    config.async_stack_size(bytes.max(2 << 20));
}

/// Run `work` on a thread with `RUN_STACK` of native stack, whatever the caller's.
fn on_run_stack<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        let t = std::thread::Builder::new().stack_size(config::RUN_STACK);
        match t.spawn_scoped(s, work).expect("cannot start a thread").join() {
            Ok(v) => v,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

/// SHA-256 of the program as 64 hex digits; lengths keep file boundaries in the hash.
pub fn program_hash(files: &[(String, String)]) -> String {
    use sha2::{Digest, Sha256};
    let mut sha = Sha256::new();
    for text in files.iter().flat_map(|(name, source)| [name, source]) {
        sha.update((text.len() as u64).to_le_bytes());
        sha.update(text);
    }
    sha.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A store holding `limits`' fuel, memory, call and I/O quotas and its deadline, counted from now.
fn new_store(engine: &Engine, limits: &Limits) -> Result<Store<State>> {
    let mut store = Store::new(engine, State::limited(limits));
    store.limiter(|s| s);
    store.set_fuel(limits.fuel)?;
    store.set_epoch_deadline(limits.timeout_secs.saturating_mul(10));
    store.epoch_deadline_trap();
    Ok(store)
}

/// The message for a Wasm trap that is one of `l`'s limits.
fn limit_trap(e: &wasmtime::Error, l: &Limits) -> Option<String> {
    Some(match e.downcast_ref::<wasmtime::Trap>()? {
        wasmtime::Trap::OutOfFuel => format!("out of fuel (limit {}; raise it with --fuel)", l.fuel),
        wasmtime::Trap::Interrupt => format!("timed out after {} s (raise it with --timeout)", l.timeout_secs),
        wasmtime::Trap::StackOverflow => {
            format!("stack overflow (limit {} KB; raise it with --stack)", l.stack >> 10)
        }
        _ => return None,
    })
}

/// The message for a run in `s` that hit one of `l`'s limits, or `None` for any other error.
fn limit_message(s: &State, l: &Limits, e: &wasmtime::Error) -> Option<String> {
    state_limit(s, l).or_else(|| limit_trap(e, l))
}

/// The message for a limit `s` recorded: memory, a quota, the report check or the deadline.
fn state_limit(s: &State, l: &Limits) -> Option<String> {
    // A refused memory grow makes the runtime's allocator trap, so the trap itself is generic.
    if s.out_of_memory {
        return Some(format!(
            "out of memory (limit {} MB; raise it with --memory)",
            l.memory >> 20
        ));
    }
    if s.unexpected || s.read_only_write {
        return Some(s.audit_msg.clone());
    }
    if s.too_many_calls {
        return Some(format!(
            "too many host calls (limit {}; raise it with --calls)",
            l.calls
        ));
    }
    if s.too_much_io {
        return Some(format!("too much I/O (limit {} MB; raise it with --io)", l.io >> 20));
    }
    if s.timed_out {
        return Some(format!(
            "timed out after {} s (raise it with --timeout)",
            l.timeout_secs
        ));
    }
    None
}

/// Named `Kv` stores. A store outlives runs, so each run gets a handle to the same one.
pub type KvGrants = Vec<(String, Arc<kv::Store>)>;

impl Handles {
    /// `(name, kind, rep)` of each handle.
    fn each(&self) -> Vec<(String, &'static str, u32)> {
        let mut out = Vec::new();
        out.extend(self.incomings.iter().map(|(n, r)| (n.clone(), "Incoming", r.rep())));
        out.extend(self.outgoings.iter().map(|(n, r)| (n.clone(), "Outgoing", r.rep())));
        out.extend(self.messages.iter().map(|(n, r)| (n.clone(), "WsMessage", r.rep())));
        out.extend(self.senders.iter().map(|(n, r)| (n.clone(), "WsSend", r.rep())));
        out
    }
}

/// A grant in the store's table: the `main` parameter it is for, its kind (`Dir`), its handle,
/// and for the audit header what it reaches (a `Dir`'s path comes from `DirPaths`).
struct Bound {
    name: String,
    kind: &'static str,
    rep: u32,
    target: Option<String>,
}

/// `(name, kind)` of each grant, for `grants::bind`.
fn named(bound: &[Bound]) -> Vec<(String, &'static str)> {
    bound.iter().map(|b| (b.name.clone(), b.kind)).collect()
}

/// Put one run's grants in `store`'s table (`handles` are there already) and record which grant
/// each handle is, for the report check and the log. In the order the audit header lists them.
fn push_grants(
    s: &mut State,
    dirs: Vec<(String, RealDir)>,
    https: Vec<(String, Origin)>,
    handles: Handles,
    kvs: KvGrants,
) -> Result<Vec<Bound>> {
    let mut bound = Vec::new();
    let mut add = |name: String, kind, rep, target| {
        bound.push(Bound {
            name,
            kind,
            rep,
            target,
        })
    };
    for (name, dir) in dirs {
        add(name, "Dir", s.table.push(Granted(dir))?.rep(), None);
    }
    for (name, origin) in https {
        let target = Some(origin.target());
        add(name, "Http", s.table.push(origin)?.rep(), target);
    }
    for (name, kind, rep) in handles.each() {
        add(name, kind, rep, None);
    }
    for (name, store) in kvs {
        let target = Some(store.label());
        add(name, "Kv", s.table.push(store)?.rep(), target);
    }
    for b in &bound {
        s.handles.insert(b.rep, (b.name.clone(), b.kind));
    }
    Ok(bound)
}

/// Write the audit header (H8) for a run of `files` with `bound` grants to `out`, and arm the
/// per-call log.
fn start_audit(
    s: &mut State,
    out: Box<dyn Write + Send>,
    dir_paths: &DirPaths,
    l: &Limits,
    files: &[(String, String)],
    bound: &[Bound],
) -> Result<()> {
    let grants: Vec<_> = bound
        .iter()
        .map(|b| {
            let target = match b.kind {
                "Dir" => dir_paths.iter().find(|(n, _)| *n == b.name).map(|(_, p)| p.clone()),
                _ => b.target.clone(),
            };
            (b.name.clone(), b.kind, target)
        })
        .collect();
    let out: SharedLog = Arc::new(Mutex::new(out));
    write_line(&out, &audit_header(files, l, &grants))?;
    s.audit = Some(Audit {
        out,
        seq: 0,
        tag: String::new(),
    });
    Ok(())
}

/// The audit header: the program's hash, each `(name, kind, target)` grant and the limits.
fn audit_header(files: &[(String, String)], l: &Limits, grants: &[(String, &str, Option<String>)]) -> String {
    let grant_json: Vec<String> = grants
        .iter()
        .map(|(name, kind, target)| {
            let t = target.as_ref().map_or("null".into(), |t| json(t));
            format!(r#"{{"name":{},"kind":"{kind}","target":{t}}}"#, json(name))
        })
        .collect();
    format!(
        r#"{{"event":"start","program_hash":"{}","grants":[{}],"limits":{{"fuel":{},"timeout_secs":{},"stack":{},"memory":{},"calls":{},"io":{}}}}}"#,
        program_hash(files),
        grant_json.join(","),
        l.fuel,
        l.timeout_secs,
        l.stack,
        l.memory,
        l.calls,
        l.io
    )
}

/// Write the audit trailer, even when the run stopped. `outcome` is `ok`, `failed` or `error`.
fn end_audit(d: &mut State, fuel_used: u64, outcome: &str) -> Result<()> {
    let Some(a) = d.audit.as_mut() else { return Ok(()) };
    let line = format!(
        r#"{{"event":"end","outcome":"{outcome}","calls":{},"io":{},"peak_memory":{},"fuel_used":{fuel_used}}}"#,
        d.calls, d.io, d.peak_memory
    );
    a.line(&line)?;
    Ok(())
}

/// A lone root file, for callers with one source.
fn single(source: &str) -> (String, String) {
    ("main".into(), source.into())
}

impl Response {
    fn internal_error() -> Self {
        Response {
            status: 500,
            headers: vec![],
            body: vec![],
        }
    }
}
