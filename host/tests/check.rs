use kio_host::{Kio, Limits};
use lang::{check, CapTable};

#[test]
fn summarize_reports_two_lines() {
    let src = std::fs::read_to_string("../examples/summarize.kio").unwrap();
    let mut kio = Kio::new(Limits::default()).unwrap();
    let report = kio.check(&src).unwrap().unwrap();
    let want = lang::report(&check(&src, &CapTable::v0()).unwrap());
    assert_eq!(report.lines.len(), 2);
    assert_eq!(report.lines.join("\n"), want);
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn unknown_name_matches_lang_diag() {
    let src = "fn main() -> Int {\n    nope\n}\n";
    let mut kio = Kio::new(Limits::default()).unwrap();
    let got = kio.check(src).unwrap().unwrap_err();
    let want = check(src, &CapTable::v0()).unwrap_err();
    assert_eq!(got.len(), want.len());
    let (g, w) = (&got[0], &want[0]);
    assert_eq!((g.start as usize, g.end as usize), (w.span.start, w.span.end));
    assert_eq!(g.message, w.msg);
    assert_eq!(g.hint, w.hint);
    assert!(g.hint.is_some());
    assert_eq!(kio.import_calls(), 0);
}

/// No clock, randomness or environment reaches the guest, so fresh instances agree exactly.
#[test]
fn check_is_deterministic_across_instances() {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let run = || Kio::new(Limits::default()).unwrap().check(&src).unwrap().unwrap().lines;
    assert_eq!(run(), run());
}

/// A temp directory with `files`, opened as a grant.
fn dir(files: &[(&str, &[u8])]) -> (tempfile::TempDir, cap_std::fs::Dir) {
    let t = tempfile::tempdir().unwrap();
    for (p, b) in files {
        std::fs::write(t.path().join(p), b).unwrap();
    }
    let d = kio_host::open_dir(t.path()).unwrap();
    (t, d)
}

fn invoices() -> String {
    std::fs::read_to_string("../examples/invoices.kio").unwrap()
}

#[test]
fn invoices_writes_total() {
    let (_i, input) = dir(&[("invoices.csv", b"a,40\nb,2\nbad\n")]);
    let (out_t, out) = dir(&[]);
    let mut kio = Kio::new(Limits::default()).unwrap();
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    let out = kio.run(&invoices(), grants).unwrap().unwrap();
    assert_eq!(out.report.lines.len(), 4);
    assert_eq!(out.value, "Ok(())");
    assert!(!out.failed);
    assert_eq!(std::fs::read(out_t.path().join("total.txt")).unwrap(), b"42");
    assert_eq!(kio.import_calls(), 2);
}

#[test]
fn summarize_writes_summary() {
    let src = std::fs::read_to_string("../examples/summarize.kio").unwrap();
    let (_i, input) = dir(&[("invoices.csv", b"12345")]);
    let (out_t, out) = dir(&[]);
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    Kio::new(Limits::default()).unwrap().run(&src, grants).unwrap().unwrap();
    assert_eq!(std::fs::read(out_t.path().join("summary.csv")).unwrap(), b"5");
}

/// Run `main(d: Dir)` as `body` on `d` and return the printed value.
fn on(d: cap_std::fs::Dir, body: &str) -> String {
    let src = format!(
        "fn main(d: Dir) -> Result<{}, IoErr> {{ {body} }}",
        if body.contains("create") { "()" } else { "Bytes" }
    );
    Kio::new(Limits::default())
        .unwrap()
        .run(&src, vec![("d".into(), d)])
        .unwrap()
        .unwrap()
        .value
}

const NOT_FOUND: &str = "IoErr::NotFound";
const DENIED: &str = "IoErr::Denied";
const INVALID: &str = "IoErr::InvalidPath";
const EXISTS: &str = "IoErr::Exists";

#[test]
fn read_errors() {
    let (t, d) = dir(&[("a", b"x")]);
    std::fs::write(t.path().join("secret"), "s").unwrap();
    std::fs::create_dir_all(t.path().join("sub/in")).unwrap();
    let read = |p: &str| {
        let d = kio_host::open_dir(&t.path().join("sub")).unwrap();
        on(d, &format!("d.read(\"{p}\")"))
    };
    assert_eq!(on(d, "d.read(\"missing\")"), NOT_FOUND);
    assert_eq!(read("../secret"), INVALID);
    assert_eq!(read("/etc/passwd"), INVALID);
    assert_eq!(read("in/../../secret"), INVALID);
    assert_eq!(read(""), INVALID);
    assert_eq!(read("a\u{0}b"), INVALID);
    assert_eq!(read("no/such/file"), NOT_FOUND);
}

#[test]
fn symlinks_out_are_refused() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join("secret"), "s").unwrap();
    std::fs::create_dir(outer.path().join("outside")).unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    use std::os::unix::fs::symlink;
    symlink(outer.path().join("secret"), root.join("file")).unwrap();
    symlink(outer.path().join("outside"), root.join("dir")).unwrap();
    symlink("../secret", root.join("rel")).unwrap();
    symlink("/etc/passwd", root.join("abs")).unwrap();
    let d = || kio_host::open_dir(&root).unwrap();
    for p in ["file", "rel", "abs", "dir/x"] {
        assert_eq!(on(d(), &format!("d.read(\"{p}\")")), INVALID, "{p}");
    }
    assert_eq!(on(d(), "d.create(\"dir/new\", \"x\".to_bytes())"), INVALID);
    assert_eq!(on(d(), "d.create(\"file\", \"x\".to_bytes())"), EXISTS);
    assert_eq!(std::fs::read(outer.path().join("secret")).unwrap(), b"s");
    assert_eq!(std::fs::read_dir(outer.path().join("outside")).unwrap().count(), 0);
}

