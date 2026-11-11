//! Runs compiled kio under Wasmtime. The host checks, lowers and compiles a program (compiled.rs),
//! then runs it in the `compiled` world of caps.wit. `dir` is backed by cap-std; `http` is a ureq
//! client fixed to one granted origin; `incoming` and `outgoing` hold one request and its first
//! response; `kv` is a store shared across runs (kv.rs); `ws-message` and `ws-send` hold one
//! WebSocket event and its sends (ws.rs); `rng`, `clock` and `log` are the OS's randomness (or a
//! seeded stream), the system clock (or a fixed time) and an appended file (or stderr); `db` is
//! a fenced SQLite connection shared across runs (db.rs); `hmac` signs and verifies under a key
//! the host read at start. Every import is counted and, if asked,
//! written to an audit log.
use cap_std::ambient_authority;
use cap_std::fs::{Dir as RealDir, OpenOptions, OpenOptionsExt};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
pub mod db;
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

/// The authority `new` has over `old` and the reverse (see `lang::report_diff`), or the
/// diagnostics of the first side that fails to check: `Err((true, ..))` for `old`.
pub fn diff_files(
    old: &[(String, String)],
    new: &[(String, String)],
) -> std::result::Result<lang::AuthorityDiff, (bool, Vec<Diag>)> {
    let check = |files: &[(String, String)]| {
        let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        lang::check_files(&pairs, &lang::CapTable::v0()).map_err(|ds| ds.into_iter().map(Diag::from).collect())
    };
    let old = check(old).map_err(|d| (true, d))?;
    let new = check(new).map_err(|d| (false, d))?;
    Ok(lang::report_diff(&old, &new))
}

/// The most one `Dir.read` returns; a larger file is `too-large`.
const READ_CAP: u64 = 10 << 20;

/// A granted directory in the resource table. Every guest path goes through it.
struct Granted(RealDir);

/// The one scheme, host and port an `Http` grant reaches. Guest input never changes it. It also
/// holds the run's pinned DNS answer; clones share it, and `for_run` gives a run its own.
#[derive(Clone, Debug)]
pub struct Origin {
    https: bool,
    host: String,
    port: u16,
    /// The host's addresses from its first resolution; every later request and hop reuses them.
    pinned: Arc<Mutex<Option<Vec<SocketAddr>>>>,
    /// Real lookups made, by every run of this grant (at most one per run).
    lookups: Arc<AtomicU32>,
    /// Response body bytes read through this grant, cut-off reads included.
    read: Arc<AtomicU64>,
    /// Non-public addresses are allowed: the grant is an IP literal.
    any_addr: bool,
    /// The methods `--http-methods` added; GET is always allowed.
    methods: Vec<&'static str>,
    /// `--http-header`: headers (names lowercased) the host adds to every request and hop.
    inject: Vec<(String, Secret)>,
}

