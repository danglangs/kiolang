//! Idle memory of `kio run --listen` after the first request (W13d). Prints RSS in KB so a
//! regression shows with `cargo test --test idle_rss -- --nocapture`; the bound is loose.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};

#[test]
fn idle_listen_rss() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args([
            "run",
            "../examples/serve/app.kio",
            "--dir",
            "public=../examples/tour/in",
        ])
        .args(["--listen", "127.0.0.1:0"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("listening on http://") {
        line.clear();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "kio exited before listening");
    }
    let port: u16 = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"GET /hello/kio HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let ps = Command::new("ps")
        .args(["-o", "rss=", "-p", &child.id().to_string()])
        .output()
        .unwrap();
    let kb: u64 = String::from_utf8_lossy(&ps.stdout).trim().parse().unwrap();
    let _ = child.kill();
    let _ = child.wait();
    println!("idle --listen RSS: {} MB ({kb} KB)", kb >> 10);
    // Release on macOS: about 54 MB, 100 MB before W13d. Debug builds are bigger; print only.
    if !cfg!(debug_assertions) {
        assert!(kb < 80 << 10, "idle RSS {kb} KB");
    }
}