#[test]
fn create_errors_leave_filesystem_unchanged() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("old"), "keep").unwrap();
    std::fs::create_dir(root.join("in")).unwrap();
    let d = || kio_host::open_dir(&root).unwrap();
    let ok = "Ok(())";
    assert_eq!(on(d(), "d.create(\"old\", \"new\".to_bytes())"), EXISTS);
    assert_eq!(std::fs::read(root.join("old")).unwrap(), b"keep");
    for p in ["../x", "/tmp/kio-x", "", "in/../../x"] {
        assert_eq!(on(d(), &format!("d.create(\"{p}\", \"x\".to_bytes())")), INVALID, "{p}");
    }
    assert_eq!(on(d(), "d.create(\"nodir/x\", \"x\".to_bytes())"), NOT_FOUND);
    assert!(!outer.path().join("x").exists() && !root.join("nodir").exists());
    assert_eq!(on(d(), "d.create(\"new\", \"x\".to_bytes())"), ok);
    assert_eq!(std::fs::read(root.join("new")).unwrap(), b"x");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
}

#[test]
fn open_dir_refuses_missing_and_files() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("f"), "x").unwrap();
    assert!(kio_host::open_dir(&t.path().join("nope")).is_err());
    assert!(kio_host::open_dir(&t.path().join("f")).is_err());
}

fn rejected(names: &[&str], message: &str, hint: &str) {
    let ts: Vec<_> = names.iter().map(|_| dir(&[])).collect();
    let grants = names.iter().zip(ts).map(|(n, (_t, d))| (n.to_string(), d)).collect();
    let mut kio = Kio::new(Limits::default()).unwrap();
    let diags = kio.run(&invoices(), grants).unwrap().unwrap_err();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, message);
    assert!(diags[0].hint.as_deref().unwrap().contains(hint));
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn duplicate_grant() {
    rejected(
        &["input", "out", "out"],
        "grant `out` is given more than once",
        "one name",
    );
}

#[test]
fn extra_grant() {
    rejected(
        &["input", "out", "extra"],
        "grant `extra` is not a parameter of `main`",
        "input, out",
    );
}

