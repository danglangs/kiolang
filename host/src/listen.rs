//! `kio run --listen`: the HTTP accept loop on top of `Server` (web decisions 2-4), and WebSocket
//! connections for a program whose `main` takes a `WsMessage` (WS1).

use crate::ws::{Upgrade, WsEvent, WsFrame};
use crate::{Request, Response, Server};
use cap_std::fs::Dir;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Method, StatusCode, Version};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{handshake::derive_accept_key, Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

/// Largest request body the host reads (decision 3).
const MAX_BODY: usize = 1 << 20;
/// How long a request waits for a worker before it is answered 503 (decision 4).
const QUEUE_WAIT: Duration = Duration::from_secs(1);
/// Requests that may wait for a worker; more are answered 503 at once.
const MAX_QUEUED: usize = 100;
/// Open connections, WebSockets included; past this the accept loop drops new ones.
const MAX_CONNECTIONS: usize = 1000;
/// Time to receive a request's headers (hyper's default is 30 s).
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Most headers in one request.
const MAX_HEADERS: usize = 100;
/// Bytes hyper buffers per connection, which bounds the header size.
const MAX_BUF: usize = 64 << 10;
/// Time for the whole body to arrive once the headers have; past it, 408.
const BODY_DEADLINE: Duration = Duration::from_secs(10);
/// A connection with no request for this long is closed.
const IDLE: Duration = Duration::from_secs(30);
/// Hard cap on a connection's life until it becomes a WebSocket, so a client that never reads
/// cannot hold a slot.
const LIFETIME: Duration = Duration::from_secs(60);
/// Largest WebSocket message (or frame); a larger one closes the connection with 1009.
const WS_MESSAGE: usize = 1 << 20;
/// Event bytes a connection may have waiting for their run; past it, close with 1008. An event
/// arriving to an empty queue is always taken, so one message up to `WS_MESSAGE` fits.
const WS_QUEUE: usize = 64 << 10;
/// What each waiting event counts besides its data, so empty messages cannot queue without bound.
const WS_EVENT_COST: usize = 64;
/// tungstenite's read buffer and the write buffer size at which it flushes (its defaults are
/// 128 KB each), so an idle connection stays small.
const WS_BUFFER: usize = 8 << 10;
/// Most bytes waiting to be written: one run's sends (`MAX_SENT`) with framing, plus control
/// frames. Past it a write fails and the socket is dropped.
const WS_WRITE_BUFFER: usize = 2 * crate::ws::MAX_SENT;
/// A WebSocket with no frame from the peer for this long is closed with 1000.
const WS_IDLE: Duration = Duration::from_secs(300);
/// Time to write one run's frames, or for the peer to answer a close; past it, the socket is dropped.
const WS_WRITE: Duration = Duration::from_secs(10);

/// The limits of `serve`, and the browser origins a WebSocket accepts besides its own host
/// (`--ws-origin`). Tests lower the limits through `serve_with`.
#[doc(hidden)]
pub struct ListenLimits {
    pub workers: usize,
    pub queued: usize,
    pub connections: usize,
    pub header_timeout: Duration,
    pub body_deadline: Duration,
    pub idle: Duration,
    pub lifetime: Duration,
    pub ws_idle: Duration,
    pub ws_origins: Vec<String>,
    pub hosts: Vec<String>,
}

impl Default for ListenLimits {
    fn default() -> Self {
        ListenLimits {
            workers: crate::compiled::default_workers(),
            queued: MAX_QUEUED,
            connections: MAX_CONNECTIONS,
            header_timeout: HEADER_TIMEOUT,
            body_deadline: BODY_DEADLINE,
            idle: IDLE,
            lifetime: LIFETIME,
            ws_idle: WS_IDLE,
            ws_origins: Vec::new(),
            hosts: Vec::new(),
        }
    }
}

struct Ctx {
    server: Server,
    dirs: Vec<(String, Dir)>,
    workers: Arc<Semaphore>,
    queue: Semaphore,
    body_deadline: Duration,
    ws_idle: Duration,
    ws_origins: Vec<String>,
    /// Whether `Host` is checked (loopback bind or `--host`), and the extra names it may be.
    check_host: bool,
    hosts: Vec<String>,
    /// The next WebSocket connection id.
    next_conn: AtomicU64,
}

/// Serve until Ctrl-C. `dirs` are opened once; each request gets clones. A WebSocket upgrade
/// from a browser page is accepted from the server's own host or one of `ws_origins`. On a
/// loopback `addr`, or when `hosts` is given, a request's `Host` must be a local name or in `hosts`.
pub fn serve(
    server: Server,
    dirs: Vec<(String, Dir)>,
    addr: SocketAddr,
    ws_origins: Vec<String>,
    hosts: Vec<String>,
) -> Result<(), String> {
    let limits = ListenLimits {
        ws_origins,
        hosts,
        ..Default::default()
    };
    serve_with(server, dirs, addr, limits, |_| {})
}

/// `serve` with explicit limits; `bound` is told the address once it is listening.
#[doc(hidden)]
pub fn serve_with(
    server: Server,
    dirs: Vec<(String, Dir)>,
    addr: SocketAddr,
    limits: ListenLimits,
    bound: impl FnOnce(SocketAddr),
) -> Result<(), String> {
    // Requests run compiled code on the runtime's blocking threads (`with_permit`).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(crate::RUN_STACK)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let result = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("--listen {addr}: {e}"))?;
        let local = listener.local_addr().map_err(|e| e.to_string())?;
        let scheme = if server.ws_params().is_some() { "ws" } else { "http" };
        eprintln!(
            "{}\nprogram sha256:{}\nlistening on {scheme}://{local}",
            server.report(),
            crate::program_hash(server.files())
        );
        bound(local);
        let ctx = Arc::new(Ctx {
            server,
            dirs,
            workers: Arc::new(Semaphore::new(limits.workers)),
            queue: Semaphore::new(limits.queued),
            body_deadline: limits.body_deadline,
            ws_idle: limits.ws_idle,
            ws_origins: limits.ws_origins,
            check_host: local.ip().is_loopback() || !limits.hosts.is_empty(),
            hosts: limits.hosts,
            next_conn: AtomicU64::new(1),
        });
        let open = Arc::new(Semaphore::new(limits.connections));
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(()),
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    // Past the cap the stream is dropped, which closes it. A WebSocket keeps a
                    // clone of the slot until it closes.
                    let Ok(slot) = open.clone().try_acquire_owned() else { continue };
                    let slot = Arc::new(slot);
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        // Dropping the future on timeout closes the socket, unless it was upgraded.
                        let _ = tokio::time::timeout(limits.lifetime, async {
                        // Requests in flight, so that only a connection with none is idle.
                        let (busy, mut changes) = watch::channel(0usize);
                        let service = service_fn(move |req| {
                            let busy = busy.clone();
                            let ctx = ctx.clone();
                            let slot = slot.clone();
                            async move {
                                busy.send_modify(|n| *n += 1);
                                let response = handle(ctx, slot, req).await;
                                busy.send_modify(|n| *n -= 1);
                                response
                            }
                        });
                        let conn = hyper::server::conn::http1::Builder::new()
                            .timer(TokioTimer::new())
                            .header_read_timeout(limits.header_timeout)
                            .max_headers(MAX_HEADERS)
                            .max_buf_size(MAX_BUF)
                            .serve_connection(TokioIo::new(stream), service)
                            .with_upgrades();
                        tokio::pin!(conn);
                        loop {
                            let idle = *changes.borrow() == 0;
                            tokio::select! {
                                _ = conn.as_mut() => break,
                                _ = tokio::time::sleep(limits.idle), if idle => conn.as_mut().graceful_shutdown(),
                                _ = changes.changed() => {}
                            }
                        }
                        })
                        .await;
                    });
                }
            }
        }
    });
    // In-flight guests are abandoned: they are bounded by their own limits and the process is exiting.
    rt.shutdown_background();
    result
}