/// A header value from `--http-header` or an `--hmac` key. It is only ever written to a request
/// or keyed into an HMAC: it has no `Display`, and its `Debug` prints `<redacted>`, so no message
/// or log can carry it.
#[derive(Clone)]
pub struct Secret(Arc<str>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The request methods a program may name; any other is `invalid`.
const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

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
            read: Arc::default(),
            any_addr,
            methods: Vec::new(),
            inject: Vec::new(),
        })
    }

    /// This grant for one run: the same origin and methods with nothing resolved yet, so no run
    /// connects to an address another run's lookup returned (each `--listen` request too).
    fn for_run(&self) -> Origin {
        Origin {
            pinned: Arc::default(),
            ..self.clone()
        }
    }

    /// Allow the methods in `list` (`POST,PUT`, any case) besides GET: `--http-methods`.
    pub fn allow_methods(&mut self, list: &str) -> std::result::Result<(), String> {
        for m in list.split(',') {
            let m = m.trim().to_ascii_uppercase();
            let Some(known) = METHODS.iter().find(|k| **k == m) else {
                return Err(format!(
                    "unknown method {m:?}; expected a list of {}",
                    METHODS.join(", ")
                ));
            };
            if *known != "GET" && !self.methods.contains(known) {
                self.methods.push(known);
            }
        }
        Ok(())
    }

    /// Add `header: value` to every request on this grant (`--http-header`). Refused: a name
    /// that is not a token, is reserved or is already injected; an empty value or one with a
    /// control character; an `http://` origin that is not loopback. No error names the value.
    pub fn inject(&mut self, header: &str, value: String) -> std::result::Result<(), String> {
        let name = header.to_ascii_lowercase();
        let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
        if name.is_empty() || !name.chars().all(token) {
            return Err(format!(
                "{header:?} is not a header name\n  hint: a name is letters, digits and !#$%&'*+-.^_`|~, as `Authorization`"
            ));
        }
        if RESERVED_HEADERS.contains(&name.as_str()) || name.starts_with("proxy-") {
            return Err(format!(
                "`{header}` is set by the host\n  hint: inject an application header, as `Authorization`"
            ));
        }
        if self.injects(&name) {
            return Err(format!(
                "`{header}` is injected twice on this grant\n  hint: give each header once"
            ));
        }
        if value.is_empty() {
            return Err("the value is empty\n  hint: set it to the secret".into());
        }
        let control = |c: char| (c.is_ascii_control() && c != '\t') || c == '\u{7f}';
        if value.len() > HEADER_MAX || value.chars().any(control) {
            return Err(format!(
                "the value has a control character (CR, LF, NUL..) or is over {HEADER_MAX} bytes\n  hint: store the value alone, on one line"
            ));
        }
        let host = self.host.trim_matches(['[', ']']);
        // Only a loopback IP literal: a name like `localhost` connects only if it resolves to a
        // public address (`Pinned`), which would carry the header in clear.
        if !self.https && !host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
            return Err(format!(
                "`http://{}:{}` would carry the header in clear\n  hint: grant `https://{}`, or `http://127.0.0.1:PORT` for a local server",
                self.host, self.port, self.host
            ));
        }
        self.inject.push((name, Secret(value.into())));
        Ok(())
    }

    /// Whether the host injects the header `name` (lowercase) on this grant.
    fn injects(&self, name: &str) -> bool {
        self.inject.iter().any(|(n, _)| n == name)
    }

    /// The audit field naming the injected headers, never their values; empty without any.
    fn injected(&self) -> String {
        if self.inject.is_empty() {
            return String::new();
        }
        let names: Vec<String> = self.inject.iter().map(|(n, _)| json(n)).collect();
        format!(r#","injected":[{}]"#, names.join(","))
    }

    /// Whether the grant allows `method`, one of `METHODS`.
    fn allows(&self, method: &str) -> bool {
        method == "GET" || self.methods.contains(&method)
    }

    /// `scheme://host:port`, with the methods added to GET and the injected header names if
    /// any, for the audit log.
    fn target(&self) -> String {
        let methods = match self.methods.is_empty() {
            true => String::new(),
            false => format!(" methods=GET,{}", self.methods.join(",")),
        };
        let inject: Vec<&str> = self.inject.iter().map(|(n, _)| n.as_str()).collect();
        let inject = match inject.is_empty() {
            true => String::new(),
            false => format!(" injects={}", inject.join(",")),
        };
        format!(
            "{}://{}:{}{methods}{inject}",
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

    /// Response body bytes read through this grant so far, by every worker.
    #[cfg(feature = "test-hooks")]
    pub fn body_bytes_read(&self) -> u64 {
        self.read.load(Relaxed)
    }

    /// Resolve `location` against `current`; `None` unless the result is exactly this origin
    /// (scheme, host and port, no credentials), so a redirect can never leave the grant.
    fn follow(&self, current: &str, location: &str) -> Option<String> {
        let mut next = url::Url::parse(current).ok()?.join(location).ok()?;
        let same = next.scheme() == if self.https { "https" } else { "http" }
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

/// Most detached workers for file, store and database calls alive at once, process-wide (only
/// calls the deadline cut off linger). Past it such a call stops its run as a timeout until a
/// worker ends. HTTP workers have their own pool, so requests never fill this one.
pub const MAX_WORKERS: usize = 256;
/// Most HTTP workers alive at once, process-wide: 16 runs each with a full `send_all`. Past it a
/// new `get`, `send` or batch is `timeout` at once, with nothing sent (audited `"busy":true`).
/// A server raises it to a full batch per worker (`size_http_pool`).
pub const MAX_HTTP_WORKERS: usize = 1024;

/// Let `workers` runs each hold a full `send_all` at once: under `--listen` a live run never
/// sees `busy`; the worker pool (a 503) is the bound, and only workers a deadline left behind
/// could reach the ceiling (#164).
pub fn size_http_pool(workers: usize) {
    let fit = workers.saturating_mul(BATCH_MAX).max(MAX_HTTP_WORKERS);
    HTTP_POOL.max.store(fit, Relaxed);
}

/// A process-wide count of live workers and its ceiling.
struct Pool {
    live: AtomicUsize,
    max: AtomicUsize,
}

static HOST_POOL: Pool = Pool {
    live: AtomicUsize::new(0),
    max: AtomicUsize::new(MAX_WORKERS),
};
static HTTP_POOL: Pool = Pool {
    live: AtomicUsize::new(0),
    max: AtomicUsize::new(MAX_HTTP_WORKERS),
};

/// One worker's place in a pool, given back when the worker ends, even if it panics.
struct Slot(&'static Pool);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Relaxed);
    }
}

impl Pool {
    /// `n` places at once, or none if they would take the pool past its ceiling.
    fn take(&'static self, n: usize) -> Option<Vec<Slot>> {
        let had = self.live.fetch_add(n, Relaxed);
        if had + n > self.max.load(Relaxed) {
            self.live.fetch_sub(n, Relaxed);
            return None;
        }
        Some((0..n).map(|_| Slot(self)).collect())
    }
}

/// Lower the HTTP worker ceiling, so a test can reach it.
#[cfg(feature = "test-hooks")]
pub fn set_http_worker_ceiling(n: usize) {
    HTTP_POOL.max.store(n, Relaxed);
}

/// HTTP workers alive now, process-wide.
#[cfg(feature = "test-hooks")]
pub fn http_workers() -> usize {
    HTTP_POOL.live.load(Relaxed)
}

/// File, store and database workers alive now, process-wide.
#[cfg(feature = "test-hooks")]
pub fn host_workers() -> usize {
    HOST_POOL.live.load(Relaxed)
}

/// The workers one run has alive; `State::within_all` keeps it at most `BATCH_MAX`.
#[derive(Default)]
struct RunWorkers {
    live: Mutex<usize>,
    freed: std::sync::Condvar,
}

/// One worker's place in its run, given back when the worker ends.
struct RunSlot(Arc<RunWorkers>);

impl Drop for RunSlot {
    fn drop(&mut self) {
        *self.0.live.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.freed.notify_all();
    }
}

/// The longest wait for one request; the run deadline can cut it shorter.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Redirects followed per request (GET and HEAD only); the next one is `denied` for `get`, and
/// `send` returns it as the response.
const MAX_REDIRECTS: usize = 5;
/// The most a `send` body may hold; more is `too-large` before anything is sent.
const BODY_CAP: usize = 8 << 20;
/// The most headers one request may carry, and the longest `name: value`; more is `invalid`.
const HEADERS_MAX: usize = 64;
const HEADER_MAX: usize = 8 << 10;
/// The most requests in one `send_all`; more is `invalid` for every one, and nothing is sent.
const BATCH_MAX: usize = 64;
/// Response bytes a worker reads at a time, each chunk taken from the run's I/O budget.
const CHUNK: usize = 64 << 10;
/// Headers the host sets or that frame the connection: a program naming one is `invalid`, with
/// any `proxy-*`.
const RESERVED_HEADERS: [&str; 8] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "trailer",
    "keep-alive",
];

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
        if pinned.is_none() {
            o.lookups.fetch_add(1, Relaxed);
            let found = ureq::unversioned::resolver::DefaultResolver::default().resolve(uri, config, timeout);
            let kept = (found.iter().flat_map(|f| f.iter()))
                .copied()
                .filter(|a| o.any_addr || is_public(a.ip()))
                .collect();
            // An all-refused answer is pinned too, and a failed lookup pins no address (its first
            // request gets the error): a run looks the name up once whatever happens.
            *pinned = Some(kept);
            found?;
        }
        let addrs = pinned.as_deref().unwrap_or_default();
        if addrs.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        let mut out = self.empty();
        addrs.iter().for_each(|a| out.push(*a));
        Ok(out)
    }
}

/// A request the host checked: the first URL, a method the grant allows, headers that passed
/// `parse_headers`, the body, and how long the whole request (every hop) may take.
struct Outbound {
    url: String,
    method: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    timeout: Duration,
    /// `get`'s rules: a non-2xx status is `Status`, a redirect not followed is `denied`, and the
    /// body is read only on 2xx. `send`'s: every status is a response, with its body.
    as_get: bool,
}

/// What a request got back.
struct Inbound {
    status: u16,
    /// `name: value`, names lowercased, in order.
    headers: Vec<String>,
    body: Vec<u8>,
}

/// A request done: the redirects followed (each a counted call), the body bytes read (charged
/// even when refused), the time it took, and the response or the error.
struct Performed {
    hops: usize,
    read: usize,
    ms: f64,
    /// Not sent: the process had `MAX_HTTP_WORKERS` HTTP workers alive.
    busy: bool,
    /// Timed out at the run's deadline, not at the request's own timeout.
    cut: bool,
    result: std::result::Result<Inbound, kio::caps::net::HttpErr>,
}

/// `name: value` headers checked: at most `HEADERS_MAX`, each at most `HEADER_MAX` bytes, the
/// name a token (lowercased here) that is not reserved, the value free of control characters
/// (so no CR or LF). `None` if any fails.
fn parse_headers(headers: &[String]) -> Option<Vec<(String, String)>> {
    if headers.len() > HEADERS_MAX {
        return None;
    }
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    let mut out = Vec::with_capacity(headers.len());
    for h in headers {
        let (name, value) = h.split_once(": ").filter(|_| h.len() <= HEADER_MAX)?;
        let name = name.to_ascii_lowercase();
        let reserved = RESERVED_HEADERS.contains(&name.as_str()) || name.starts_with("proxy-");
        if name.is_empty() || !name.chars().all(token) || reserved {
            return None;
        }
        let value = value.trim_matches([' ', '\t']);
        if value
            .chars()
            .any(|c| (c.is_ascii_control() && c != '\t') || c == '\u{7f}')
        {
            return None;
        }
        out.push((name, value.to_string()));
    }
    Some(out)
}

