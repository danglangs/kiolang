mod mcp;

use kio_host::{listen, open_dir, Diag, Kio, Limits, Origin, Server};

fn main() {
    // The host checks and lowers programs natively, recursing as deep as the guest may; run on a
    // stack well above the largest `--stack`, not the main thread's default.
    let t = std::thread::Builder::new().stack_size(4 * kio_host::MAX_STACK);
    std::process::exit(t.spawn(cli).unwrap().join().unwrap());
}

fn cli() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check", path] => return check(path),
        ["mcp"] => return mcp::serve(),
        ["run", path, ref flags @ ..] => return run(path, flags),
        _ => eprintln!(
            "kio: a capability-typed language; code names what it needs, the operator grants what that is.\n\nusage: kio check <prog.kio>\n       kio mcp\n       kio run <prog.kio> [--dir name=path]... [--http name=host]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb] [--kv name=path|:memory:]... [--kv-mb mb] [--audit log] [--listen addr]"
        ),
    }
    2
}

/// The root file, then `NAME.kio` beside it for each `mod NAME;` line. This is operator input,
/// read by the host like the root; the guest never sees the filesystem.
fn read_files(path: &str) -> Result<Vec<(String, String)>, i32> {
    let root = std::fs::read_to_string(path).map_err(|e| fail(format!("{path}: cannot read: {e}")))?;
    let dir = std::path::Path::new(path).parent().unwrap_or(".".as_ref());
    let mut files = Vec::new();
    for name in root.lines().filter_map(mod_name) {
        let file = dir.join(format!("{name}.kio"));
        let src = std::fs::read_to_string(&file)
            .map_err(|e| fail(format!("mod {name}: cannot read {}: {e}", file.display())))?;
        files.push((format!("{name}.kio"), src));
    }
    files.insert(0, (path.to_string(), root));
    Ok(files)
}

/// `web` for a line that is `mod web;`.
fn mod_name(line: &str) -> Option<&str> {
    let name = line.trim().strip_prefix("mod ")?.strip_suffix(';')?.trim();
    let mut chars = name.chars();
    let ident = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    ident.then_some(name)
}