async fn handle(
    ctx: Arc<Ctx>,
    slot: Arc<OwnedSemaphorePermit>,
    req: hyper::Request<Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    if ctx.check_host && !host_allowed(req.headers(), &ctx.hosts) {
        let body = "this Host is not served here: the server allows it with `--host <name>`\n";
        let mut r = hyper::Response::new(Full::new(Bytes::from(body)));
        *r.status_mut() = StatusCode::MISDIRECTED_REQUEST;
        return Ok(r);
    }
    if ctx.server.ws_params().is_some() {
        return Ok(upgrade(ctx, slot, req));
    }
    Ok(match read_request(req, ctx.body_deadline).await {
        Err(status) => reply(status),
        Ok(request) => match run(ctx, request).await {
            Ok(r) => respond(r),
            Err(status) => reply(status),
        },
    })
}

/// The request as the guest sees it, or the status to answer with instead.
async fn read_request(req: hyper::Request<Incoming>, deadline: Duration) -> Result<Request, StatusCode> {
    let (parts, body) = req.into_parts();
    let mut header_names = Vec::new();
    let mut header_values = Vec::new();
    for (name, value) in &parts.headers {
        let value = std::str::from_utf8(value.as_bytes()).map_err(|_| StatusCode::BAD_REQUEST)?;
        header_names.push(name.as_str().to_string()); // hyper lowercases names
        header_values.push(value.to_string());
    }
    // A slow client must not hold a connection forever.
    let body = tokio::time::timeout(deadline, Limited::new(body, MAX_BODY).collect())
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
        .map_err(|e| {
            if e.is::<LengthLimitError>() {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            }
        })?;
    Ok(Request {
        method: parts.method.as_str().to_string(),
        target: parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_string(),
        header_names,
        header_values,
        body: body.to_bytes().to_vec(),
    })
}