#[test]
fn missing_grant_is_no_grant_error() {
    let (_t, d) = dir(&[]);
    let mut kio = Kio::new(Limits::default()).unwrap();
    let diags = kio.run(&invoices(), vec![("input".into(), d)]).unwrap().unwrap_err();
    assert_eq!(diags[0].message, "no grant for `out: Dir`");
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn runtime_error_has_span() {
    let src = "fn main(d: Dir) -> Int {\n    let xs: List<Int> = [];\n    xs[1]\n}\n";
    let (_t, d) = dir(&[]);
    let diags = Kio::new(Limits::default())
        .unwrap()
        .run(src, vec![("d".into(), d)])
        .unwrap()
        .unwrap_err();
    assert_eq!(diags.len(), 1);
    assert!(diags[0].end > diags[0].start);
    assert!(diags[0].hint.is_none());
}

/// G4: on the default stack the deepest recursion stops with the depth error, never a trap,
/// in debug and release builds of the host.
#[test]
fn depth_budget_fits_the_default_stack() {
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { down(n - 1) } } fn main() -> Int { down(100000) }";
    let diags = Kio::new(Limits::default())
        .unwrap()
        .run(src, vec![])
        .unwrap()
        .unwrap_err();
    assert_eq!(diags[0].message, "evaluation is too deep");
}

#[test]
fn small_stack_traps_with_stack_overflow() {
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { down(n - 1) } } fn main() -> Int { down(1000) }";
    let err = Kio::new(Limits {
        stack: 64 << 10,
        ..Limits::default()
    })
    .unwrap()
    .run(src, vec![])
    .unwrap_err();
    assert_eq!(
        err.downcast_ref::<wasmtime::Trap>(),
        Some(&wasmtime::Trap::StackOverflow)
    );
}

/// `main` returning `Err` is a failed run: the host exits nonzero and still shows the value.
#[test]
fn main_err_marks_the_run_failed() {
    let (_i, input) = dir(&[("invoices.csv", b"a,1\n")]);
    let (_o, out) = dir(&[("total.txt", b"old")]);
    let grants = vec![("input".to_string(), input), ("out".to_string(), out)];
    let out = Kio::new(Limits::default())
        .unwrap()
        .run(&invoices(), grants)
        .unwrap()
        .unwrap();
    assert!(out.failed);
    assert_eq!(out.value, EXISTS);
}

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let limits = Limits {
        fuel: 10_000_000,
        ..Limits::default()
    };
    let src = "fn main() -> Int { while true {} 0 }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "out of fuel (limit 10000000; raise it with --fuel)"
    );
}

/// H5: a read that blocks forever (the `pipe` test hook) stops at the deadline, not after it.
#[cfg(unix)]
#[test]
fn blocked_host_call_times_out() {
    let (_t, d) = dir(&[("a", b"x")]);
    let limits = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let src = "fn main(d: Dir) -> Int { let r = d.read(\"pipe\"); 0 }";
    let mut kio = Kio::new(limits).unwrap();
    let start = std::time::Instant::now();
    let err = kio.run(src, vec![("d".into(), d)]).unwrap_err();
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(
        kio.explain(&err).unwrap(),
        "timed out after 1 s (raise it with --timeout)"
    );
}

/// H5: with the deadline in force, a read that does not block is unaffected.
#[test]
fn quick_host_call_is_unaffected_by_deadline() {
    let limits = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let (_t, d) = dir(&[("a", b"x")]);
    let src = "fn main(d: Dir) -> Result<Bytes, IoErr> { d.read(\"a\") }";
    let out = Kio::new(limits)
        .unwrap()
        .run(src, vec![("d".into(), d)])
        .unwrap()
        .unwrap();
    assert!(!out.failed);
}

