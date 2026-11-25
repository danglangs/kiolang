// In-process probe (C23): `Server::handle` in a loop on THREADS threads, no sockets, hyper or
// tokio, under the limits `--listen` uses (`Limits::web`), each request in a fresh instance.
// probe hello|bookings THREADS N FILE.kio [LIB.kio...]   (env PUBLIC=dir grants `public`, BODY=file)
use kio_host::{open_dir, Limits, Request, Server, RUN_STACK};
use std::time::Instant;

/// Process (user, system) CPU seconds.
fn cpu() -> (f64, f64) {
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    (t(r.ru_utime), t(r.ru_stime))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let kind = a[1].clone();
    let threads: usize = a[2].parse().unwrap();
    let n: usize = a[3].parse().unwrap();
    let files: Vec<(String, String)> = (a[4..].iter())
        .map(|p| {
            let name = std::path::Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, std::fs::read_to_string(p).unwrap())
        })
        .collect();
    let public = std::env::var("PUBLIC").ok();
    let body = std::env::var("BODY")
        .map(|p| std::fs::read(p).unwrap())
        .unwrap_or_default();
    let server = match Server::new(files, Limits::web()).unwrap() {
        Ok(s) => s,
        Err(d) => panic!("{d:?}"),
    };
    let req = || match kind.as_str() {
        "bookings" => Request {
            method: "POST".into(),
            target: "/bookings".into(),
            header_names: vec!["host".into(), "content-type".into()],
            header_values: vec!["localhost".into(), "application/json".into()],
            body: body.clone(),
        },
        _ => Request {
            method: "GET".into(),
            target: "/hello/kio".into(),
            header_names: vec!["host".into()],
            header_values: vec!["localhost".into()],
            body: vec![],
        },
    };
    // Opened once, cloned for each request, as `--listen` does.
    let public = public.map(|p| open_dir(std::path::Path::new(&p)).unwrap());
    let dirs = || match &public {
        Some(d) => vec![("public".to_string(), d.try_clone().unwrap())],
        None => vec![],
    };
    let run = |count: usize| {
        for _ in 0..count {
            let r = server.handle(req(), dirs()).unwrap();
            assert!(r.status < 300, "status {}", r.status);
        }
    };
    let on_threads = |count: usize| {
        std::thread::scope(|sc| {
            for _ in 0..threads {
                std::thread::Builder::new()
                    .stack_size(RUN_STACK)
                    .spawn_scoped(sc, || run(count))
                    .unwrap();
            }
        })
    };
    on_threads(2000 / threads + 1);
    let per = n / threads;
    let total = (per * threads) as f64;
    let ((u0, s0), t0) = (cpu(), Instant::now());
    on_threads(per);
    let ((u1, s1), w) = (cpu(), t0.elapsed().as_secs_f64());
    // One line, key=value, for bench.py.
    println!(
        "threads={threads} n={} cpu_us={:.2} sys_us={:.2} wall_us={:.3} rps={:.0}",
        per * threads,
        ((u1 - u0) + (s1 - s0)) / total * 1e6,
        (s1 - s0) / total * 1e6,
        w / total * 1e6,
        total / w
    );
}