/// Run the guest on a blocking thread, once a worker permit is free.
async fn run(ctx: Arc<Ctx>, request: Request) -> Result<Response, StatusCode> {
    on_worker(ctx, move |server, dirs| server.handle(request, dirs)).await
}

/// Run `work` on a blocking thread with clones of the granted dirs, once a worker permit is
/// free: 503 if the queue is full or the wait passes `QUEUE_WAIT`, or if `work` could not start.
async fn on_worker<T: Send + 'static>(
    ctx: Arc<Ctx>,
    work: impl FnOnce(&Server, Vec<(String, Dir)>) -> Result<T, String> + Send + 'static,
) -> Result<T, StatusCode> {
    let busy = StatusCode::SERVICE_UNAVAILABLE;
    let waiting = ctx.queue.try_acquire().map_err(|_| busy)?;
    let permit = tokio::time::timeout(QUEUE_WAIT, ctx.workers.clone().acquire_owned())
        .await
        .map_err(|_| busy)?
        .map_err(|_| busy)?;
    drop(waiting);
    with_permit(ctx, permit, work).await
}

/// Run `work` as `on_worker` does, holding `permit`: 503 if it could not start.
async fn with_permit<T: Send + 'static>(
    ctx: Arc<Ctx>,
    permit: OwnedSemaphorePermit,
    work: impl FnOnce(&Server, Vec<(String, Dir)>) -> Result<T, String> + Send + 'static,
) -> Result<T, StatusCode> {
    let busy = StatusCode::SERVICE_UNAVAILABLE;
    let work = tokio::task::spawn_blocking(move || {
        let dirs = ctx
            .dirs
            .iter()
            .map(|(name, dir)| Ok((name.clone(), dir.try_clone()?)))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let out = work(&ctx.server, dirs);
        drop(permit);
        out
    });
    work.await.map_err(|_| busy)?.map_err(|e| {
        eprintln!("request refused: {e}");
        busy
    })
}

fn respond(r: Response) -> hyper::Response<Full<Bytes>> {
    let mut out = hyper::Response::builder().status(r.status);
    // Framing headers belong to the host: a guest-set length could desync a keep-alive connection.
    let framing = [
        "content-length",
        "transfer-encoding",
        "connection",
        "keep-alive",
        "upgrade",
    ];
    for (name, value) in r
        .headers
        .iter()
        .filter(|(n, _)| !framing.contains(&n.to_ascii_lowercase().as_str()))
    {
        out = out.header(name, value);
    }
    // hyper sets content-length. A status or header it rejects is the guest's fault: 500.
    out.body(Full::new(Bytes::from(r.body)))
        .unwrap_or_else(|_| reply(StatusCode::INTERNAL_SERVER_ERROR))
}

fn reply(status: StatusCode) -> hyper::Response<Full<Bytes>> {
    let mut r = hyper::Response::new(Full::new(Bytes::new()));
    *r.status_mut() = status;
    r
}

