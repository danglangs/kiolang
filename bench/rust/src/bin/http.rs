// kio-web's hello example in hyper: `GET /hello/:name` answers `{"hello": "<name>"}`.
// `http 127.0.0.1:PORT`.
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;

async fn handle(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let path = req.uri().path();
    let resp = match path.strip_prefix("/hello/").filter(|n| !n.contains('/')) {
        Some(name) => Response::builder()
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(format!(
                "{{\"hello\": {}}}",
                serde_json::to_string(name).unwrap()
            )))),
        None => Response::builder()
            .status(404)
            .body(Full::new(Bytes::from("not found"))),
    };
    Ok(resp.unwrap())
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).expect("ip:port");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), hyper::service::service_fn(handle))
                .await;
        });
    }
}