/// Read a response body `CHUNK` bytes at a time into `body` until it ends (`Ok(true)`), or it
/// passes `READ_CAP` or a chunk takes `budget` below zero (`Ok(false)`: too large), or a read
/// fails. `budget` is what the run may still read; a batch's workers share one, so together they
/// hold little more than it however many respond at once.
fn read_body(mut r: impl Read, budget: &AtomicI64, body: &mut Vec<u8>) -> io::Result<bool> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => return Ok(true),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        body.extend_from_slice(&buf[..n]);
        let had = budget.fetch_sub(n as i64, Relaxed);
        if had < n as i64 || body.len() as u64 > READ_CAP {
            return Ok(false);
        }
    }
}

/// Do `req` with no proxy, connecting only to `origin`'s pinned addresses, following a redirect
/// within the origin (GET and HEAD only, `MAX_REDIRECTS` at most). Each hop is bounded by what
/// is left of `req.timeout` and of `deadline`; the body is read through the same cap as
/// `Dir.read` and within `budget` (`read_body`).
fn perform(origin: &Origin, req: &Outbound, deadline: Instant, budget: &AtomicI64) -> Performed {
    use kio::caps::net::HttpErr;
    let start = Instant::now();
    let until = deadline.min(start + req.timeout);
    let timed_out = |e: &io::Error| e.kind() == io::ErrorKind::TimedOut;
    let follows = matches!(req.method, "GET" | "HEAD");
    let mut done = Performed {
        hops: 0,
        read: 0,
        ms: 0.0,
        busy: false,
        cut: false,
        result: Err(HttpErr::Denied),
    };
    let mut url = req.url.clone();
    let result = loop {
        let config = ureq::Agent::config_builder()
            .proxy(None) // ureq's default reads proxy variables from the environment
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(until.saturating_duration_since(Instant::now())))
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::default(),
            Pinned(origin.clone()),
        );
        let mut b = ureq::http::Request::builder().method(req.method).uri(url.as_str());
        for (name, value) in &req.headers {
            b = b.header(name.as_str(), value.as_str());
        }
        // `prepare` refused a program header of the same name, so each is sent once.
        for (name, Secret(value)) in &origin.inject {
            b = b.header(name.as_str(), &**value);
        }
        // A method that carries a body sends it with its length, `Content-Length: 0` when empty
        // (ureq would otherwise send an empty chunked body); any other sends none unless given one.
        let with_body = !req.body.is_empty() || matches!(req.method, "POST" | "PUT" | "PATCH");
        let sent = match with_body {
            true => b
                .body(req.body.as_slice())
                .map_err(ureq::Error::from)
                .and_then(|r| agent.run(r)),
            false => b.body(()).map_err(ureq::Error::from).and_then(|r| agent.run(r)),
        };
        let resp = match sent {
            Ok(r) => r,
            Err(ureq::Error::Timeout(_)) => break Err(HttpErr::Timeout),
            Err(ureq::Error::Io(e)) if timed_out(&e) => break Err(HttpErr::Timeout),
            Err(ureq::Error::Http(_)) => break Err(HttpErr::Invalid),
            Err(_) => break Err(HttpErr::Denied),
        };
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = resp.headers().get("location").and_then(|v| v.to_str().ok());
            match location.and_then(|l| origin.follow(&url, l)) {
                Some(next) if follows && done.hops < MAX_REDIRECTS => {
                    done.hops += 1;
                    url = next;
                    continue;
                }
                // `get` refuses a redirect it cannot follow; `send` hands it to the program.
                _ if req.as_get => break Err(HttpErr::Denied),
                _ => {}
            }
        }
        if req.as_get && !(200..300).contains(&status) {
            break Err(HttpErr::Status(status));
        }
        let headers = (resp.headers().iter())
            .map(|(n, v)| format!("{}: {}", n.as_str(), String::from_utf8_lossy(v.as_bytes())))
            .collect();
        let mut body = Vec::new();
        let read = read_body(resp.into_body().into_reader(), budget, &mut body);
        done.read = body.len();
        origin.read.fetch_add(body.len() as u64, Relaxed);
        break match read {
            Ok(true) => Ok(Inbound { status, headers, body }),
            Ok(false) => Err(HttpErr::TooLarge),
            Err(e) if timed_out(&e) => Err(HttpErr::Timeout),
            Err(_) => Err(HttpErr::Denied),
        };
    };
    done.ms = start.elapsed().as_secs_f64() * 1000.0;
    done.cut = until == deadline && matches!(result, Err(HttpErr::Timeout));
    done.result = result;
    done
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
    /// Set by a write to a read-only `Kv` store or `Db`.
    read_only_write: bool,
    /// `(capability parameter, method)` pairs on `main`'s report line; any other call is a checker
    /// bug. Set at the start of every run, whether or not a log is written.
    allowed: HashSet<(String, String)>,
    /// Grant name and kind of each handle.
    handles: HashMap<u32, (String, &'static str)>,
    audit_msg: String,
    /// More fields of the current call's audit line (`,"n":1`), set by the call; cleared after it.
    audit_extra: String,
    /// `send_all` calls so far: the next batch's id in the audit log.
    batches: u64,
    /// This run's live workers.
    workers: Arc<RunWorkers>,
    /// Compiled code: the third value of the next runtime error's message (compiled.rs).
    trap_c: i64,
    /// `calls` when `Outgoing.send` handed over the response; the trailer counts the calls after.
    sent_at: Option<u32>,
    table: ResourceTable,
}