/// Accept a WebSocket upgrade (RFC 6455 4.2) with 101 and serve the connection on its own task,
/// which keeps `slot` until it ends. Any other request is 426.
fn upgrade(
    ctx: Arc<Ctx>,
    slot: Arc<OwnedSemaphorePermit>,
    mut req: hyper::Request<Incoming>,
) -> hyper::Response<Full<Bytes>> {
    let headers = req.headers();
    // Whether header `name` lists `token`, ignoring case.
    let lists = |name: &str, token: &str| {
        headers.get_all(name).iter().any(|v| {
            v.to_str()
                .is_ok_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
        })
    };
    // The key is 16 bytes in base64.
    let key = headers.get("sec-websocket-key").filter(|k| k.len() == 24);
    let valid = req.method() == Method::GET
        && req.version() >= Version::HTTP_11
        && lists("connection", "upgrade")
        && lists("upgrade", "websocket")
        && headers.get("sec-websocket-version").is_some_and(|v| v == "13");
    let accept = match key {
        Some(key) if valid => derive_accept_key(key.as_bytes()),
        _ => {
            let body = "this server speaks WebSocket only: send an upgrade request (RFC 6455)\n";
            let mut r = hyper::Response::new(Full::new(Bytes::from(body)));
            *r.status_mut() = StatusCode::UPGRADE_REQUIRED;
            r.headers_mut().insert("upgrade", "websocket".parse().unwrap());
            r.headers_mut().insert("sec-websocket-version", "13".parse().unwrap());
            return r;
        }
    };
    if !origin_allowed(req.headers(), &ctx.ws_origins) {
        let body = "this origin may not open a WebSocket here: the server allows it with `--ws-origin <origin>`\n";
        let mut r = hyper::Response::new(Full::new(Bytes::from(body)));
        *r.status_mut() = StatusCode::FORBIDDEN;
        return r;
    }
    let Some(request) = upgrade_request(&req) else {
        return reply(StatusCode::BAD_REQUEST);
    };
    let upgraded = hyper::upgrade::on(&mut req);
    tokio::spawn(async move {
        if let Ok(io) = upgraded.await {
            let config = WebSocketConfig::default()
                .max_message_size(Some(WS_MESSAGE))
                .max_frame_size(Some(WS_MESSAGE))
                .read_buffer_size(WS_BUFFER)
                .write_buffer_size(WS_BUFFER)
                .max_write_buffer_size(WS_WRITE_BUFFER);
            let ws = WebSocketStream::from_raw_socket(TokioIo::new(io), Role::Server, Some(config)).await;
            connection(ctx, ws, request).await;
        }
        drop(slot);
    });
    let mut r = hyper::Response::new(Full::new(Bytes::new()));
    *r.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let h = r.headers_mut();
    h.insert("upgrade", "websocket".parse().unwrap());
    h.insert("connection", "upgrade".parse().unwrap());
    h.insert("sec-websocket-accept", accept.parse().unwrap());
    r
}

/// What the connection's events see of its upgrade request; `None` if a header value is not
/// UTF-8, as for an HTTP request.
fn upgrade_request(req: &hyper::Request<Incoming>) -> Option<Arc<Upgrade>> {
    let mut headers = Vec::new();
    for (name, value) in req.headers() {
        // hyper lowercases names.
        headers.push((name.as_str().to_string(), value.to_str().ok()?.to_string()));
    }
    let target = req.uri().path_and_query().map_or("/", |p| p.as_str()).to_string();
    Some(Arc::new(Upgrade { target, headers }))
}

/// Whether a request's `Host` names this server, against DNS rebinding: a page on `evil.example`
/// whose name resolves to a local address sends `Host: evil.example`. Allowed are `localhost`,
/// `127.0.0.1`, `[::1]` (any port, any case) and the names in `extra`, which are lowercase. A
/// missing, repeated or non-UTF-8 `Host` is refused.
fn host_allowed(headers: &hyper::HeaderMap, extra: &[String]) -> bool {
    let mut hosts = headers.get_all("host").iter();
    let host = match (hosts.next(), hosts.next()) {
        (Some(h), None) => h,
        _ => return false,
    };
    let Ok(host) = host.to_str() else {
        return false;
    };
    let name = if host.starts_with('[') {
        host.split_once(']').and_then(|(n, rest)| match rest {
            "" => Some(&host[..=n.len()]),
            _ => rest.strip_prefix(':').filter(|p| port(p)).map(|_| &host[..=n.len()]),
        })
    } else {
        match host.split_once(':') {
            None => Some(host),
            Some((n, p)) => port(p).then_some(n),
        }
    };
    let Some(name) = name else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    ["localhost", "127.0.0.1", "[::1]"].contains(&name.as_str()) || extra.contains(&name)
}