fn check(path: &str) -> i32 {
    let files = match read_files(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    match Kio::new(Limits::default()).and_then(|mut k| k.check_files(&files)) {
        Err(e) => fail(format!("{e:#}")),
        Ok(Ok(report)) => {
            println!("{}", report.lines.join("\n"));
            0
        }
        Ok(Err(diags)) => print_diags(&files, diags),
    }
}

fn run(path: &str, flags: &[&str]) -> i32 {
    let files = match read_files(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let mut dirs = Vec::new();
    let mut https = Vec::new();
    let mut limits = if flags.contains(&"--listen") {
        Limits::web()
    } else {
        Limits::default()
    };
    let mut audit = None;
    let mut listen = None;
    let mut dir_paths = Vec::new();
    let mut kv_specs: Vec<(String, String)> = Vec::new();
    let mut kv_mb = 64;
    for pair in flags.chunks(2) {
        let (name, dir) = match pair {
            ["--http", spec] => {
                let Some((name, origin)) = spec.split_once('=') else {
                    return fail(format!("--http {spec}: expected name=host"));
                };
                match Origin::parse(origin) {
                    Ok(o) => https.push((name.to_string(), o)),
                    Err(e) => return fail(format!("--http {name}={origin}: {e}")),
                }
                continue;
            }
            [flag @ ("--fuel" | "--timeout" | "--memory" | "--stack" | "--calls" | "--io"), n] => {
                let Some(v) = n.parse::<u64>().ok().filter(|&v| v > 0) else {
                    return fail(format!("{flag} {n}: expected a positive integer"));
                };
                match *flag {
                    "--fuel" => limits.fuel = v,
                    "--timeout" => limits.timeout_secs = v,
                    "--memory" => limits.memory = (v as usize).saturating_mul(1 << 20),
                    "--calls" => limits.calls = u32::try_from(v).unwrap_or(u32::MAX),
                    "--io" => limits.io = v.saturating_mul(1 << 20),
                    _ if v > (kio_host::MAX_STACK >> 10) as u64 => {
                        return fail(format!("--stack {n}: at most {} KB", kio_host::MAX_STACK >> 10));
                    }
                    _ => limits.stack = (v as usize).saturating_mul(1 << 10),
                }
                continue;
            }
            ["--kv", spec] => {
                match spec.split_once('=') {
                    Some((name, s)) if !name.is_empty() && !s.is_empty() => kv_specs.push((name.into(), s.into())),
                    _ => return fail(format!("--kv {spec}: expected name=path or name=:memory:")),
                }
                continue;
            }
            ["--kv-mb", n] => {
                match n.parse::<u64>().ok().filter(|&v| v > 0) {
                    Some(v) => kv_mb = v,
                    None => return fail(format!("--kv-mb {n}: expected a positive integer")),
                }
                continue;
            }
            ["--audit", path] => {
                // Created below, once the flags that conflict with it are known.
                audit = Some(path.to_string());
                continue;
            }
            ["--listen", spec] => {
                match spec.parse::<std::net::SocketAddr>() {
                    Ok(a) => listen = Some(a),
                    Err(e) => return fail(format!("--listen {spec}: {e}; expected ip:port")),
                }
                continue;
            }
            ["--dir", spec] => match spec.split_once('=') {
                Some(nd) => nd,
                None => return fail(format!("--dir {spec}: expected name=path")),
            },
            _ => return fail(
                "usage: kio run <prog.kio> [--dir name=path]... [--http name=host]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb] [--kv name=path|:memory:]... [--kv-mb mb] [--audit log] [--listen addr]"
                    .into(),
            ),
        };
        match open_dir(dir.as_ref()) {
            Ok(d) => {
                dirs.push((name.to_string(), d));
                dir_paths.push((name.to_string(), dir.to_string()));
            }
            Err(e) => return fail(format!("--dir {name}={dir}: {e}")),
        }
    }
    if listen.is_some() {
        if audit.is_some() {
            return fail(
                "--audit with --listen\n  hint: per-request audit logging is not supported yet; drop --audit".into(),
            );
        }
        if !https.is_empty() {
            return fail("--http with --listen\n  hint: Http grants under --listen are not supported yet".into());
        }
    }
    // A program that fails to check is reported by the run below.
    let params = lang::check_files(
        &files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect::<Vec<_>>(),
        &lang::CapTable::v0(),
    )
    .map(|p| p.main_params())
    .unwrap_or_default();
    let takes = |ty: &str| params.iter().filter(|(_, t)| t == ty).count();
    let http = takes("Incoming") + takes("Outgoing") > 0;
    let ws = takes("WsMessage") + takes("WsSend") > 0;
    if ws && http {
        return fail("main takes both WebSocket and HTTP grants\n  hint: a WebSocket server answers through WsSend; drop the Incoming and Outgoing parameters".into());
    }
    if ws && (takes("WsMessage"), takes("WsSend")) != (1, 1) {
        return fail("a WebSocket server's main takes one WsMessage and one WsSend\n  hint: write `fn main(msg: WsMessage, conn: WsSend, ..)`".into());
    }
    if http && (takes("Incoming"), takes("Outgoing")) != (1, 1) {
        return fail("an HTTP server's main takes one Incoming and one Outgoing\n  hint: write `fn main(request: Incoming, response: Outgoing, ..)`".into());
    }
    match (listen, http, ws) {
        (None, true, _) => return fail("main takes Incoming/Outgoing but there is no address\n  hint: add `--listen 127.0.0.1:8080`".into()),
        (None, _, true) => return fail("main takes WsMessage/WsSend but there is no address\n  hint: add `--listen 127.0.0.1:8080`".into()),
        (Some(_), false, false) => return fail("--listen needs a `main` that takes Incoming and Outgoing, or WsMessage and WsSend\n  hint: add `req: Incoming, res: Outgoing` (HTTP) or `msg: WsMessage, conn: WsSend` (WebSocket) to main, or drop --listen".into()),
        _ => {}
    }
    // A program that can write a store's log through a `Dir` could replace or craft it.
    for (name, spec) in kv_specs.iter().filter(|(_, s)| s != ":memory:") {
        let parent = std::path::Path::new(spec.as_str()).parent();
        let parent = parent.filter(|p| !p.as_os_str().is_empty()).unwrap_or(".".as_ref());
        let Ok(parent) = std::fs::canonicalize(parent) else {
            continue;
        };
        for (d, dir) in &dir_paths {
            if std::fs::canonicalize(dir).is_ok_and(|root| parent.starts_with(root)) {
                return fail(format!(
                    "`--kv {name}` is inside `--dir {d}`\n  hint: keep stores outside granted directories"
                ));
            }
        }
    }
    let kvs = match kio_host::kv::open_all(&kv_specs, kv_mb) {
        Ok(kvs) => kvs,
        Err(e) => return fail(e),
    };
    if let Some(addr) = listen {
        return serve(files, dirs, kvs, addr, limits);
    }
    let mut kio = match Kio::new(limits) {
        Ok(k) => k,
        Err(e) => return fail(format!("{e:#}")),
    };
    if let Some(path) = audit {
        match std::fs::File::create(&path) {
            Ok(f) => kio.audit(f, dir_paths),
            Err(e) => return fail(format!("--audit {path}: {e}")),
        }
    }
    kio.grant_kvs(kvs);
    match kio.run_files(&files, dirs, https) {
        Err(e) => {
            // A limit or trap is the guest failing, not a usage error.
            match kio.explain(&e) {
                Some(msg) => eprintln!("{msg}"),
                None => eprintln!("kio: {e:#}"),
            }
            1
        }
        Ok(Ok(out)) => {
            if !params.is_empty() {
                eprintln!("{}", out.report.lines.join("\n"));
            }
            // Like Rust's `main`: `()` and `Ok(())` print nothing, `Err(e)` goes to stderr and fails the run.
            if out.failed {
                eprintln!("error: {}", out.value);
            } else if out.value != "()" && out.value != "Ok(())" {
                println!("{}", out.value);
            }
            i32::from(out.failed)
        }
        Ok(Err(diags)) => print_diags(&files, diags),
    }
}

/// `run --listen`: check once, print the report, then answer requests until Ctrl-C.
fn serve(
    files: Vec<(String, String)>,
    dirs: Vec<(String, cap_std::fs::Dir)>,
    kvs: kio_host::KvGrants,
    addr: std::net::SocketAddr,
    limits: Limits,
) -> i32 {
    let mut server = match Server::new(files.clone(), limits) {
        Err(e) => return fail(format!("{e:#}")),
        Ok(Err(diags)) => return print_diags(&files, diags),
        Ok(Ok(s)) => s,
    };
    server.grant_kvs(kvs);
    match listen::serve(server, dirs, addr) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

/// The file index and 1-based line of each diag. The guest is untrusted: any file number or
/// offset maps to some line, never a panic.
fn locate(files: &[(String, String)], diags: &[Diag]) -> Vec<(usize, usize)> {
    // Byte offset of each line start, one table per file: per-diag scans are quadratic in the source.
    let starts: Vec<Vec<usize>> = files
        .iter()
        .map(|(_, src)| {
            std::iter::once(0)
                .chain(src.match_indices('\n').map(|(i, _)| i + 1))
                .collect()
        })
        .collect();
    diags
        .iter()
        .map(|d| {
            let file = (d.file as usize).min(files.len() - 1);
            (file, starts[file].partition_point(|&s| s <= d.start as usize).max(1))
        })
        .collect()
}

fn print_diags(files: &[(String, String)], diags: Vec<Diag>) -> i32 {
    for (d, (file, line)) in diags.iter().zip(locate(files, &diags)) {
        eprintln!("{}:{line}: error: {}", files[file].0, d.message);
        if let Some(h) = &d.hint {
            eprintln!("  hint: {h}");
        }
    }
    1
}

fn fail(msg: String) -> i32 {
    eprintln!("kio: {msg}");
    2
}