/// H3: a program that allocates without bound stops at the memory limit with a message naming it.
#[test]
fn unbounded_allocation_runs_out_of_memory() {
    let limits = Limits {
        memory: 16 << 20,
        ..Limits::default()
    };
    let src = "fn main() -> Int { let mut xs: List<Str> = []; while true { xs.push(\"0123456789abcdef\"); } 0 }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "out of memory (limit 16 MB; raise it with --memory)"
    );
    assert!(kio.peak_memory() <= 16 << 20);
}

/// H3: recursion past the Wasm stack stops with a message naming the stack limit.
#[test]
fn deep_recursion_overflows_a_small_stack() {
    let limits = Limits {
        stack: 64 << 10,
        ..Limits::default()
    };
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { down(n - 1) } } fn main() -> Int { down(100000) }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "stack overflow (limit 64 KB; raise it with --stack)"
    );
}

/// Raising `--stack` above the 2 MB default lets deeper expressions run (once refused by Wasmtime).
#[test]
fn a_larger_stack_runs_deeper() {
    // The host checks the program first, natively; give that recursion room too.
    let t = std::thread::Builder::new().stack_size(64 << 20);
    t.spawn(larger_stack).unwrap().join().unwrap();
}

fn larger_stack() {
    let deep = format!(
        "fn d(n: Int) -> Int {{ if n == 0 {{ 0 }} else {{ {}d(n - 1) }} }} fn main() -> Int {{ d(600) }}",
        "- ".repeat(58)
    );
    let mut small = Kio::new(Limits::default()).unwrap();
    let err = small.run(&deep, vec![]).unwrap_err();
    assert!(small.explain(&err).unwrap().starts_with("stack overflow"));
    let limits = Limits {
        stack: kio_host::MAX_STACK,
        ..Limits::default()
    };
    Kio::new(limits).unwrap().run(&deep, vec![]).unwrap().unwrap();
}

/// H4: the call past `--calls` stops the run.
#[test]
fn too_many_calls_stops_the_run() {
    let limits = Limits {
        calls: 3,
        ..Limits::default()
    };
    let (_t, d) = dir(&[("a", b"x")]);
    let src = "fn main(d: Dir) -> Int { let mut i = 0; while i < 10 { let r = d.read(\"a\"); i = i + 1; } i }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![("d".into(), d)]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "too many host calls (limit 3; raise it with --calls)"
    );
    assert_eq!(kio.import_calls(), 4);
}

/// H4: reads and writes share one byte budget.
#[test]
fn too_much_io_stops_the_run() {
    let limits = Limits {
        io: 1 << 20,
        ..Limits::default()
    };
    let big = vec![b'x'; 600 << 10];
    let (_i, input) = dir(&[("big", &big)]);
    let (out_t, out) = dir(&[]);
    // 600 KB read, then 600 KB written: only the sum passes 1 MB.
    let src = "fn main(i: Dir, o: Dir) -> Result<(), IoErr> { let b = i.read(\"big\")?; o.create(\"copy\", b) }";
    let mut kio = Kio::new(limits).unwrap();
    let err = kio.run(src, vec![("i".into(), input), ("o".into(), out)]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "too much I/O (limit 1 MB; raise it with --io)"
    );
    assert!(!out_t.path().join("copy").exists());
}

/// H4: a file over 10 MB is `too-large` to the program, which can handle it.
#[test]
fn oversized_read_is_too_large() {
    let (_t, d) = dir(&[("big", &vec![b'x'; (10 << 20) + 1]), ("ok", &vec![b'x'; 10 << 20])]);
    let src = "fn main(d: Dir) -> Int { \
        let big = match d.read(\"big\") { Ok(_) => 0, Err(TooLarge) => 1, Err(_) => 2 }; \
        let ok = match d.read(\"ok\") { Ok(b) => 10, Err(_) => 20 }; \
        big + ok }";
    let limits = Limits {
        io: 100 << 20,
        ..Limits::default()
    };
    let out = Kio::new(limits)
        .unwrap()
        .run(src, vec![("d".into(), d)])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, "11");
}