fn port(p: &str) -> bool {
    !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())
}

/// Whether a WebSocket upgrade may proceed, against cross-site hijacking: a browser always sends
/// `Origin`, so a request without one is not from a web page. With one, it must name the host the
/// request was sent to (`http://H` or `https://H` for `Host: H`) or be listed in `allowed`.
fn origin_allowed(headers: &hyper::HeaderMap, allowed: &[String]) -> bool {
    let mut origins = headers.get_all("origin").iter();
    let origin = match (origins.next(), origins.next()) {
        (None, _) => return true,
        (Some(o), None) => o,
        _ => return false,
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let host = headers.get("host").and_then(|h| h.to_str().ok());
    let same = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .zip(host)
        .is_some_and(|(o, h)| o.eq_ignore_ascii_case(h));
    same || allowed.iter().any(|a| a == origin)
}

/// What a run on the worker pool came back with: the frames to write, `None` if the run failed,
/// or the status it could not start with.
type Ran = Result<Option<Vec<WsFrame>>, StatusCode>;

/// Serve one WebSocket until it closes. Events (open, each message, close) wait in `queue` and
/// run one at a time, in order, each in a fresh sandbox on the worker pool; frames are read
/// meanwhile. An event waits for a worker as long as it takes (the queue bounds what waits). A
/// run's sends are written after it ends. A failed run closes with 1011 and drops the waiting
/// messages; the close event always comes last, once no more frames will be read.
async fn connection<S>(ctx: Arc<Ctx>, ws: WebSocketStream<S>, upgrade: Arc<Upgrade>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let conn = ctx.next_conn.fetch_add(1, Relaxed);
    let (mut sink, mut stream) = ws.split();
    let event = |kind, data| WsEvent {
        kind,
        conn,
        data,
        upgrade: upgrade.clone(),
    };
    let cost = |e: &WsEvent| e.data.len() + WS_EVENT_COST;
    let mut queue = VecDeque::from([event("open", vec![])]);
    let mut queued = WS_EVENT_COST;
    let mut running: Option<tokio::task::JoinHandle<Ran>> = None;
    // A close frame was sent or received: messages are no longer delivered and sends are `closed`.
    let mut closed = false;
    // No more frames will be read; the close event has been queued.
    let mut done = false;
    // When to close an idle connection, or, once `closed`, to stop waiting for the peer.
    let mut deadline = Instant::now() + ctx.ws_idle;
    // Drop the messages waiting, keeping the close event.
    macro_rules! drop_waiting {
        () => {{
            queue.retain(|e| e.kind == "close");
            queued = queue.iter().map(cost).sum();
        }};
    }
    // The connection is over: drop the messages waiting and queue the close event.
    macro_rules! end {
        () => {{
            if !done {
                (done, closed) = (true, true);
                drop_waiting!();
                queue.push_back(event("close", vec![]));
                queued += WS_EVENT_COST;
            }
        }};
    }
    // Messages are no longer delivered; the peer has `WS_WRITE` to finish the closing handshake.
    macro_rules! closing {
        () => {{
            closed = true;
            deadline = Instant::now() + WS_WRITE;
            drop_waiting!();
        }};
    }
    // The host closes with `code`, unless a close frame already went either way.
    macro_rules! close {
        ($code:expr) => {{
            if !closed {
                closing!();
                if !write(&mut sink, vec![WsFrame::Close($code)]).await {
                    end!();
                }
            }
        }};
    }
    loop {
        if running.is_none() {
            if let Some(e) = queue.pop_front() {
                queued -= cost(&e);
                let closed = closed || e.kind == "close";
                let ctx = ctx.clone();
                running = Some(tokio::spawn(async move {
                    let permit = ctx.workers.clone().acquire_owned().await;
                    let permit = permit.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    with_permit(ctx, permit, move |s, dirs| s.handle_ws(e, closed, dirs)).await
                }));
            }
        }
        if running.is_none() && done {
            return;
        }
        tokio::select! {
            ran = async { running.as_mut().unwrap().await }, if running.is_some() => {
                running = None;
                match ran.unwrap_or(Ok(None)) {
                    Ok(Some(frames)) if !closed => {
                        if frames.iter().any(|f| matches!(f, WsFrame::Close(_))) {
                            closing!();
                        }
                        if !write(&mut sink, frames).await {
                            end!();
                        }
                    }
                    Ok(Some(_)) => {}
                    // The cause is logged where the run failed.
                    Ok(None) | Err(_) => {
                        drop_waiting!();
                        close!(1011);
                    }
                }
            }
            frame = stream.next(), if !done => {
                if !closed {
                    deadline = Instant::now() + ctx.ws_idle;
                }
                let data = match frame {
                    Some(Ok(Message::Text(t))) => Some(("text", t.as_bytes().to_vec())),
                    Some(Ok(Message::Binary(b))) => Some(("binary", b.to_vec())),
                    // tungstenite answers pings and close frames itself.
                    Some(Ok(Message::Close(_))) => { closing!(); None }
                    Some(Ok(_)) => None,
                    Some(Err(WsError::Capacity(_))) => { close!(1009); None }
                    Some(Err(WsError::Utf8(_))) => { close!(1007); None }
                    // The peer is gone: there is no one to send a close to.
                    Some(Err(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake))) => { end!(); None }
                    Some(Err(WsError::Protocol(_))) => { close!(1002); None }
                    None | Some(Err(_)) => { end!(); None }
                };
                match data {
                    // Once closed, a message is not delivered.
                    Some(_) if closed => {}
                    Some((kind, data)) => {
                        let e = event(kind, data);
                        if !queue.is_empty() && queued + cost(&e) > WS_QUEUE {
                            close!(1008);
                        } else {
                            queued += cost(&e);
                            queue.push_back(e);
                        }
                    }
                    None => {}
                }
            }
            _ = tokio::time::sleep_until(deadline), if !done => {
                if closed {
                    // The peer did not finish the closing handshake: drop the socket.
                    end!();
                } else {
                    close!(1000);
                }
            }
        }
    }
}

