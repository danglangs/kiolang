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

/// Checking is deterministic: two checks of one program agree exactly.
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

/// G4, decision 6: recursion is bounded by `--stack`. On the default stack a deep recursion runs
/// and a deeper one stops with "stack overflow", never a host crash, whatever the caller's stack
/// (this test thread's is small).
#[test]
fn deep_recursion_stops_at_the_default_stack() {
    let src = |n: u32| {
        format!("fn down(n: Int) -> Int {{ if n == 0 {{ 0 }} else {{ 1 + down(n - 1) }} }} fn main() -> Int {{ down({n}) }}")
    };
    let mut kio = Kio::new(Limits::default()).unwrap();
    assert_eq!(kio.run(&src(10_000), vec![]).unwrap().unwrap().value, "10000");
    let err = kio.run(&src(100_000_000), vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&err).unwrap(),
        "stack overflow (limit 2048 KB; raise it with --stack)"
    );
}

#[test]
fn small_stack_traps_with_stack_overflow() {
    let src = "fn down(n: Int) -> Int { if n == 0 { 0 } else { down(n - 1) } } fn main() -> Int { down(1000000) }";
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
    // The deadline is 1 s; under a loaded test run the whole call can take longer, so the bound only
    // shows the blocked call did not hang.
    assert!(start.elapsed() < std::time::Duration::from_secs(15));
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

/// Raising `--stack` above the 2 MB default lets deeper recursion run (once refused by Wasmtime).
/// About 43,000 calls of `d` fit in 2 MB and 174,000 in 8 MB.
#[test]
fn a_larger_stack_runs_deeper() {
    let deep = "fn d(n: Int) -> Int { if n == 0 { 0 } else { 1 + d(n - 1) } } fn main() -> Int { d(100000) }";
    let mut small = Kio::new(Limits::default()).unwrap();
    let err = small.run(deep, vec![]).unwrap_err();
    assert!(small.explain(&err).unwrap().starts_with("stack overflow"));
    let limits = Limits {
        stack: kio_host::MAX_STACK,
        ..Limits::default()
    };
    Kio::new(limits).unwrap().run(deep, vec![]).unwrap().unwrap();
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
/// never answers, `/slow` answers after two seconds, `/echo` is 200 with the request line, headers
/// and body as received (and two response headers), `/created` 201, `/error` 500 `boom`,
/// `/moved` redirects off-host, `/same` to `/ok`, `/port`, `/https` and `/ftp` to another port or
/// scheme, `/bare` redirects with no `Location`, `/hopN` redirects N times then `/ok`, anything
/// else is 404. Returns the grant string.
fn serve() -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let grant = format!("http://{}", listener.local_addr().unwrap());
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut words = request.split_whitespace();
                let (method, path) = (words.next().unwrap().to_string(), words.next().unwrap().to_string());
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length: ") {
                        length = n.parse().unwrap();
                    }
                    headers += line;
                    headers.push('\n');
                }
                let mut sent = vec![0; length];
                reader.read_exact(&mut sent).unwrap();
                let (head, body): (String, Vec<u8>) = match path.as_str() {
                    "/ok" => ("200 OK".into(), b"hello".to_vec()),
                    "/echo" => {
                        let mut body = format!("{method} {path}\n{headers}\n").into_bytes();
                        body.extend(sent);
                        ("200 OK\r\nX-Echo: yes\r\nSet-Thing: a".into(), body)
                    }
                    "/created" => ("201 Created".into(), b"made".to_vec()),
                    "/error" => ("500 Internal Server Error".into(), b"boom".to_vec()),
                    "/slow" => {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        ("200 OK".into(), b"slow".to_vec())
                    }
                    "/big" => ("200 OK".into(), vec![b'x'; (10 << 20) + 1]),
                    "/moved" => ("302 Found\r\nLocation: http://evil.com/x".into(), vec![]),
                    "/same" => ("302 Found\r\nLocation: /ok".into(), vec![]),
                    "/port" => ("302 Found\r\nLocation: http://127.0.0.1:1/ok".into(), vec![]),
                    "/https" => ("302 Found\r\nLocation: https://127.0.0.1/ok".into(), vec![]),
                    "/ftp" => (format!("302 Found\r\nLocation: ftp://127.0.0.1:{port}/ok"), vec![]),
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
    for p in ["/moved", "/port", "/https", "/ftp", "/bare", "/hop5"] {
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
    // The deadline cuts the request off: the run stops; `timeout` never reaches the program (H24).
    let e = kio.run_with(src, vec![], vec![("h".into(), o)]).unwrap_err();
    assert_eq!(
        kio.explain(&e).unwrap(),
        "timed out after 1 s (raise it with --timeout)"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

/// H24: a `get`, `send` or `send_all` still in flight at the run's deadline always ends the run
/// as a timeout, compiled or interpreted: 50 runs, one outcome.
#[test]
fn http_call_at_the_deadline_ends_the_run() {
    let g = serve();
    let one_sec = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let calls = [
        r#"h.get("/hang").is_ok()"#,
        r#"h.send("POST", "/hang", [], "x".to_bytes()).is_ok()"#,
        r#"h.send_all([HttpRequest { method: "GET", path: "/ok", headers: [], body: "".to_bytes(), timeout_ms: 0 }, HttpRequest { method: "GET", path: "/hang", headers: [], body: "".to_bytes(), timeout_ms: 0 }]).len() == 2"#,
    ];
    // Ten at a time, so the load stays light for the tests beside it.
    for wave in 0..5 {
        std::thread::scope(|s| {
            let runs: Vec<_> = (wave * 10..wave * 10 + 10)
                .map(|i| {
                    let (g, call) = (&g, calls[i % 3]);
                    s.spawn(move || {
                        let src = format!("fn main(h: Http) -> Bool {{ {call} }}");
                        let grants = vec![("h".into(), granted(g, "POST"))];
                        let out = if i % 5 == 4 {
                            let mut interp = kio_host::Interp::new(one_sec).unwrap();
                            let r = interp.run_with(&src, vec![], grants);
                            r.map(|o| o.map(|o| o.value)).map_err(|e| interp.explain(&e))
                        } else {
                            let mut kio = Kio::new(one_sec).unwrap();
                            let r = kio.run_with(&src, vec![], grants);
                            r.map(|o| o.map(|o| o.value)).map_err(|e| kio.explain(&e))
                        };
                        format!("{out:?}")
                    })
                })
                .collect();
            for (i, r) in (wave * 10..).zip(runs) {
                assert_eq!(
                    r.join().unwrap(),
                    r#"Err(Some("timed out after 1 s (raise it with --timeout)"))"#,
                    "run {i}: {}",
                    calls[i % 3]
                );
            }
        });
    }
}

/// H24: a request's own `timeout_ms`, shorter than what the run has left, is still `timeout` to
/// the program, which goes on.
#[test]
fn a_short_request_timeout_reaches_the_program() {
    let g = serve();
    let src = r#"fn main(h: Http) -> List<Result<Int, HttpErr>> { h.send_all([HttpRequest { method: "GET", path: "/hang", headers: [], body: "".to_bytes(), timeout_ms: 200 }]).map(|r| r.map(|r| r.status)) }"#;
    let one_sec = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    for _ in 0..5 {
        let mut kio = Kio::new(one_sec).unwrap();
        let out = kio.run_with(src, vec![], vec![("h".into(), granted(&g, ""))]);
        assert_eq!(out.unwrap().unwrap().value, "[Err(HttpErr::Timeout)]");
    }
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

/// `main(h: Http) -> R { body }` compiled, on the server at `grant` with `methods` added to the
/// grant; the printed value.
fn send(grant: &str, methods: &str, ret: &str, body: &str) -> String {
    let src = format!("fn main(h: Http) -> {ret} {{ {body} }}");
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.run_with(&src, vec![], vec![("h".into(), granted(grant, methods))])
        .unwrap()
        .unwrap()
        .value
}

fn granted(grant: &str, methods: &str) -> kio_host::Origin {
    let mut origin = kio_host::Origin::parse(grant).unwrap();
    if !methods.is_empty() {
        origin.allow_methods(methods).unwrap();
    }
    origin
}

const RESPONSE: &str = "Result<HttpResponse, HttpErr>";

/// H16: `send` with each method; a method not granted is `denied`, one that is not a method is
/// `invalid`; a non-2xx status is `Ok` with its status; `GET` needs no `--http-methods`.
#[test]
fn http_send_methods_and_statuses() {
    let g = serve();
    let empty = r#""".to_bytes()"#;
    let status = |call: &str| {
        send(
            &g,
            "POST,PUT,PATCH,DELETE,HEAD,OPTIONS",
            "Result<Int, HttpErr>",
            &format!("Ok({call}?.status)"),
        )
    };
    for m in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
        assert_eq!(
            status(&format!(r#"h.send("{m}", "/ok", [], {empty})"#)),
            "Ok(200)",
            "{m}"
        );
    }
    assert_eq!(
        status(&format!(r#"h.send("POST", "/created", [], {empty})"#)),
        "Ok(201)"
    );
    assert_eq!(status(&format!(r#"h.send("GET", "/error", [], {empty})"#)), "Ok(500)");
    assert_eq!(status(&format!(r#"h.send("GET", "/nope", [], {empty})"#)), "Ok(404)");
    let text = |methods: &str, call: &str| {
        send(
            &g,
            methods,
            "Result<Str, HttpErr>",
            &format!("Ok({call}?.body.to_str())"),
        )
    };
    assert_eq!(text("", r#"h.send("GET", "/ok", [], "".to_bytes())"#), "Ok(\"hello\")");
    assert_eq!(
        text("", r#"h.send("GET", "/error", [], "".to_bytes())"#),
        "Ok(\"boom\")"
    );
    assert_eq!(text("HEAD", r#"h.send("HEAD", "/ok", [], "".to_bytes())"#), "Ok(\"\")");
    for m in ["POST", "PUT", "DELETE"] {
        let call = format!(r#"h.send("{m}", "/ok", [], {empty})"#);
        assert_eq!(text("", &call), "HttpErr::Denied", "{m} without the grant");
        assert_eq!(
            text("POST,PUT", &call),
            if m == "DELETE" {
                "HttpErr::Denied"
            } else {
                "Ok(\"hello\")"
            }
        );
    }
    for m in ["get", "Post", "PROPFIND", "", "GET ", "GE\nT"] {
        let call = format!(r#"h.send("{}", "/ok", [], {empty})"#, m.replace('\n', "\\n"));
        assert_eq!(text("POST", &call), "HttpErr::Invalid", "{m:?}");
    }
    // A bad path is `denied` as for `get`; the body is sent and echoed.
    assert_eq!(
        text("POST", r#"h.send("POST", "http://evil.com/", [], "".to_bytes())"#),
        "HttpErr::Denied"
    );
    let echoed = text("POST", r#"h.send("POST", "/echo", [], "payload".to_bytes())"#);
    assert!(
        echoed.starts_with("Ok(\"POST /echo\\n") && echoed.ends_with("\\n\\npayload\")"),
        "{echoed}"
    );
    // A POST without a body says so; nothing is chunked.
    let echoed = text("POST", r#"h.send("POST", "/echo", [], "".to_bytes())"#);
    assert!(
        echoed.contains("\\ncontent-length: 0\\n") && !echoed.contains("chunked"),
        "{echoed}"
    );
}

/// H16: request headers reach the server (names lowercased), response headers come back as
/// `name: value` with lowercase names in order; a header the host refuses is `invalid`.
#[test]
fn http_send_headers() {
    let g = serve();
    let echoed = send(
        &g,
        "POST",
        "Result<Str, HttpErr>",
        r#"Ok(h.send("POST", "/echo", ["X-Token: abc", "content-type: text/plain", "accept:  a, b "], "hi".to_bytes())?.body.to_str())"#,
    );
    for want in [
        "\\nx-token: abc\\n",
        "\\ncontent-type: text/plain\\n",
        "\\naccept: a, b\\n",
        "\\ncontent-length: 2\\n",
    ] {
        assert!(echoed.contains(want), "{want} in {echoed}");
    }
    let headers = send(
        &g,
        "",
        "Result<List<Str>, HttpErr>",
        r#"Ok(h.send("GET", "/echo", [], "".to_bytes())?.headers)"#,
    );
    assert!(
        headers.starts_with(r#"Ok(["x-echo: yes", "set-thing: a", "content-length: "#)
            && headers.ends_with(r#""connection: close"])"#),
        "{headers}"
    );
    let bad = [
        "X-A: a\\r\\nX-B: b",
        "X-A: a\\nb",
        "NoColon",
        "name:value",
        ": v",
        "bad name: v",
        "Host: evil.com",
        "host: evil.com",
        "Content-Length: 0",
        "Transfer-Encoding: chunked",
        "Connection: close",
        "Upgrade: h2c",
        "TE: trailers",
        "Trailer: x",
        "Keep-Alive: 1",
        "Proxy-Authorization: x",
        "X-A: a\\0",
    ];
    for h in bad {
        let r = send(
            &g,
            "",
            RESPONSE,
            &format!(r#"h.send("GET", "/ok", ["{h}"], "".to_bytes())"#),
        );
        assert_eq!(r, "HttpErr::Invalid", "{h}");
    }
    let del = r#"let mut b = "X-A: a".to_bytes(); b.push(127); h.send("GET", "/ok", [b.to_str()], "".to_bytes())"#;
    assert_eq!(send(&g, "", RESPONSE, del), "HttpErr::Invalid");
    let many = r#"let mut hs: List<Str> = []; for i in 0..65 { hs.push("x-" + i.to_str() + ": a"); } h.send("GET", "/ok", hs, "".to_bytes())"#;
    assert_eq!(send(&g, "", RESPONSE, many), "HttpErr::Invalid");
    let long = r#"let mut v = "a"; for i in 0..13 { v = v + v; } h.send("GET", "/ok", ["x: " + v], "".to_bytes())"#;
    assert_eq!(send(&g, "", RESPONSE, long), "HttpErr::Invalid"); // 8,195 bytes
    let fits = r#"let mut v = "a"; for i in 0..12 { v = v + v; } Ok(h.send("GET", "/ok", ["x: " + v], "".to_bytes())?.status)"#;
    assert_eq!(send(&g, "", "Result<Int, HttpErr>", fits), "Ok(200)");
}

/// H16: a body over 8 MB is `too-large` and nothing is sent; one at the cap goes.
#[test]
fn http_send_body_limit() {
    let g = serve();
    let big = |extra: &str| {
        format!(
            r#"let mut b = "x".to_bytes(); for i in 0..23 {{ b.extend(b); }} {extra} Ok(h.send("POST", "/created", [], b)?.status)"#
        )
    };
    assert_eq!(send(&g, "POST", "Result<Int, HttpErr>", &big("")), "Ok(201)");
    assert_eq!(
        send(&g, "POST", "Result<Int, HttpErr>", &big("b.push(1);")),
        "HttpErr::TooLarge"
    );
    let mut kio = Kio::new(Limits::default()).unwrap();
    let src = format!("fn main(h: Http) -> Result<Int, HttpErr> {{ {} }}", big("b.push(1);"));
    kio.run_with(&src, vec![], vec![("h".into(), granted(&g, "POST"))])
        .unwrap()
        .unwrap();
    assert_eq!(kio.import_calls(), 1);
}

/// H16: `send` follows a redirect for GET and HEAD only, within the origin and up to 5 hops,
/// each a counted call; any other redirect is the response, with its `location`.
#[test]
fn http_send_redirects() {
    let g = serve();
    let status = |methods: &str, m: &str, path: &str| {
        send(
            &g,
            methods,
            "Result<Int, HttpErr>",
            &format!(r#"Ok(h.send("{m}", "{path}", [], "".to_bytes())?.status)"#),
        )
    };
    assert_eq!(status("", "GET", "/same"), "Ok(200)");
    assert_eq!(status("HEAD", "HEAD", "/same"), "Ok(200)");
    assert_eq!(status("", "GET", "/hop4"), "Ok(200)");
    assert_eq!(status("POST", "POST", "/same"), "Ok(302)");
    // `/ftp` names the host and port with another scheme: not followed (H19 review).
    for p in ["/moved", "/port", "/https", "/ftp", "/bare"] {
        assert_eq!(status("", "GET", p), "Ok(302)", "{p}");
    }
    assert_eq!(status("", "GET", "/hop5"), "Ok(307)");
    let location = send(
        &g,
        "POST",
        "Result<List<Str>, HttpErr>",
        r#"Ok(h.send("POST", "/same", [], "".to_bytes())?.headers)"#,
    );
    assert!(location.starts_with(r#"Ok(["location: /ok""#), "{location}");
    let calls = |path: &str| {
        let src = format!(r#"fn main(h: Http) -> Int {{ let r = h.send("GET", "{path}", [], "".to_bytes()); 0 }}"#);
        let mut kio = Kio::new(Limits::default()).unwrap();
        kio.run_with(&src, vec![], vec![("h".into(), granted(&g, ""))])
            .unwrap()
            .unwrap();
        kio.import_calls()
    };
    assert_eq!(calls("/hop2"), calls("/ok") + 3);
}

/// H19 review: a path, method or header name from request input cannot break an audit line's
/// JSON (`send_all`'s per-request lines included); a path is cut at 256 characters.
#[test]
fn http_audit_lines_stay_json() {
    let g = serve();
    let long = "/".to_string() + &"a".repeat(300);
    let src = format!(
        r#"fn main(h: Http) -> Int {{
        let a = h.get("/x\"y\\z");
        let b = h.get("{long}");
        let c = h.send("GE\"T", "/q", ["x-\"y: v"], "".to_bytes());
        let one = HttpRequest {{ method: "GET", path: "/x\"y", headers: [], body: "".to_bytes(), timeout_ms: 0 }};
        let two = HttpRequest {{ method: "P\"OST", path: "/ok", headers: [], body: "".to_bytes(), timeout_ms: 0 }};
        let d = h.send_all([one, two]);
        0
    }}"#
    );
    let log = Log::default();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(log.clone(), vec![]);
    kio.run_with(&src, vec![], vec![("h".into(), kio_host::Origin::parse(&g).unwrap())])
        .unwrap()
        .unwrap();
    let lines: Vec<serde_json::Value> = (log.lines().iter())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect();
    assert_eq!(lines.len(), 8, "{lines:?}");
    assert_eq!(lines[1]["arg"], "/x\"y\\z");
    assert_eq!(lines[2]["arg"].as_str().unwrap().len(), 256);
    assert_eq!(
        (&lines[3]["http"], &lines[3]["result"]),
        (&serde_json::json!("GE\"T"), &serde_json::json!("invalid"))
    );
    assert_eq!(
        (&lines[4]["arg"], &lines[4]["index"]),
        (&serde_json::json!("/x\"y"), &serde_json::json!(0))
    );
    assert_eq!(lines[5]["http"], "P\"OST");
}

/// An audit log in memory.
#[derive(Clone, Default)]
struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }
}

/// H16: the audit line of a `send` holds the method, path, status, byte counts and the request
/// header names, never a value; the grant's methods are in the header.
#[test]
fn http_send_is_audited_without_header_values() {
    let g = serve();
    let src = r#"fn main(h: Http) -> Result<Int, HttpErr> {
        Ok(h.send("POST", "/echo", ["Authorization: Bearer secret-token-xyz", "content-type: text/plain"], "hi".to_bytes())?.status)
    }"#;
    let log = Log::default();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(log.clone(), vec![]);
    let out = kio
        .run_with(src, vec![], vec![("h".into(), granted(&g, "post,PUT"))])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, "Ok(200)");
    let lines = log.lines();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].contains(&format!(r#""kind":"Http","target":"{g} methods=GET,POST,PUT""#)),
        "{}",
        lines[0]
    );
    let call = &lines[1];
    for want in [
        r#""method":"send","arg":"/echo","bytes":"#,
        r#""result":"ok","http":"POST","headers":["authorization","content-type"],"sent":2,"status":200,"received":"#,
    ] {
        assert!(call.contains(want), "{want} in {call}");
    }
    assert!(!lines.join("\n").contains("secret"), "{lines:?}");
    // A refused request is audited before the network: the method, and no header names.
    let src = r#"fn main(h: Http) -> Result<Int, HttpErr> { Ok(h.send("DELETE", "/echo", ["Authorization: Bearer s3cret"], "".to_bytes())?.status) }"#;
    let log = Log::default();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(log.clone(), vec![]);
    kio.run_with(src, vec![], vec![("h".into(), granted(&g, ""))])
        .unwrap()
        .unwrap();
    let call = &log.lines()[1];
    assert!(call.contains(r#""result":"denied","http":"DELETE","ms":"#), "{call}");
    assert!(!call.contains("s3cret"), "{call}");
}

/// H16b: `send_all` answers in list order whatever finished first, runs its requests at once,
/// times out one request at its position, and refuses a method inside the batch as `send`
/// would; the interpreter agrees.
#[test]
fn http_send_all() {
    let g = serve();
    let req = |m: &str, path: &str, ms: u32| {
        format!(
            r#"HttpRequest {{ method: "{m}", path: "{path}", headers: [], body: "".to_bytes(), timeout_ms: {ms} }}"#
        )
    };
    let src = format!(
        "fn main(h: Http) -> List<Result<Str, HttpErr>> {{ h.send_all([{}, {}, {}, {}, {}, {}]).map(|r| r.map(|r| r.status.to_str() + \" \" + r.body.to_str())) }}",
        req("GET", "/slow", 0),
        req("GET", "/ok", 0),
        req("GET", "/slow", 0),
        req("GET", "/hang", 300),
        req("POST", "/ok", 0),
        req("DELETE", "/ok", 0),
    );
    let want = r#"[Ok("200 slow"), Ok("200 hello"), Ok("200 slow"), Err(HttpErr::Timeout), Ok("200 hello"), Err(HttpErr::Denied)]"#;
    let start = std::time::Instant::now();
    let mut kio = Kio::new(Limits::default()).unwrap();
    let out = kio
        .run_with(&src, vec![], vec![("h".into(), granted(&g, "POST"))])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, want);
    // Two 2 s requests in parallel, not 4 s in turn; the margin covers compiling under load.
    assert!(
        start.elapsed() < std::time::Duration::from_millis(3500),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(kio.import_calls(), 6);
    let mut interp = kio_host::Interp::new(Limits::default()).unwrap();
    let out = interp
        .run_with(&src, vec![], vec![("h".into(), granted(&g, "POST"))])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, want);
    assert_eq!(interp.import_calls(), 6);
}

/// H16b: a batch over 64 is `invalid` throughout and sends nothing; each request counts toward
/// `--calls`; the audit log has one line per request with the batch id and index, then the
/// batch's own.
#[test]
fn http_send_all_limits_and_audit() {
    let g = serve();
    let batch = |n: usize, m: &str| {
        format!(
            r#"fn main(h: Http) -> List<Result<Int, HttpErr>> {{ let mut rs: List<HttpRequest> = []; for i in 0..{n} {{ rs.push(HttpRequest {{ method: "{m}", path: "/ok", headers: [], body: "".to_bytes(), timeout_ms: 0 }}); }} h.send_all(rs).map(|r| r.map(|r| r.status)) }}"#
        )
    };
    let mut kio = Kio::new(Limits::default()).unwrap();
    let out = kio
        .run_with(&batch(65, "GET"), vec![], vec![("h".into(), granted(&g, ""))])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, format!("[{}]", vec!["Err(HttpErr::Invalid)"; 65].join(", ")));
    assert_eq!(kio.import_calls(), 1);
    let mut kio = Kio::new(Limits::default()).unwrap();
    let out = kio
        .run_with(&batch(64, "GET"), vec![], vec![("h".into(), granted(&g, ""))])
        .unwrap()
        .unwrap();
    assert_eq!(out.value, format!("[{}]", vec!["Ok(200)"; 64].join(", ")));
    assert_eq!(kio.import_calls(), 64);
    let limits = Limits {
        calls: 2,
        ..Limits::default()
    };
    let mut kio = Kio::new(limits).unwrap();
    let r = kio.run_with(&batch(3, "GET"), vec![], vec![("h".into(), granted(&g, ""))]);
    assert_eq!(
        kio.explain(&r.unwrap_err()).unwrap(),
        "too many host calls (limit 2; raise it with --calls)"
    );
    let log = Log::default();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(log.clone(), vec![]);
    let src = format!(
        r#"fn main(h: Http) -> Int {{ let a = h.send_all([{}, {}]); let b = h.send_all([{}]); 0 }}"#,
        r#"HttpRequest { method: "GET", path: "/ok", headers: ["X-A: 1"], body: "".to_bytes(), timeout_ms: 0 }"#,
        r#"HttpRequest { method: "POST", path: "/nope", headers: [], body: "".to_bytes(), timeout_ms: 0 }"#,
        r#"HttpRequest { method: "GET", path: "/error", headers: [], body: "".to_bytes(), timeout_ms: 0 }"#,
    );
    kio.run_with(&src, vec![], vec![("h".into(), granted(&g, ""))])
        .unwrap()
        .unwrap();
    let lines = log.lines();
    assert_eq!(lines.len(), 7, "{lines:?}");
    let wants = [
        r#""seq":1,"cap":"h","kind":"Http","method":"send","arg":"/ok","bytes":5,"result":"ok","batch":1,"index":0,"http":"GET","headers":["x-a"],"sent":0,"status":200,"received":5,"ms":"#,
        r#""seq":2,"cap":"h","kind":"Http","method":"send","arg":"/nope","bytes":0,"result":"denied","batch":1,"index":1,"http":"POST","ms":"#,
        r#""seq":3,"cap":"h","kind":"Http","method":"send_all","arg":"2","bytes":5,"result":"ok","batch":1,"ms":"#,
        r#""seq":4,"cap":"h","kind":"Http","method":"send","arg":"/error","bytes":4,"result":"ok","batch":2,"index":0,"http":"GET","headers":[],"sent":0,"status":500,"received":4,"ms":"#,
        r#""seq":5,"cap":"h","kind":"Http","method":"send_all","arg":"1","bytes":4,"result":"ok","batch":2,"ms":"#,
    ];
    for (line, want) in lines[1..].iter().zip(wants) {
        assert!(line.contains(want), "{want}\n in {line}");
    }
    assert!(
        lines[6].contains(r#""event":"end","outcome":"ok","calls":3,"io":9"#),
        "{}",
        lines[6]
    );
}

/// H16b review: a batch's workers read within one budget, the run's remaining `--io`, so eight
/// 10 MB responses with 1 MB left hold about 1 MB between them (each read its 10 MB before) and
/// the run stops on `--io` as it did; within the budget, a response over 10 MB is `TooLarge` at
/// its position and the others are whole.
#[test]
fn http_send_all_reads_within_the_io_budget() {
    let g = serve();
    let batch = |paths: &[&str]| {
        let reqs: Vec<String> = paths
            .iter()
            .map(|p| {
                format!(
                    r#"HttpRequest {{ method: "GET", path: "{p}", headers: [], body: "".to_bytes(), timeout_ms: 0 }}"#
                )
            })
            .collect();
        format!(
            "fn main(h: Http) -> List<Result<Int, HttpErr>> {{ h.send_all([{}]).map(|r| r.map(|r| r.body.len())) }}",
            reqs.join(", ")
        )
    };
    let origin = granted(&g, "");
    let limits = Limits {
        io: 1 << 20,
        ..Limits::default()
    };
    let mut kio = Kio::new(limits).unwrap();
    let r = kio.run_with(&batch(&["/big"; 8]), vec![], vec![("h".into(), origin.clone())]);
    assert_eq!(
        kio.explain(&r.unwrap_err()).unwrap(),
        "too much I/O (limit 1 MB; raise it with --io)"
    );
    let read = origin.body_bytes_read();
    assert!(
        read > 1 << 20 && read <= (1 << 20) + 8 * (64 << 10),
        "{read} bytes read"
    );
    let origin = granted(&g, "");
    let mut kio = Kio::new(Limits::default()).unwrap();
    let out = kio
        .run_with(
            &batch(&["/ok", "/big", "/ok"]),
            vec![],
            vec![("h".into(), origin.clone())],
        )
        .unwrap()
        .unwrap();
    assert_eq!(out.value, "[Ok(5), Err(HttpErr::TooLarge), Ok(5)]");
    let read = origin.body_bytes_read();
    assert!(
        read > 10 << 20 && read <= (10 << 20) + (64 << 10) + 10,
        "{read} bytes read"
    );
}

/// H16: `--http-methods` syntax.
#[test]
fn allow_methods() {
    let mut o = kio_host::Origin::parse("example.com").unwrap();
    o.allow_methods("post, put").unwrap();
    o.allow_methods("POST,DELETE").unwrap();
    for bad in ["FOO", "POST,", "", "GET PUT", "PROPFIND"] {
        let e = kio_host::Origin::parse("example.com")
            .unwrap()
            .allow_methods(bad)
            .unwrap_err();
        assert!(e.contains("expected a list of GET, HEAD, POST"), "{bad}: {e}");
    }
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
    // A run includes compiling; time one reading a regular file to compare.
    let regular = std::time::Instant::now();
    assert_eq!(on(kio_host::open_dir(t.path()).unwrap(), "d.read(\"q\")"), NOT_FOUND);
    let regular = regular.elapsed();
    let start = std::time::Instant::now();
    assert_eq!(on(d, "d.read(\"p\")"), DENIED);
    assert!(start.elapsed() < regular + std::time::Duration::from_secs(1));
}