/// H4: a write that fails after the file is created leaves no file, so a retry works.
#[test]
fn failed_write_removes_the_file() {
    let (t, d) = dir(&[]);
    let src = "fn main(d: Dir) -> Result<(), IoErr> { d.create(\"f\", \"x\".to_bytes()) }";
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.fail_writes();
    let out = kio.run(src, vec![("d".into(), d)]).unwrap().unwrap();
    assert_eq!(out.value, DENIED);
    assert!(!t.path().join("f").exists());
}

/// H6: a huge `--timeout` is capped, never a panic.
#[test]
fn huge_timeout_does_not_panic() {
    let limits = Limits {
        timeout_secs: u64::MAX,
        ..Limits::default()
    };
    let out = Kio::new(limits).unwrap().run("fn main() -> Int { 1 }", vec![]).unwrap();
    assert_eq!(out.unwrap().value, "1");
}

/// H6: a path longer than the host accepts is refused before any work, so it costs no host memory.
#[test]
fn overlong_path_is_invalid() {
    let (_t, d) = dir(&[]);
    let src = "fn main(d: Dir) -> Result<Bytes, IoErr> { let mut s = \"a/\"; let mut i = 0; while i < 12 { s = s + s; i = i + 1; } d.read(s) }";
    let out = Kio::new(Limits::default())
        .unwrap()
        .run(src, vec![("d".into(), d)])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, "IoErr::InvalidPath");
}

/// A one-thread HTTP/1.1 server on 127.0.0.1: `/ok` is 200 `hello`, `/big` is 10 MB + 1, `/hang`
/// never answers, `/moved` redirects off-host, `/same` to `/ok`, `/port` and `/https` to another
/// port and scheme, `/bare` redirects with no `Location`, `/hopN` redirects N times then `/ok`,
/// anything else is 404. Returns the grant string.
fn serve() -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let grant = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut lines = BufReader::new(&stream).lines().map(Result::unwrap);
                let path = lines.next().unwrap().split(' ').nth(1).unwrap().to_string();
                while lines.next().is_some_and(|l| !l.is_empty()) {}
                let (head, body): (String, Vec<u8>) = match path.as_str() {
                    "/ok" => ("200 OK".into(), b"hello".to_vec()),
                    "/big" => ("200 OK".into(), vec![b'x'; (10 << 20) + 1]),
                    "/moved" => ("302 Found\r\nLocation: http://evil.com/x".into(), vec![]),
                    "/same" => ("302 Found\r\nLocation: /ok".into(), vec![]),
                    "/port" => ("302 Found\r\nLocation: http://127.0.0.1:1/ok".into(), vec![]),
                    "/https" => ("302 Found\r\nLocation: https://127.0.0.1/ok".into(), vec![]),
                    "/bare" => ("302 Found".into(), vec![]),
                    p if p.starts_with("/hop") => {
                        let n: u32 = p[4..].parse().unwrap();
                        let to = if n == 0 {
                            "/ok".to_string()
                        } else {
                            format!("/hop{}", n - 1)
                        };
                        (format!("307 Temporary Redirect\r\nLocation: {to}"), vec![])
                    }
                    "/hang" => loop {
                        std::thread::sleep(std::time::Duration::from_secs(60));
                    },
                    _ => ("404 Not Found".into(), vec![]),
                };
                let mut out = &stream;
                let _ = write!(
                    out,
                    "HTTP/1.1 {head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = out.write_all(&body);
            });
        }
    });
    grant
}

/// Run `main(h: Http)` with `body` on the server at `grant`; the printed value.
fn get(grant: &str, body: &str, limits: Limits) -> String {
    let src = format!("fn main(h: Http) -> Result<Bytes, HttpErr> {{ {body} }}");
    let origin = kio_host::Origin::parse(grant).unwrap();
    let mut kio = Kio::new(limits).unwrap();
    kio.run_with(&src, vec![], vec![("h".into(), origin)])
        .unwrap()
        .unwrap()
        .value
}