/// Write `frames` in order within `WS_WRITE`; `false` if the connection failed or stalled.
async fn write<S>(sink: &mut S, frames: Vec<WsFrame>) -> bool
where
    S: futures_util::Sink<Message> + Unpin,
{
    let send = async {
        for f in frames {
            let m = match f {
                WsFrame::Text(s) => Message::text(s),
                WsFrame::Binary(b) => Message::binary(b),
                WsFrame::Close(code) => Message::Close(Some(CloseFrame {
                    code: code.into(),
                    reason: "".into(),
                })),
            };
            sink.feed(m).await.map_err(|_| ())?;
        }
        sink.flush().await.map_err(|_| ())
    };
    matches!(tokio::time::timeout(WS_WRITE, send).await, Ok(Ok(())))
}

#[cfg(test)]
mod tests {
    use super::host_allowed;

    fn allowed(host: Option<&str>, extra: &[&str]) -> bool {
        let mut h = hyper::HeaderMap::new();
        if let Some(host) = host {
            h.insert("host", host.parse().unwrap());
        }
        let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        host_allowed(&h, &extra)
    }

    #[test]
    fn host_forms() {
        for ok in [
            "localhost",
            "LocalHost:8080",
            "127.0.0.1",
            "127.0.0.1:80",
            "[::1]",
            "[::1]:3000",
            "[::1]:0",
        ] {
            assert!(allowed(Some(ok), &[]), "{ok}");
        }
        for bad in [
            "evil.example",
            "evil.example:80",
            "localhost.evil.example",
            "localhost:",
            "localhost:x",
            "127.0.0.2",
            "[::2]",
            "[::1]x",
            "::1",
            "",
            "localhost:80:80",
        ] {
            assert!(!allowed(Some(bad), &[]), "{bad}");
        }
        assert!(!allowed(None, &[]));
        assert!(allowed(Some("App.Example:9"), &["app.example"]));
        assert!(allowed(Some("localhost"), &["app.example"]));
        assert!(!allowed(Some("evil.example"), &["app.example"]));
    }
}