impl State {
    /// State for a run under `limits`, its deadline counted from now. Nothing but caps.wit is
    /// linked: compiled code has no environment, and reaches a clock or randomness only through
    /// a `Clock` or `Rng` grant.
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
            audit_extra: String::new(),
            batches: 0,
            workers: Arc::default(),
            trap_c: 0,
            sent_at: None,
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
        let extra = std::mem::take(&mut self.audit_extra);
        if self.audit.is_none() {
            return r;
        }
        let why = match &r {
            Ok(v) => result(v),
            Err(_) if self.unexpected => "stopped:unexpected".into(),
            Err(_) if self.read_only_write => "stopped:read-only".into(),
            Err(_) if self.too_many_calls => "stopped:calls".into(),
            Err(_) if self.too_much_io => "stopped:io".into(),
            Err(_) if self.timed_out => "stopped:time".into(),
            Err(_) => "stopped:error".into(),
        };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let written = self.audit_line(handle, method, arg, self.io - io, &why, &extra, ms);
        let r = r?;
        written.map_err(|e| wasmtime::Error::msg(format!("audit: cannot write the log: {e}")))?;
        Ok(r)
    }

    /// One line of the audit log for a call of `method` on `handle`, with the next `seq`: `arg`
    /// cut at 256 characters, `bytes` of I/O, its `result`, `extra` fields (`,"n":1`) and its
    /// duration. Nothing without a log.
    #[allow(clippy::too_many_arguments)]
    fn audit_line(
        &mut self,
        handle: u32,
        method: &str,
        arg: &str,
        bytes: u64,
        result: &str,
        extra: &str,
        ms: f64,
    ) -> io::Result<()> {
        let Some(a) = self.audit.as_mut() else { return Ok(()) };
        let (cap, kind) = self.handles.get(&handle).cloned().unwrap_or(("?".into(), "?"));
        a.seq += 1;
        let line = format!(
            r#"{{"event":"call","seq":{},"cap":{},"kind":"{kind}","method":{},"arg":{},"bytes":{bytes},"result":{}{extra}{},"ms":{ms:.1}}}"#,
            a.seq,
            json(&cap),
            json(method),
            json(&arg.chars().take(256).collect::<String>()),
            json(result),
            if self.unexpected { r#","unexpected":true"# } else { "" },
        );
        a.line(&line)
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
    /// run. The thread finishes in the background and its result is dropped. Past `MAX_WORKERS`
    /// such threads alive in the process, the run stops as a timeout.
    fn within<T: Send + 'static>(&mut self, work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        let Some(slots) = HOST_POOL.take(1) else {
            return Err(self.stop_timed_out());
        };
        let mut done = self.within_all(vec![Box::new(work)], slots)?;
        Ok(done.pop().expect("one worker, one result"))
    }

    /// The run stops: a host call reached the deadline.
    fn stop_timed_out(&mut self) -> wasmtime::Error {
        self.timed_out = true;
        wasmtime::Error::msg("host call timed out")
    }

    /// Run each of `works` on its own worker thread, holding one of `slots` (its place in a
    /// process-wide pool), and wait for all of them, in order, until the run's deadline; past it,
    /// the run stops as `within`'s does. A run has at most `BATCH_MAX` workers alive: past that
    /// this waits for its own to end, until the deadline.
    fn within_all<T: Send + 'static>(
        &mut self,
        works: Vec<Box<dyn FnOnce() -> T + Send>>,
        slots: Vec<Slot>,
    ) -> Result<Vec<T>> {
        let n = works.len();
        assert_eq!(slots.len(), n, "a pool slot per worker");
        let run = self.workers.clone();
        let mut live = run.live.lock().unwrap_or_else(PoisonError::into_inner);
        while *live + n > BATCH_MAX {
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                drop(live);
                return Err(self.stop_timed_out());
            }
            live = run
                .freed
                .wait_timeout(live, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *live += n;
        drop(live);
        let (tx, rx) = std::sync::mpsc::channel();
        for ((i, work), slot) in works.into_iter().enumerate().zip(slots) {
            let (tx, held) = (tx.clone(), RunSlot(run.clone()));
            std::thread::spawn(move || {
                let _slots = (slot, held);
                let _ = tx.send((i, work()));
            });
        }
        let mut done: Vec<Option<T>> = (0..n).map(|_| None).collect();
        for _ in 0..n {
            let (i, v) = rx
                .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| match e {
                    RecvTimeoutError::Timeout => self.stop_timed_out(),
                    RecvTimeoutError::Disconnected => wasmtime::Error::msg("host call failed"),
                })?;
            done[i] = Some(v);
        }
        Ok(done.into_iter().map(|v| v.expect("every worker answered")).collect())
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

    fn send(
        &mut self,
        h: Resource<kio::caps::net::Http>,
        method: String,
        path: String,
        headers: Vec<String>,
        body: Vec<u8>,
    ) -> Result<Result<HttpResponse, kio::caps::net::HttpErr>> {
        let arg = path.clone();
        self.call(
            h.rep(),
            "send",
            &arg,
            |s| s.send_one(&h, &method, &path, &headers, body),
            outcome,
        )
    }

    fn send_all(
        &mut self,
        h: Resource<kio::caps::net::Http>,
        reqs: Vec<HttpRequest>,
    ) -> Result<Vec<Result<HttpResponse, kio::caps::net::HttpErr>>> {
        let arg = reqs.len().to_string();
        let refused = |rs: &Vec<_>| if rs.len() > BATCH_MAX { "invalid" } else { "ok" }.to_string();
        self.call(h.rep(), "send_all", &arg, |s| s.send_batch(&h, reqs), refused)
    }

    fn drop(&mut self, h: Resource<kio::caps::net::Http>) -> Result<()> {
        let _ = self.table.delete(Resource::<Origin>::new_own(h.rep()));
        Ok(())
    }
}

use kio::caps::net::{HttpRequest, HttpResponse};