const HTTP_DENIED: &str = "HttpErr::Denied";

/// N1: a granted GET returns the body, a 404 is `status(404)`.
#[test]
fn http_get_and_status() {
    let g = serve();
    let l = Limits::default();
    assert_eq!(get(&g, "h.get(\"/ok\")", l), "Ok(<5 bytes>)");
    let status = |n: i64| format!("HttpErr::Status({n})");
    assert_eq!(get(&g, "h.get(\"/nope\")", l), status(404));
}

/// N2: a redirect within the origin is followed, up to 5 hops; any other host, port or scheme, a
/// sixth hop and a redirect with no `Location` are `denied`. Each hop is a counted call.
#[test]
fn http_redirects() {
    let g = serve();
    let l = Limits::default();
    let ok = "Ok(<5 bytes>)";
    assert_eq!(get(&g, "h.get(\"/same\")", l), ok);
    assert_eq!(get(&g, "h.get(\"/hop4\")", l), ok); // 5 redirects
    for p in ["/moved", "/port", "/https", "/bare", "/hop5"] {
        assert_eq!(get(&g, &format!("h.get(\"{p}\")"), l), HTTP_DENIED, "{p}");
    }
    let calls = |path: &str| {
        let src = format!("fn main(h: Http) -> Int {{ let r = h.get(\"{path}\"); 0 }}");
        let mut kio = Kio::new(l).unwrap();
        let origin = kio_host::Origin::parse(&g).unwrap();
        kio.run_with(&src, vec![], vec![("h".into(), origin)]).unwrap().unwrap();
        kio.import_calls()
    };
    assert_eq!(calls("/hop2"), calls("/ok") + 3); // /hop2 redirects 3 times
}

/// N2: a named host is resolved once per run, however many requests and hops; one that resolves
/// only to loopback is refused unless the grant is an IP literal.
#[test]
fn http_dns_is_pinned_and_filtered() {
    let g = serve().replace("127.0.0.1", "localhost");
    let src = "fn main(h: Http) -> Int { let a = h.get(\"/ok\"); let b = h.get(\"/same\"); 0 }";
    let run = |origin: kio_host::Origin| {
        Kio::new(Limits::default())
            .unwrap()
            .run_with(src, vec![], vec![("h".into(), origin)])
            .unwrap()
            .unwrap();
    };
    let mut origin = kio_host::Origin::parse(&g).unwrap();
    origin.allow_private_addrs();
    run(origin.clone());
    assert_eq!(origin.lookups(), 1);
    // Without the allowance, `localhost` has no usable address: denied, and still looked up once.
    let origin = kio_host::Origin::parse(&g).unwrap();
    run(origin.clone());
    assert_eq!(origin.lookups(), 1);
    assert_eq!(get(&g, "h.get(\"/ok\")", Limits::default()), HTTP_DENIED);
}

/// N1: a path cannot name another host, scheme, authority or header; none of these reach the network.
#[test]
fn http_paths_are_denied() {
    let g = serve();
    for p in [
        "//evil.com/x",
        "http://evil.com",
        "ok",
        "",
        "/a\\\\b",
        "/a#b",
        "/a b",
        "/a\r\nHost: evil",
        "/a\u{0}b",
    ] {
        let body = format!("h.get(\"{p}\")");
        assert_eq!(get(&g, &body, Limits::default()), HTTP_DENIED, "{p:?}");
    }
}

