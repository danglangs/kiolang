//! `kio run --listen`: the HTTP accept loop on top of `Server` (web decisions 2-4).

use cap_std::fs::Dir;
use host::{Request, Response, Server};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Largest request body the host reads (decision 3).
const MAX_BODY: usize = 1 << 20;
/// How long a request waits for a worker before it is answered 503 (decision 4).
const QUEUE_WAIT: Duration = Duration::from_secs(1);

struct Ctx {
    server: Server,
    dirs: Vec<(String, Dir)>,
    workers: Arc<Semaphore>,
}

/// Serve until Ctrl-C. `dirs` are opened once; each request gets clones.
pub fn serve(server: Server, dirs: Vec<(String, Dir)>, addr: SocketAddr) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let permits = 2 * std::thread::available_parallelism().map_or(1, |n| n.get());
    let result = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("--listen {addr}: {e}"))?;
        let bound = listener.local_addr().map_err(|e| e.to_string())?;
        eprintln!("{}\nlistening on http://{bound}", server.report());
        let ctx = Arc::new(Ctx {
            server,
            dirs,
            workers: Arc::new(Semaphore::new(permits)),
        });
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(()),
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |req| handle(ctx.clone(), req));
                        // W8: header read timeout, header limits, slow-client body deadline, idle keep-alive.
                        let conn = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service);
                        let _ = conn.await;
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
    Ok(match read_request(req).await {
        Err(status) => reply(status),
        Ok(request) => match run(ctx, request).await {
            Ok(r) => respond(r),
            Err(status) => reply(status),
        },
    })
}

/// The request as the guest sees it, or the status to answer with instead.
async fn read_request(req: hyper::Request<Incoming>) -> Result<Request, StatusCode> {
    let (parts, body) = req.into_parts();
    let mut header_names = Vec::new();
    let mut header_values = Vec::new();
    for (name, value) in &parts.headers {
        let value = std::str::from_utf8(value.as_bytes()).map_err(|_| StatusCode::BAD_REQUEST)?;
        header_names.push(name.as_str().to_string()); // hyper lowercases names
        header_values.push(value.to_string());
    }
    // W8: body read deadline (a slow client must not hold a connection forever).
    let body = Limited::new(body, MAX_BODY).collect().await.map_err(|e| {
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
    // W8: cap the detached host-call workers (H5) and answer 503 at the cap.
    let permit = tokio::time::timeout(QUEUE_WAIT, ctx.workers.clone().acquire_owned())
        .await
        .map_err(|_| busy)?
        .map_err(|_| busy)?;
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