/// The audit fields a finished request adds: its status if it got one, and the body bytes read.
fn received(done: &Performed) -> String {
    match &done.result {
        _ if done.busy => BUSY.into(),
        Ok(r) => format!(r#","status":{},"received":{}"#, r.status, done.read),
        Err(_) => format!(r#","received":{}"#, done.read),
    }
}

/// The audit field of a request not sent because the process had `MAX_HTTP_WORKERS` alive.
const BUSY: &str = r#","busy":true"#;

/// A request not sent: no HTTP worker was free.
fn busy() -> Performed {
    Performed {
        hops: 0,
        read: 0,
        ms: 0.0,
        busy: true,
        cut: false,
        result: Err(kio::caps::net::HttpErr::Timeout),
    }
}

fn audit_failed(e: io::Error) -> wasmtime::Error {
    wasmtime::Error::msg(format!("audit: cannot write the log: {e}"))
}

impl State {
    /// The grant behind `h`.
    fn origin(&self, h: &Resource<kio::caps::net::Http>) -> Option<Origin> {
        self.table.get(&Resource::<Origin>::new_borrow(h.rep())).ok().cloned()
    }

    fn get_url(
        &mut self,
        h: &Resource<kio::caps::net::Http>,
        path: String,
    ) -> Result<Result<Vec<u8>, kio::caps::net::HttpErr>> {
        use kio::caps::net::HttpErr;
        let Some(origin) = self.origin(h) else {
            return Ok(Err(HttpErr::Denied));
        };
        let Some(url) = origin.url(&path) else {
            return Ok(Err(HttpErr::Denied));
        };
        let req = Outbound {
            url,
            method: "GET",
            headers: Vec::new(),
            body: Vec::new(),
            timeout: HTTP_TIMEOUT,
            as_get: true,
        };
        let done = self.finish(&origin, req)?;
        self.audit_extra = origin.injected();
        if done.busy {
            self.audit_extra += BUSY;
        }
        Ok(done.result.map(|r| r.body))
    }

    /// Do `req` on an HTTP worker, then count its hops and charge what it read; `busy` if the
    /// process has no HTTP worker free.
    fn finish(&mut self, origin: &Origin, req: Outbound) -> Result<Performed> {
        let Some(slots) = HTTP_POOL.take(1) else {
            return Ok(busy());
        };
        let (o, deadline) = (origin.clone(), self.deadline);
        let budget = Arc::new(AtomicI64::new(self.io_left()));
        let b = budget.clone();
        let work: Box<dyn FnOnce() -> Performed + Send> = Box::new(move || perform(&o, &req, deadline, &b));
        // A worker the deadline left behind stops reading at its next chunk.
        let mut done = (self.within_all(vec![work], slots)).inspect_err(|_| budget.store(0, Relaxed))?;
        let done = done.pop().expect("one worker, one result");
        self.at_deadline(std::slice::from_ref(&done))?;
        self.settle(&done)?;
        Ok(done)
    }

    /// A request still in flight at the run's deadline stops the run as a timeout (H24): its
    /// result, `timeout` or a late answer, is never the program's. A request's own shorter
    /// timeout is still `timeout` to the program.
    fn at_deadline(&mut self, done: &[Performed]) -> Result<()> {
        if Instant::now() >= self.deadline || done.iter().any(|d| d.cut) {
            return Err(self.stop_timed_out());
        }
        Ok(())
    }

    /// Bytes the run may still read or write before `--io` stops it: a response's `read_body`
    /// budget, so no worker holds what the run could never be charged for.
    fn io_left(&self) -> i64 {
        i64::try_from(self.io_limit.saturating_sub(self.io)).unwrap_or(i64::MAX)
    }

    /// Count each redirect a request followed as a call, and charge the body read, even when
    /// it was refused.
    fn settle(&mut self, done: &Performed) -> Result<()> {
        for _ in 0..done.hops {
            self.count_call()?;
        }
        self.spend_io(done.read)
    }

    /// Check one `send`: the method is one of `METHODS` (else `invalid`) that `origin` allows
    /// (else `denied`), the path as `get`'s, the headers (`parse_headers`), the body at most
    /// `BODY_CAP` (charged to `--io` here). `extra` gets the audit fields known so far: the method,
    /// the header names once they pass, never a value, and the body size.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        origin: &Origin,
        method: &str,
        path: &str,
        headers: &[String],
        body: Vec<u8>,
        timeout: Duration,
        extra: &mut String,
    ) -> Result<Result<Outbound, kio::caps::net::HttpErr>> {
        use kio::caps::net::HttpErr;
        *extra += &format!(r#","http":{}"#, json(&method.chars().take(32).collect::<String>()));
        let Some(&method) = METHODS.iter().find(|m| **m == method) else {
            return Ok(Err(HttpErr::Invalid));
        };
        if !origin.allows(method) {
            return Ok(Err(HttpErr::Denied));
        }
        let Some(url) = origin.url(path) else {
            return Ok(Err(HttpErr::Denied));
        };
        let Some(headers) = parse_headers(headers) else {
            return Ok(Err(HttpErr::Invalid));
        };
        let names: Vec<String> = headers.iter().map(|(n, _)| json(n)).collect();
        *extra += &format!(r#","headers":[{}]"#, names.join(","));
        // The host sets this header; the program may not replace or repeat it.
        if headers.iter().any(|(n, _)| origin.injects(n)) {
            return Ok(Err(HttpErr::Invalid));
        }
        if body.len() > BODY_CAP {
            return Ok(Err(HttpErr::TooLarge));
        }
        *extra += &origin.injected();
        self.spend_io(body.len())?;
        *extra += &format!(r#","sent":{}"#, body.len());
        Ok(Ok(Outbound {
            url,
            method,
            headers,
            body,
            timeout,
            as_get: false,
        }))
    }

    fn send_one(
        &mut self,
        h: &Resource<kio::caps::net::Http>,
        method: &str,
        path: &str,
        headers: &[String],
        body: Vec<u8>,
    ) -> Result<Result<HttpResponse, kio::caps::net::HttpErr>> {
        let Some(origin) = self.origin(h) else {
            return Ok(Err(kio::caps::net::HttpErr::Denied));
        };
        let mut extra = String::new();
        let r = match self.prepare(&origin, method, path, headers, body, HTTP_TIMEOUT, &mut extra)? {
            Ok(req) => {
                let done = self.finish(&origin, req)?;
                extra += &received(&done);
                done.result.map(response)
            }
            Err(e) => Err(e),
        };
        self.audit_extra = extra;
        Ok(r)
    }

    /// `send_all`: every request checked as a `send`, each counted as a call (the batch's own
    /// count is the first), issued together on workers and awaited until the run's deadline. One
    /// audit line per request, with the batch's id and the request's index, before the batch's
    /// own line.
    fn send_batch(
        &mut self,
        h: &Resource<kio::caps::net::Http>,
        reqs: Vec<HttpRequest>,
    ) -> Result<Vec<Result<HttpResponse, kio::caps::net::HttpErr>>> {
        use kio::caps::net::HttpErr;
        self.batches += 1;
        let batch = self.batches;
        self.audit_extra = format!(r#","batch":{batch}"#);
        let n = reqs.len();
        if n > BATCH_MAX {
            return Ok((0..n).map(|_| Err(HttpErr::Invalid)).collect());
        }
        let Some(origin) = self.origin(h) else {
            return Ok((0..n).map(|_| Err(HttpErr::Denied)).collect());
        };
        for _ in 1..n {
            self.count_call()?;
        }
        let mut works: Vec<Box<dyn FnOnce() -> Performed + Send>> = Vec::new();
        // One read budget for the batch's workers, set once every body is charged.
        let budget = Arc::new(AtomicI64::new(0));
        // Per request: its path, body bytes and audit fields, and the error that refused it, if
        // any (else its worker's result is the next of `done`).
        let mut checked = Vec::with_capacity(n);
        for (i, r) in reqs.into_iter().enumerate() {
            let mut extra = format!(r#","batch":{batch},"index":{i}"#);
            let timeout = match r.timeout_ms {
                0 => HTTP_TIMEOUT,
                ms => Duration::from_millis(ms.into()).min(HTTP_TIMEOUT),
            };
            let sent = r.body.len() as u64;
            let refused = match self.prepare(&origin, &r.method, &r.path, &r.headers, r.body, timeout, &mut extra)? {
                Err(e) => Some(e),
                Ok(req) => {
                    let (o, deadline, budget) = (origin.clone(), self.deadline, budget.clone());
                    works.push(Box::new(move || perform(&o, &req, deadline, &budget)));
                    None
                }
            };
            checked.push((r.path, sent, extra, refused));
        }
        budget.store(self.io_left(), Relaxed);
        // The batch is sent whole or, with too few HTTP workers free in the process, not at all.
        let done = match HTTP_POOL.take(works.len()) {
            None => works.iter().map(|_| busy()).collect(),
            // Workers the deadline left behind stop reading at their next chunk.
            Some(slots) => {
                let done = (self.within_all(works, slots)).inspect_err(|_| budget.store(0, Relaxed))?;
                self.at_deadline(&done)?;
                done
            }
        };
        let mut done = done.into_iter();
        let mut results = Vec::with_capacity(n);
        for (path, sent, mut extra, refused) in checked {
            let (bytes, ms, result) = match refused {
                Some(e) => (0, 0.0, Err(e)),
                None => {
                    let d = done.next().expect("a worker per request issued");
                    self.settle(&d)?;
                    extra += &received(&d);
                    (sent + d.read as u64, d.ms, d.result.map(response))
                }
            };
            let why = outcome(&result);
            self.audit_line(h.rep(), "send", &path, bytes, &why, &extra, ms)
                .map_err(audit_failed)?;
            results.push(result);
        }
        Ok(results)
    }
}

fn response(r: Inbound) -> HttpResponse {
    HttpResponse {
        status: r.status,
        headers: r.headers,
        body: r.body,
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

/// The `Incoming` and `Outgoing` grants for one request, named `names`, and the slot holding
/// `deliver` until `Outgoing.send` sends the response down it.
fn web_grants(s: &mut State, request: Request, deliver: Deliver, names: (&str, &str)) -> Result<(Sent, Handles)> {
    let sent: Sent = Arc::new(Mutex::new(Some(deliver)));
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

/// Where the first valid `outgoing.send` sends its response, at once (H18).
pub type Deliver = tokio::sync::oneshot::Sender<Response>;

/// The sender until the response is sent, then `None`; shared so the host can answer after the
/// run when `main` never sent.
type Sent = Arc<Mutex<Option<Deliver>>>;

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
        if slot.is_none() {
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
        // The receiver may be gone (the client left); `main` runs on either way.
        if let Some(deliver) = slot.take() {
            let _ = deliver.send(Response { status, headers, body });
        }
        self.sent_at = Some(self.calls);
        Ok(Ok(()))
    }
}

/// Where an `Rng` grant's randomness comes from (`--rng name` or `--rng name=seed:N`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RngSpec {
    /// The operating system's (`getrandom`).
    Os,
    /// A ChaCha20 stream keyed by the seed: the same values every run. Under `--listen` each
    /// request's stream is keyed by the seed and the request number.
    Seed(u64),
}

impl RngSpec {
    /// Parse what follows `name=` in `--rng`, if anything: nothing is the OS, `seed:N` a seed.
    pub fn parse(spec: Option<&str>) -> std::result::Result<Self, String> {
        match spec {
            None => Ok(RngSpec::Os),
            Some(s) => match s.strip_prefix("seed:").and_then(|n| n.parse::<u64>().ok()) {
                Some(n) => Ok(RngSpec::Seed(n)),
                None => Err(format!(
                    "{s:?}: expected `seed:N` (N a non-negative integer), or no `=` for the OS's randomness"
                )),
            },
        }
    }

    /// For the audit header.
    fn label(&self) -> String {
        match self {
            RngSpec::Os => "os".into(),
            RngSpec::Seed(n) => format!("seed:{n}"),
        }
    }
}

/// What a `Clock` grant tells (`--clock name` or `--clock name=fixed:..`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockSpec {
    System,
    /// Always this many milliseconds since the Unix epoch.
    Fixed(i64),
}

impl ClockSpec {
    /// Parse what follows `name=` in `--clock`, if anything: nothing is the system clock,
    /// `fixed:<ms>` or `fixed:<RFC 3339 UTC time ending in Z>` one fixed time.
    pub fn parse(spec: Option<&str>) -> std::result::Result<Self, String> {
        let Some(s) = spec else { return Ok(ClockSpec::System) };
        let bad = || {
            format!(
                "{s:?}: expected `fixed:<ms>` or `fixed:YYYY-MM-DDTHH:MM:SS[.fff]Z`, or no `=` for the system clock"
            )
        };
        let t = s.strip_prefix("fixed:").ok_or_else(bad)?;
        if let Ok(ms) = t.parse::<i64>() {
            return Ok(ClockSpec::Fixed(ms));
        }
        rfc3339_ms(t).map(ClockSpec::Fixed).ok_or_else(bad)
    }

    fn label(&self) -> String {
        match self {
            ClockSpec::System => "system".into(),
            ClockSpec::Fixed(ms) => format!("fixed:{ms}"),
        }
    }
}

/// Milliseconds since the Unix epoch of `YYYY-MM-DDTHH:MM:SS[.fff]Z` (digits as written, UTC
/// only); `None` for anything else, including a day the month does not have.
fn rfc3339_ms(t: &str) -> Option<i64> {
    let t = t.strip_suffix('Z')?;
    let (date, time) = t.split_once('T')?;
    let num = |s: &str, digits: usize| {
        (s.len() == digits && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<i64>().ok())?
    };
    let [y, mo, d] = <[&str; 3]>::try_from(date.split('-').collect::<Vec<_>>()).ok()?;
    let (hms, frac) = time.split_once('.').map_or((time, "000"), |(a, b)| (a, b));
    let [h, mi, s] = <[&str; 3]>::try_from(hms.split(':').collect::<Vec<_>>()).ok()?;
    let (y, mo, d) = (num(y, 4)?, num(mo, 2)?, num(d, 2)?);
    let (h, mi, s) = (num(h, 2)?, num(mi, 2)?, num(s, 2)?);
    let ms = match frac.len() {
        1..=3 => num(frac, frac.len())? * 10i64.pow(3 - frac.len() as u32),
        _ => return None,
    };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in_month = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month[mo as usize - 1] || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    // Days from 1970-01-01 (Howard Hinnant's `days_from_civil`).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + s * 1000 + ms)
}

/// Where a `Log` grant writes (`--log name=path`, appended, or `name=-`, stderr), shared by every
/// run of the process; each line is one write under the lock, so lines never interleave.
#[derive(Clone)]
pub struct LogSink {
    out: SharedLog,
    /// The path, or `-`, for the audit header.
    label: String,
}

impl LogSink {
    pub fn stderr() -> Self {
        LogSink::new(Box::new(io::stderr()), "-")
    }

    /// `file`, opened to append by the caller (main.rs `open_append`), at `path`.
    pub fn file(file: std::fs::File, path: &str) -> Self {
        LogSink::new(Box::new(file), path)
    }

    pub fn new(out: Box<dyn Write + Send>, label: &str) -> Self {
        LogSink {
            out: Arc::new(Mutex::new(out)),
            label: label.into(),
        }
    }
}

/// The `--rng`, `--clock`, `--log`, `--db` and `--hmac` grants, by `main` parameter name. Every
/// run (every request under `--listen`) gets fresh handles to them; a `Log`'s byte budget is per
/// run; a `Db` outlives runs, so each run gets a handle to the same one (db.rs); an `Hmac` key is
/// read once, at start.
#[derive(Clone, Default)]
pub struct Sources {
    pub rngs: Vec<(String, RngSpec)>,
    pub clocks: Vec<(String, ClockSpec)>,
    pub logs: Vec<(String, LogSink)>,
    pub dbs: Vec<(String, Arc<db::Db>)>,
    pub hmacs: Vec<(String, HmacKey)>,
}

/// An `--hmac` grant: the key, and whether `sign` is refused (`:verify`). Its `Debug` never
/// shows the key.
#[derive(Clone, Debug)]
pub struct HmacKey {
    key: Secret,
    verify_only: bool,
}

impl HmacKey {
    /// A key of `value`'s bytes as given. Refused: empty, or over 8 KB (a file is read only that
    /// far, so a longer key would be cut short). `kio run` also refuses one of only whitespace.
    pub fn new(value: String, verify_only: bool) -> std::result::Result<Self, String> {
        if value.is_empty() {
            return Err("the key is empty\n  hint: set it to the key".into());
        }
        if value.len() > HEADER_MAX {
            return Err(format!("the key is over {HEADER_MAX} bytes\n  hint: use a shorter key"));
        }
        Ok(HmacKey {
            key: Secret(value.into()),
            verify_only,
        })
    }

    /// What the audit header shows: the methods the grant allows, never the key.
    pub(crate) fn label(&self) -> String {
        if self.verify_only { "verify" } else { "sign,verify" }.into()
    }

    /// Whether `sign` is refused (`:verify`).
    pub fn verify_only(&self) -> bool {
        self.verify_only
    }
}

/// Whether `main`'s report line has `name sign`: a `:verify` grant of `name` is then refused.
pub fn signs_in_report(report: &str, name: &str) -> bool {
    main_calls(report).contains(&(name.to_string(), "sign".to_string()))
}

/// HMAC-SHA256 (RFC 2104) of `msg` under `key`, over lang's SHA-256.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use lang::digest::sha256;
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + msg.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(msg);
    let mut outer = Vec::with_capacity(96);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}

/// A granted `Rng` in the resource table.
enum Randomness {
    Os,
    Seeded(Box<rand_chacha::ChaCha20Rng>),
}

impl Randomness {
    /// `spec`'s source for request `stream` (0 for a one-shot run): a seeded stream is the
    /// ChaCha20 stream of that number under a key holding the seed.
    fn new(spec: RngSpec, stream: u64) -> Self {
        use rand_chacha::rand_core::SeedableRng;
        match spec {
            RngSpec::Os => Randomness::Os,
            RngSpec::Seed(seed) => {
                let mut key = [0u8; 32];
                key[..8].copy_from_slice(&seed.to_le_bytes());
                let mut rng = rand_chacha::ChaCha20Rng::from_seed(key);
                rng.set_stream(stream);
                Randomness::Seeded(Box::new(rng))
            }
        }
    }

    fn fill(&mut self, out: &mut [u8]) -> Result<()> {
        use rand_chacha::rand_core::Rng;
        match self {
            Randomness::Os => {
                getrandom::fill(out).map_err(|e| wasmtime::Error::msg(format!("the OS gave no randomness: {e}")))
            }
            Randomness::Seeded(rng) => {
                rng.fill_bytes(out);
                Ok(())
            }
        }
    }

    fn next_u64(&mut self) -> Result<u64> {
        let mut b = [0u8; 8];
        self.fill(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
}

/// The most bytes one `Rng.bytes` gives (`lang::eval::RNG_BYTES_MAX`).
const RNG_BYTES_MAX: u32 = 65_536;

impl kio::caps::rng::Host for State {}
impl kio::caps::clock::Host for State {}
impl kio::caps::log::Host for State {}
impl kio::caps::hmac::Host for State {}

impl kio::caps::hmac::HostHmac for State {
    /// `denied` under `:verify`. The message's bytes are charged to `--io`.
    fn sign(
        &mut self,
        h: Resource<kio::caps::hmac::Hmac>,
        msg: Vec<u8>,
    ) -> Result<Result<Vec<u8>, kio::caps::hmac::HmacErr>> {
        self.call(
            h.rep(),
            "sign",
            "sign",
            |s| {
                s.audit_extra = format!(r#","len":{}"#, msg.len());
                if s.table.get(&Resource::<HmacKey>::new_borrow(h.rep()))?.verify_only {
                    return Ok(Err(kio::caps::hmac::HmacErr::Denied));
                }
                s.spend_io(msg.len())?;
                let k = s.table.get(&Resource::<HmacKey>::new_borrow(h.rep()))?;
                Ok(Ok(hmac_sha256(k.key.0.as_bytes(), &msg).to_vec()))
            },
            outcome,
        )
    }

    /// Whether `mac` is `msg`'s HMAC, compared in constant time; `false` for a wrong length.
    fn verify(&mut self, h: Resource<kio::caps::hmac::Hmac>, msg: Vec<u8>, mac: Vec<u8>) -> Result<bool> {
        self.call(
            h.rep(),
            "verify",
            "verify",
            |s| {
                s.audit_extra = format!(r#","len":{}"#, msg.len());
                s.spend_io(msg.len() + mac.len())?;
                let k = s.table.get(&Resource::<HmacKey>::new_borrow(h.rep()))?;
                Ok(lang::digest::ct_eq(&hmac_sha256(k.key.0.as_bytes(), &msg), &mac))
            },
            |v| v.to_string(),
        )
    }

    fn drop(&mut self, h: Resource<kio::caps::hmac::Hmac>) -> Result<()> {
        let _ = self.table.delete(Resource::<HmacKey>::new_own(h.rep()));
        Ok(())
    }
}

impl kio::caps::rng::HostRng for State {
    fn bytes(&mut self, r: Resource<kio::caps::rng::Rng>, n: u32) -> Result<Vec<u8>> {
        self.call(r.rep(), "bytes", "bytes", |s| s.random_bytes(&r, n), ok)
    }

    fn int(&mut self, r: Resource<kio::caps::rng::Rng>, lo: i64, hi: i64) -> Result<i64> {
        self.call(r.rep(), "int", "int", |s| s.random_int(&r, lo, hi), ok)
    }

    fn drop(&mut self, r: Resource<kio::caps::rng::Rng>) -> Result<()> {
        let _ = self.table.delete(Resource::<Randomness>::new_own(r.rep()));
        Ok(())
    }
}

impl State {
    /// The guest checks `n` before calling (lang/src/eval.rs `cap_arg_error`); one past the cap
    /// here came from code that skipped its check, and stops the run. The bytes are charged
    /// to `--io`.
    fn random_bytes(&mut self, r: &Resource<kio::caps::rng::Rng>, n: u32) -> Result<Vec<u8>> {
        self.audit_extra = format!(r#","n":{n}"#);
        if n > RNG_BYTES_MAX {
            wasmtime::bail!("Rng.bytes({n}): the count must be 0 to {RNG_BYTES_MAX}");
        }
        self.spend_io(n as usize)?;
        let src = self.table.get_mut(&Resource::<Randomness>::new_borrow(r.rep()))?;
        let mut out = vec![0; n as usize];
        src.fill(&mut out)?;
        Ok(out)
    }

    /// Uniform in `lo..hi` by rejection: draw 64 bits masked to the width's bit length until
    /// the draw is below the width (at least every second draw is). The same source, so the
    /// same values, for the interpreter and compiled code.
    fn random_int(&mut self, r: &Resource<kio::caps::rng::Rng>, lo: i64, hi: i64) -> Result<i64> {
        self.audit_extra = format!(r#","lo":{lo},"hi":{hi}"#);
        if lo >= hi {
            wasmtime::bail!("Rng.int({lo}, {hi}): lo must be less than hi");
        }
        // 1 to u64::MAX: the whole `i64` range fits.
        let width = hi.wrapping_sub(lo) as u64;
        let mask = u64::MAX.checked_shr((width - 1).leading_zeros()).unwrap_or(0);
        let src = self.table.get_mut(&Resource::<Randomness>::new_borrow(r.rep()))?;
        loop {
            let draw = src.next_u64()? & mask;
            if draw < width {
                return Ok(lo.wrapping_add(draw as i64));
            }
        }
    }
}

impl kio::caps::clock::HostClock for State {
    fn now_ms(&mut self, c: Resource<kio::caps::clock::Clock>) -> Result<i64> {
        self.call(
            c.rep(),
            "now_ms",
            "now_ms",
            |s| {
                let now = match s.table.get(&Resource::<ClockSpec>::new_borrow(c.rep()))? {
                    ClockSpec::System => unix_ms(),
                    ClockSpec::Fixed(ms) => *ms,
                };
                s.audit_extra = format!(r#","now":{now}"#);
                Ok(now)
            },
            ok,
        )
    }

    fn drop(&mut self, c: Resource<kio::caps::clock::Clock>) -> Result<()> {
        let _ = self.table.delete(Resource::<ClockSpec>::new_own(c.rep()));
        Ok(())
    }
}

/// Milliseconds since the Unix epoch now; negative before it.
fn unix_ms() -> i64 {
    let ms = |d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => ms(d),
        Err(e) => -ms(e.duration()),
    }
}

/// One run's handle on a `Log` grant: the sink, the line prefix (`req 3: ` under `--listen`)
/// and the bytes this run has written.
struct LogHandle {
    sink: LogSink,
    prefix: String,
    written: u64,
}

/// The most escaped bytes in one `Log.write` line, and in one run's lines together.
const LOG_LINE_MAX: usize = 64 << 10;
const LOG_RUN_MAX: u64 = 1 << 20;

/// A line as the log holds it: `\` as `\\`, LF, CR and tab as `\n`, `\r` and `\t`, any other
/// control character (C0, DEL, C1), the line and paragraph separators (U+2028, U+2029) and the
/// bidirectional controls (U+202A-E, U+2066-9) as `\u{XX}`. So a line is one line, holds no
/// terminal escape, and reads in the order it was written (no Trojan-source reordering can hide
/// its prefix). Never shorter than `s`.
fn escape_line(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}') => {
                o.push_str(&format!("\\u{{{:02x}}}", c as u32))
            }
            c => o.push(c),
        }
    }
    o
}

impl kio::caps::log::HostLog for State {
    fn write(&mut self, l: Resource<kio::caps::log::Log>, line: String) -> Result<Result<(), kio::caps::log::LogErr>> {
        self.call(l.rep(), "write", "write", |s| s.log_line(&l, line), outcome)
    }

    fn drop(&mut self, l: Resource<kio::caps::log::Log>) -> Result<()> {
        let _ = self.table.delete(Resource::<LogHandle>::new_own(l.rep()));
        Ok(())
    }
}

impl State {
    /// Escape `line`, refuse it past the line or run cap (`too-large`, nothing written), charge
    /// its bytes to `--io`, and write it with the prefix and a newline in one locked write.
    fn log_line(
        &mut self,
        l: &Resource<kio::caps::log::Log>,
        line: String,
    ) -> Result<Result<(), kio::caps::log::LogErr>> {
        // Escaping never shrinks a line, so one over the cap is refused before it is escaped:
        // escaping a line of control characters takes six times its bytes.
        if line.len() > LOG_LINE_MAX {
            return Ok(Err(kio::caps::log::LogErr::TooLarge));
        }
        let escaped = escape_line(&line);
        let h = self.table.get_mut(&Resource::<LogHandle>::new_borrow(l.rep()))?;
        let total = h.written.saturating_add(escaped.len() as u64);
        if escaped.len() > LOG_LINE_MAX || total > LOG_RUN_MAX {
            return Ok(Err(kio::caps::log::LogErr::TooLarge));
        }
        h.written = total;
        let (out, text) = (h.sink.out.clone(), format!("{}{escaped}", h.prefix));
        self.spend_io(escaped.len())?;
        write_line(&out, &text).map_err(|e| wasmtime::Error::msg(format!("log: cannot write: {e}")))?;
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
    /// Per-request limits for serving (web decision 9): 10 million fuel, 64 MB, 5 s, and 1,000
    /// calls (#164: with 10,000 one request could make 10,000 outbound requests in 5 s).
    pub fn web() -> Self {
        Limits {
            fuel: 10_000_000,
            timeout_secs: 5,
            memory: 64 << 20,
            calls: 1_000,
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
/// `req` is the request number under `--listen` (`None` for a one-shot run): a seeded `Rng`'s
/// stream and a `Log` line's prefix come from it.
fn push_grants(
    s: &mut State,
    dirs: Vec<(String, RealDir)>,
    https: Vec<(String, Origin)>,
    handles: Handles,
    kvs: KvGrants,
    sources: Sources,
    req: Option<u64>,
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
        add(name, "Http", s.table.push(origin.for_run())?.rep(), target);
    }
    for (name, kind, rep) in handles.each() {
        add(name, kind, rep, None);
    }
    for (name, store) in kvs {
        let target = Some(store.label());
        add(name, "Kv", s.table.push(store)?.rep(), target);
    }
    for (name, spec) in sources.rngs {
        let rng = Randomness::new(spec, req.unwrap_or(0));
        add(name, "Rng", s.table.push(rng)?.rep(), Some(spec.label()));
    }
    for (name, spec) in sources.clocks {
        add(name, "Clock", s.table.push(spec)?.rep(), Some(spec.label()));
    }
    for (name, sink) in sources.logs {
        let target = Some(sink.label.clone());
        let prefix = req.map_or(String::new(), |n| format!("req {n}: "));
        let handle = LogHandle {
            sink,
            prefix,
            written: 0,
        };
        add(name, "Log", s.table.push(handle)?.rep(), target);
    }
    for (name, db) in sources.dbs {
        let target = Some(db.label());
        add(name, "Db", s.table.push(db)?.rep(), target);
    }
    for (name, key) in sources.hmacs {
        let target = Some(key.label());
        add(name, "Hmac", s.table.push(key)?.rep(), target);
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
/// Once a response was sent, `after_send` counts the calls made after it: the last that many
/// `seq` numbers (H18).
fn end_audit(d: &mut State, fuel_used: u64, outcome: &str) -> Result<()> {
    let Some(a) = d.audit.as_mut() else { return Ok(()) };
    let after = (d.sent_at).map_or(String::new(), |n| format!(r#","after_send":{}"#, d.calls - n));
    let line = format!(
        r#"{{"event":"end","outcome":"{outcome}","calls":{},"io":{},"peak_memory":{},"fuel_used":{fuel_used}{after}}}"#,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A run with `BATCH_MAX` workers alive waits for one of them to end before it starts
    /// another, and stops as a timeout if none ends before its deadline.
    #[test]
    fn a_run_past_its_worker_cap_waits() {
        let limits = Limits {
            timeout_secs: 1,
            ..Limits::default()
        };
        let mut s = State::limited(&limits);
        *s.workers.live.lock().unwrap() = BATCH_MAX;
        let held = RunSlot(s.workers.clone());
        let freed = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        let start = Instant::now();
        let one = |s: &mut State| s.within_all(vec![Box::new(|| 7)], HOST_POOL.take(1).unwrap());
        assert_eq!(one(&mut s).unwrap(), [7]);
        assert!(start.elapsed() >= Duration::from_millis(150), "{:?}", start.elapsed());
        freed.join().unwrap();
        *s.workers.live.lock().unwrap() = BATCH_MAX;
        assert!(one(&mut s).is_err());
        assert!(s.timed_out);
        assert!(start.elapsed() >= Duration::from_millis(900));
    }

    /// RFC 4231's seven HMAC-SHA-256 test cases (case 5 truncated to 128 bits).
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let key4: Vec<u8> = (1..=25).collect();
        let long = b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.";
        let cases: [(&[u8], &[u8], &str); 7] = [
            (
                &[0x0b; 20],
                b"Hi There",
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                b"Jefe",
                b"what do ya want for nothing?",
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                &[0xaa; 20],
                &[0xdd; 50],
                "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
            ),
            (
                &key4,
                &[0xcd; 50],
                "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
            ),
            (&[0x0c; 20], b"Test With Truncation", "a3b6167473100ee06e0c796c2955552b"),
            (
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First",
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
            (
                &[0xaa; 131],
                long,
                "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
            ),
        ];
        for (i, (key, msg, want)) in cases.iter().enumerate() {
            let got = lang::digest::to_hex(&hmac_sha256(key, msg));
            assert!(got.starts_with(want), "case {}: {got}", i + 1);
        }
    }

    #[test]
    fn an_hmac_key_never_shows() {
        let k = HmacKey::new("whsec_never_shown".into(), true).unwrap();
        assert_eq!(format!("{k:?}"), "HmacKey { key: <redacted>, verify_only: true }");
        assert!(HmacKey::new(String::new(), false).is_err());
        assert!(HmacKey::new("k".repeat(HEADER_MAX + 1), false).is_err());
        assert!(HmacKey::new("k".repeat(HEADER_MAX), false).is_ok());
    }
}
