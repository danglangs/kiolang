//! A small HTTP/1.1 upstream on 127.0.0.1 for `Http` grant tests, one thread per connection:
//! `/hang` never answers, `/wait` answers after 300 ms, `/trickle` sends its headers and half its
//! body then stalls, `/redirect` is a 302 to `/landed`, and any other path is 200 with
//! `METHOD path body` as its body.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A request as received: method, path, body.
pub type Seen = (String, String, Vec<u8>);
/// A request as received: path and header lines.
pub type Head = (String, Vec<String>);

pub struct Upstream {
    /// `http://127.0.0.1:port`, for an `--http` grant.
    pub grant: String,
    /// The most requests in flight at once.
    pub most: Arc<AtomicUsize>,
    /// Each request's method, path and body, as received.
    pub seen: Arc<Mutex<Vec<Seen>>>,
    /// Each request's path and header lines (`Name: value`, as received).
    pub heads: Arc<Mutex<Vec<Head>>>,
}

pub fn start() -> Upstream {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let up = Upstream {
        grant: format!("http://{}", listener.local_addr().unwrap()),
        most: Arc::default(),
        seen: Arc::default(),
        heads: Arc::default(),
    };
    let (most, seen, heads) = (up.most.clone(), up.seen.clone(), up.heads.clone());
    let live = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (most, seen, heads, live) = (most.clone(), seen.clone(), heads.clone(), live.clone());
            std::thread::spawn(move || {
                let now = live.fetch_add(1, SeqCst) + 1;
                most.fetch_max(now, SeqCst);
                answer(&stream, &seen, &heads);
                live.fetch_sub(1, SeqCst);
            });
        }
    });
    up
}

fn answer(stream: &std::net::TcpStream, seen: &Mutex<Vec<Seen>>, heads: &Mutex<Vec<Head>>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut words = line.split_whitespace();
    let (method, path) = (
        words.next().unwrap_or("").to_string(),
        words.next().unwrap_or("").to_string(),
    );
    let mut length = 0;
    let mut lines = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() {
            return;
        }
        lines.push(h.trim_end().to_string());
        let h = h.trim_end().to_ascii_lowercase();
        if h.is_empty() {
            break;
        }
        if let Some(n) = h.strip_prefix("content-length: ") {
            length = n.parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    seen.lock().unwrap().push((method.clone(), path.clone(), body.clone()));
    lines.pop();
    heads.lock().unwrap().push((path.clone(), lines));
    let mut w = stream;
    if path == "/redirect" {
        let _ =
            w.write_all(b"HTTP/1.1 302 Found\r\nLocation: /landed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    match path.as_str() {
        "/hang" => std::thread::sleep(Duration::from_secs(60)),
        "/wait" => std::thread::sleep(Duration::from_millis(300)),
        _ => {}
    }
    let mut out = format!("{method} {path} ").into_bytes();
    out.extend(body);
    let _ = write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        out.len()
    );
    if path == "/trickle" {
        let _ = w.write_all(&out[..out.len() / 2]);
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    let _ = w.write_all(&out);
}
