// POST /bookings: serde_json::Value parse, validate, answer 201 with JSON (hyper 1, tokio).
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};

fn err(status: u16, msg: &str) -> (u16, Value) { (status, json!({ "error": msg })) }
fn valid_email(e: &str) -> bool {
    if e.len() < 3 || e.len() > 254 { return false; }
    let parts: Vec<&str> = e.split('@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() { return false; }
    if !parts[1].contains('.') || parts[1].starts_with('.') || parts[1].ends_with('.') { return false; }
    e.bytes().all(|c| c > 32 && c != 127)
}
fn handle(method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    if method != "POST" || path != "/bookings" { return err(404, "no such route"); }
    let Ok(text) = std::str::from_utf8(body) else { return err(400, "body is not UTF-8") };
    let Ok(doc) = serde_json::from_str::<Value>(text) else { return err(400, "body is not valid JSON") };
    let Some(m) = doc.as_object() else { return err(400, "body must be a JSON object") };
    let Some(email) = m.get("email").and_then(|v| v.as_str()) else { return err(400, "`email` must be a string") };
    let Some(name) = m.get("name").and_then(|v| v.as_str()).map(str::trim) else { return err(400, "`name` must be a string") };
    let Some(slot) = m.get("slot").and_then(|v| v.as_i64()) else { return err(400, "`slot` must be an integer") };
    let Some(tags) = m.get("tags").and_then(|v| v.as_array()) else { return err(400, "`tags` must be a list") };
    if !valid_email(email) { return err(400, "`email` is not an address"); }
    if name.is_empty() || name.len() > 100 { return err(400, "`name` must be 1-100 bytes"); }
    if !(1..=10000).contains(&slot) { return err(400, "`slot` out of range"); }
    if tags.len() > 10 { return err(400, "at most 10 tags"); }
    if tags.iter().any(|t| !t.is_string()) { return err(400, "tags are strings"); }
    (201, json!({ "email": email.to_lowercase(), "id": slot * 7, "name": name, "status": "booked", "tags": tags.len() }))
}
async fn serve(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let body = req.into_body().collect().await.map(|b| b.to_bytes()).unwrap_or_default();
    let (status, v) = handle(&method, &path, &body);
    Ok(Response::builder().status(status).header("content-type", "application/json")
        .body(Full::new(Bytes::from(serde_json::to_vec(&v).unwrap()))).unwrap())
}
#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).expect("ip:port");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), hyper::service::service_fn(serve)).await;
        });
    }
}