/// N1: the call is counted, the body is charged to `--io`, and a server that never answers
/// stops the run at the deadline.
#[test]
fn http_quotas_and_timeout() {
    let g = serve();
    let src = "fn main(h: Http) -> Int { let r = h.get(\"/ok\"); let s = h.get(\"/ok\"); 0 }";
    let run = |limits| {
        let mut kio = Kio::new(limits).unwrap();
        let o = kio_host::Origin::parse(&g).unwrap();
        let err = kio.run_with(src, vec![], vec![("h".into(), o)]);
        (kio, err)
    };
    let (kio, r) = run(Limits::default());
    r.unwrap().unwrap();
    assert_eq!(kio.import_calls(), 2);
    let (kio, r) = run(Limits {
        calls: 1,
        ..Limits::default()
    });
    assert_eq!(
        kio.explain(&r.unwrap_err()).unwrap(),
        "too many host calls (limit 1; raise it with --calls)"
    );
    let (kio, r) = run(Limits {
        io: 9,
        ..Limits::default()
    });
    assert_eq!(
        kio.explain(&r.unwrap_err()).unwrap(),
        "too much I/O (limit 0 MB; raise it with --io)"
    );

    let limits = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let src = "fn main(h: Http) -> Int { let r = h.get(\"/hang\"); 0 }";
    let mut kio = Kio::new(limits).unwrap();
    let o = kio_host::Origin::parse(&g).unwrap();
    let start = std::time::Instant::now();
    // The request timeout and the deadline coincide: either stops the run, or `timeout` reaches the program.
    match kio.run_with(src, vec![], vec![("h".into(), o)]) {
        Err(e) => assert_eq!(
            kio.explain(&e).unwrap(),
            "timed out after 1 s (raise it with --timeout)"
        ),
        Ok(out) => assert_eq!(out.unwrap().value, "0"),
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

/// N1: a body over 10 MB is `too-large`, and what was read is charged.
#[test]
fn http_oversized_body_is_too_large() {
    let g = serve();
    let l = Limits::default();
    assert_eq!(get(&g, "h.get(\"/big\")", l), "HttpErr::TooLarge");
}

/// N1: a request that exceeds the per-request timeout (shorter than the run deadline) is `timeout`.
#[test]
fn http_slow_server_is_timeout() {
    let g = serve();
    let l = Limits {
        timeout_secs: 30,
        ..Limits::default()
    };
    // The 30 s run deadline is far off, so only the 10 s request timeout can produce `timeout`.
    assert_eq!(get(&g, "h.get(\"/hang\")", l), "HttpErr::Timeout");
}

/// N1: grants are strict, as for `Dir`.
#[test]
fn http_grant_rules() {
    let o = || kio_host::Origin::parse("http://127.0.0.1:1").unwrap();
    let src = "fn main(h: Http) -> Int { 1 }";
    let run = |names: &[&str]| {
        let grants = names.iter().map(|n| (n.to_string(), o())).collect();
        Kio::new(Limits::default())
            .unwrap()
            .run_with(src, vec![], grants)
            .unwrap()
            .unwrap_err()
    };
    assert_eq!(run(&["h", "h"])[0].message, "grant `h` is given more than once");
    assert_eq!(run(&["h", "x"])[0].message, "grant `x` is not a parameter of `main`");
    assert_eq!(run(&[])[0].message, "no grant for `h: Http`");
}

/// N1: grant syntax.
#[test]
fn origin_parse() {
    for ok in [
        "example.com",
        "https://example.com:8443",
        "http://127.0.0.1:8080",
        "http://[::1]:80",
        "127.0.0.1",
    ] {
        assert!(kio_host::Origin::parse(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "ftp://x",
        "http://",
        "http://a/b",
        "a@b",
        "a b",
        "a:0",
        "a:99999",
        "a:",
        "::1",
        "http://a?x",
        "a#b",
        "https://a:b",
        "http://a\\b",
        "[::1",
        "[zz]:80",
    ] {
        assert!(kio_host::Origin::parse(bad).is_err(), "{bad}");
    }
}

/// W11b: a FIFO is not a regular file; reading it is `denied` at once, never blocked.
#[cfg(unix)]
#[test]
fn fifo_read_is_denied_at_once() {
    let (t, d) = dir(&[]);
    assert!(std::process::Command::new("mkfifo")
        .arg(t.path().join("p"))
        .status()
        .unwrap()
        .success());
    let start = std::time::Instant::now();
    assert_eq!(on(d, "d.read(\"p\")"), DENIED);
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}
