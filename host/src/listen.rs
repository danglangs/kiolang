//! `kio run --listen`: the HTTP accept loop on top of `Server` (web decisions 2-4).

use crate::{Request, Response, Server};
use cap_std::fs::Dir;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::StatusCode;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Semaphore};

/// Largest request body the host reads (decision 3).
const MAX_BODY: usize = 1 << 20;
/// How long a request waits for a worker before it is answered 503 (decision 4).
const QUEUE_WAIT: Duration = Duration::from_secs(1);
/// Requests that may wait for a worker; more are answered 503 at once.
const MAX_QUEUED: usize = 100;
/// Open connections; past this the accept loop drops new ones.
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
/// Hard cap on a connection's life, so a client that never reads cannot hold a slot.
const LIFETIME: Duration = Duration::from_secs(60);

/// The limits of `serve`. Tests lower them through `serve_with`.
#[doc(hidden)]
pub struct ListenLimits {
    pub workers: usize,
    pub queued: usize,
    pub connections: usize,
    pub header_timeout: Duration,
    pub body_deadline: Duration,
    pub idle: Duration,
    pub lifetime: Duration,
}

impl Default for ListenLimits {
    fn default() -> Self {
        ListenLimits {
            workers: 2 * std::thread::available_parallelism().map_or(1, |n| n.get()),
            queued: MAX_QUEUED,
            connections: MAX_CONNECTIONS,
            header_timeout: HEADER_TIMEOUT,
            body_deadline: BODY_DEADLINE,
            idle: IDLE,
            lifetime: LIFETIME,
        }
    }
}

struct Ctx {
    server: Server,
    dirs: Vec<(String, Dir)>,
    workers: Arc<Semaphore>,
    queue: Semaphore,
    body_deadline: Duration,
}

/// Serve until Ctrl-C. `dirs` are opened once; each request gets clones.
pub fn serve(server: Server, dirs: Vec<(String, Dir)>, addr: SocketAddr) -> Result<(), String> {
    serve_with(server, dirs, addr, ListenLimits::default(), |_| {})
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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let result = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("--listen {addr}: {e}"))?;
        let local = listener.local_addr().map_err(|e| e.to_string())?;
        eprintln!(
            "{}\nprogram sha256:{}\nlistening on http://{local}",
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
        });
        let open = Arc::new(Semaphore::new(limits.connections));
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(()),
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    // Past the cap the stream is dropped, which closes it.
                    let Ok(slot) = open.clone().try_acquire_owned() else { continue };
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        // Dropping the future on timeout closes the socket.
                        let _ = tokio::time::timeout(limits.lifetime, async {
                        // Requests in flight, so that only a connection with none is idle.
                        let (busy, mut changes) = watch::channel(0usize);
                        let service = service_fn(move |req| {
                            let busy = busy.clone();
                            let ctx = ctx.clone();
                            async move {
                                busy.send_modify(|n| *n += 1);
                                let response = handle(ctx, req).await;
                                busy.send_modify(|n| *n -= 1);
                                response
                            }
                        });
                        let conn = hyper::server::conn::http1::Builder::new()
                            .timer(TokioTimer::new())
                            .header_read_timeout(limits.header_timeout)
                            .max_headers(MAX_HEADERS)
                            .max_buf_size(MAX_BUF)
                            .serve_connection(TokioIo::new(stream), service);
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
                        drop(slot);
                    });
                }
            }
        }
    });
    // In-flight guests are abandoned: they are bounded by their own limits and the process is exiting.
    rt.shutdown_background();
    result
}

async fn handle(ctx: Arc<Ctx>, req: hyper::Request<Incoming>) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
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
    let busy = StatusCode::SERVICE_UNAVAILABLE;
    let waiting = ctx.queue.try_acquire().map_err(|_| busy)?;
    let permit = tokio::time::timeout(QUEUE_WAIT, ctx.workers.clone().acquire_owned())
        .await
        .map_err(|_| busy)?
        .map_err(|_| busy)?;
    drop(waiting);
    let work = tokio::task::spawn_blocking(move || {
        let dirs = ctx
            .dirs
            .iter()
            .map(|(name, dir)| Ok((name.clone(), dir.try_clone()?)))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let response = ctx.server.handle(request, dirs);
        drop(permit);
        response
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
