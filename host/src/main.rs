mod mcp;

use kio_host::{listen, open_dir, ClockSpec, Diag, Kio, Limits, LogSink, Origin, RngSpec, Server, Sources};

/// `kio run`'s flags.
const USAGE: &str = "[--dir name=path]... [--http name=host]... [--http-methods name=POST,PUT,..]... [--kv name=path[:ro]|:memory:]... [--kv-mb mb] [--db name=path[:ro]|:memory:]... [--db-mb mb] [--rng name[=seed:N]]... [--clock name[=fixed:T]]... [--log name=path|-]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb] [--audit log] [--listen addr] [--ws-origin origin]... [--host name]...";

fn main() {
    // The host checks and lowers programs natively, and compiled code runs on this thread's stack
    // up to `--stack`: run on a stack well above the largest `--stack`, not the main thread's.
    let t = std::thread::Builder::new().stack_size(kio_host::RUN_STACK);
    std::process::exit(t.spawn(cli).unwrap().join().unwrap());
}

fn cli() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check", path] => return check(path),
        ["check", path, "--base", base] => return check_base(path, base),
        ["mcp"] => return mcp::serve(),
        ["run", path, ref flags @ ..] => return run(path, flags),
        _ => eprintln!(
            "kio: a capability-typed language; code names what it needs, the operator grants what that is.\n\nusage: kio check <prog.kio> [--base <old.kio>]\n       kio mcp\n       kio run <prog.kio> {USAGE}"
        ),
    }
    2
}

/// The root file, then `NAME.kio` beside it for each `mod NAME;` line. This is operator input,
/// read by the host like the root; the program never sees the filesystem.
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
    match kio_host::check_files(&files) {
        Ok(report) => {
            println!("{}", report.lines.join("\n"));
            0
        }
        Err(diags) => print_diags(&files, diags),
    }
}

/// The authority `path` adds over `base` and the authority it drops: exit 3 when any was added.
fn check_base(path: &str, base: &str) -> i32 {
    let (old, new) = match (read_files(base), read_files(path)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    match kio_host::diff_files(&old, &new) {
        Ok(lines) if lines.is_empty() => {
            println!("no change in authority");
            0
        }
        Ok(lines) => {
            println!("{}", lines.join("\n"));
            let added = lines.iter().filter(|l| l.starts_with('+')).count();
            if added == 0 {
                println!("authority removed only");
                return 0;
            }
            println!(
                "authority added: {added} line{} (exit 3)",
                if added == 1 { "" } else { "s" }
            );
            3
        }
        Err((is_base, diags)) => {
            let (side, files) = if is_base { ("base", &old) } else { ("new", &new) };
            eprintln!("kio: the {side} program {} does not check", files[0].0);
            print_diags(files, diags)
        }
    }
}

fn run(path: &str, flags: &[&str]) -> i32 {
    let files = match read_files(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let mut dirs = Vec::new();
    let mut https: Vec<(String, Origin)> = Vec::new();
    // `(name, methods)` of each `--http-methods`, applied once every `--http` is known.
    let mut http_methods: Vec<(String, String)> = Vec::new();
    let mut limits = if flags.contains(&"--listen") {
        Limits::web()
    } else {
        Limits::default()
    };
    let mut audit = None;
    let mut listen = None;
    let mut ws_origins = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut dir_paths = Vec::new();
    let mut kv_specs: Vec<(String, String)> = Vec::new();
    let mut kv_mb = 64;
    let mut db_specs: Vec<(String, String)> = Vec::new();
    let mut db_mb = kio_host::db::DB_MB;
    let mut sources = Sources::default();
    // `(name, path)` of each `--log` to a file, opened once the paths are checked.
    let mut log_paths: Vec<(String, String)> = Vec::new();
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
            ["--http-methods", spec] => {
                match spec.split_once('=') {
                    Some((name, list)) if !name.is_empty() && !list.is_empty() => {
                        http_methods.push((name.into(), list.into()))
                    }
                    _ => return fail(format!("--http-methods {spec}: expected name=GET,POST,..")),
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
                    Some((_, ":memory::ro")) => {
                        return fail(format!(
                            "--kv {spec}: an in-memory store is never read-only\n  hint: drop `:ro`; only this process can write it"
                        ))
                    }
                    Some((name, s)) if !name.is_empty() && !s.is_empty() => kv_specs.push((name.into(), s.into())),
                    _ => return fail(format!("--kv {spec}: expected name=path, name=path:ro or name=:memory:")),
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
            ["--db-mb", n] => {
                match n.parse::<u64>().ok().filter(|&v| v > 0) {
                    Some(v) => db_mb = v,
                    None => return fail(format!("--db-mb {n}: expected a positive integer")),
                }
                continue;
            }
            ["--db", spec] => {
                match spec.split_once('=') {
                    Some((name, s)) if !name.is_empty() && !s.is_empty() => db_specs.push((name.into(), s.into())),
                    _ => {
                        return fail(format!(
                            "--db {spec}: expected name=path, name=path:ro or name=:memory:"
                        ))
                    }
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
            ["--ws-origin", origin] => {
                ws_origins.push(origin.to_string());
                continue;
            }
            ["--host", name] => {
                hosts.push(name.to_ascii_lowercase());
                continue;
            }
            [flag @ ("--rng" | "--clock"), spec] => {
                let (name, rest) = match spec.split_once('=') {
                    Some((n, r)) => (n, Some(r)),
                    None => (*spec, None),
                };
                if name.is_empty() {
                    return fail(format!("{flag} {spec}: expected a name"));
                }
                let parsed = match *flag {
                    "--rng" => RngSpec::parse(rest).map(|r| sources.rngs.push((name.into(), r))),
                    _ => ClockSpec::parse(rest).map(|c| sources.clocks.push((name.into(), c))),
                };
                if let Err(e) = parsed {
                    return fail(format!("{flag} {spec}: {e}"));
                }
                continue;
            }
            ["--log", spec] => {
                match spec.split_once('=') {
                    Some((name, "-")) if !name.is_empty() => sources.logs.push((name.into(), LogSink::stderr())),
                    Some((name, path)) if !name.is_empty() && !path.is_empty() => {
                        log_paths.push((name.into(), path.into()))
                    }
                    _ => return fail(format!("--log {spec}: expected name=path or name=- (stderr)")),
                }
                continue;
            }
            ["--dir", spec] => match spec.split_once('=') {
                Some(nd) => nd,
                None => return fail(format!("--dir {spec}: expected name=path")),
            },
            _ => return fail(format!("usage: kio run <prog.kio> {USAGE}")),
        };
        match open_dir(dir.as_ref()) {
            Ok(d) => {
                dirs.push((name.to_string(), d));
                dir_paths.push((name.to_string(), dir.to_string()));
            }
            Err(e) => return fail(format!("--dir {name}={dir}: {e}")),
        }
    }
    for (i, (name, list)) in http_methods.iter().enumerate() {
        if http_methods[..i].iter().any(|(n, _)| n == name) {
            return fail(format!(
                "--http-methods {name}={list}: `{name}` is given more than once\n  hint: list every method in one flag: `--http-methods {name}=GET,POST,PUT`"
            ));
        }
        let Some((_, origin)) = https.iter_mut().find(|(n, _)| n == name) else {
            return fail(format!(
                "--http-methods {name}={list}: no `--http {name}=host` grant\n  hint: --http-methods adds methods to an Http grant of that name; GET needs none"
            ));
        };
        if let Err(e) = origin.allow_methods(list) {
            return fail(format!("--http-methods {name}={list}: {e}"));
        }
    }
    // A program that fails to check is reported by the run below.
    let checked = lang::check_files(
        &files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect::<Vec<_>>(),
        &lang::CapTable::v0(),
    );
    let params = checked.as_ref().map(|p| p.main_params()).unwrap_or_default();
    // A read-only store or database is refused to a program that can write it (H12, H17); the
    // host also stops any write that reaches it.
    let report = checked.as_ref().map(lang::report).unwrap_or_default();
    type WriteIn = fn(&str, &str) -> Option<&'static str>;
    let readers = (kv_specs
        .iter()
        .map(|(n, s)| (n, s, kio_host::kv::write_in_report as WriteIn)))
    .chain(
        db_specs
            .iter()
            .map(|(n, s)| (n, s, kio_host::db::write_in_report as WriteIn)),
    );
    for (name, spec, write_in_report) in readers {
        if !kio_host::kv::split_spec(spec).1 {
            continue;
        }
        if let Some(m) = write_in_report(&report, name) {
            return fail(format!(
                "`{name}` is granted read-only but `main` can call `{name} {m}`\n  hint: drop `:ro` or remove the writes"
            ));
        }
    }
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
    if listen.is_none() && !hosts.is_empty() {
        return fail("--host without --listen\n  hint: --host names a Host the server answers to; add `--listen 127.0.0.1:8080` or drop it".into());
    }
    match (listen, http, ws) {
        (None, true, _) => return fail("main takes Incoming/Outgoing but there is no address\n  hint: add `--listen 127.0.0.1:8080`".into()),
        (None, _, true) => return fail("main takes WsMessage/WsSend but there is no address\n  hint: add `--listen 127.0.0.1:8080`".into()),
        (Some(_), false, false) => return fail("--listen needs a `main` that takes Incoming and Outgoing, or WsMessage and WsSend\n  hint: add `req: Incoming, res: Outgoing` (HTTP) or `msg: WsMessage, conn: WsSend` (WebSocket) to main, or drop --listen".into()),
        _ => {}
    }
    if !ws && !ws_origins.is_empty() {
        return fail("--ws-origin without a WebSocket server\n  hint: --ws-origin lets browser pages from that origin connect to a `main` that takes WsMessage and WsSend; drop it".into());
    }
    // The compile cache holds native code later runs load as trusted. A program granted a `Dir`
    // over it could plant an entry, so then this run does not use it (open question 75). Says
    // nothing: the cache is an optimization, and its path is not the program's business.
    let cache = kio_host::cache::default_dir();
    let resolved = cache.as_deref().and_then(kio_host::cache::resolve);
    let apart = |c: &std::path::PathBuf| {
        (dir_paths.iter()).all(|(_, d)| std::fs::canonicalize(d).is_ok_and(|r| !kio_host::cache::overlaps(c, &r)))
    };
    let use_cache = resolved.as_ref().is_some_and(apart);
    // A program that can write a store's log or a database through a `Dir` could replace or
    // craft it; one inside the cache could likewise become an entry. `stores` is each `--kv` and
    // `--db` file with the files beside it that are its own (a compaction's `.tmp`, SQLite's
    // `-wal`, `-shm` and `-journal`), by flag.
    let mut stores: Vec<(String, String)> = Vec::new();
    // Each grant's files, its own path first.
    let mut grants: Vec<(String, Vec<String>)> = Vec::new();
    let on_disk = |specs: &[(String, String)], flag: &str| -> Vec<(String, String)> {
        (specs.iter().filter(|(_, s)| s != ":memory:"))
            .map(|(n, s)| (format!("{flag} {n}"), kio_host::kv::split_spec(s).0.to_string()))
            .collect()
    };
    for (flag, path) in on_disk(&kv_specs, "--kv").into_iter().chain(on_disk(&db_specs, "--db")) {
        let store = kio_host::cache::resolve(path.as_ref());
        if let (Some(c), Some(s)) = (&resolved, store) {
            if s.starts_with(c) {
                return fail(format!(
                    "`{flag}` is inside kio's compile cache\n  hint: keep stores outside `$XDG_CACHE_HOME/kio` (default `~/.cache/kio`)"
                ));
            }
        }
        let parent = std::path::Path::new(&path).parent();
        let parent = parent.filter(|p| !p.as_os_str().is_empty()).unwrap_or(".".as_ref());
        if let Ok(parent) = std::fs::canonicalize(parent) {
            for (d, dir) in &dir_paths {
                if std::fs::canonicalize(dir).is_ok_and(|root| parent.starts_with(root)) {
                    return fail(format!(
                        "`{flag}` is inside `--dir {d}`\n  hint: keep stores outside granted directories"
                    ));
                }
            }
        }
        // A database granted on a store's file (or the other way round) would corrupt it.
        if let Some((other, _)) = stores
            .iter()
            .find(|(o, p)| !o.starts_with(&flag[..4]) && same_file(p, &path))
        {
            return fail(format!(
                "`{flag}` is the same file as `{other}`\n  hint: give each its own file"
            ));
        }
        let own: &[&str] = if flag.starts_with("--kv") {
            &["", ".tmp"]
        } else {
            &["", "-wal", "-shm", "-journal"]
        };
        let own: Vec<String> = own.iter().map(|s| format!("{path}{s}")).collect();
        // A grant naming another's `-wal` (or the other way round) shares a file with it. Two
        // grants of one file are `open_all`'s to refuse.
        for (other, theirs) in grants.iter().filter(|(o, _)| o.starts_with(&flag[..4])) {
            if same_file(&own[0], &theirs[0]) {
                continue;
            }
            let pairs = own.iter().flat_map(|m| theirs.iter().map(move |t| (m, t)));
            if let Some((m, t)) = pairs.into_iter().find(|(m, t)| same_file(m, t)) {
                return fail(format!(
                    "`{flag}` and `{other}` share the file {}\n  hint: a database's `-wal`, `-shm` and `-journal` (a store's `.tmp`) are its own: give each grant its own file",
                    if *m == path { t } else { m }
                ));
            }
        }
        grants.push((flag.clone(), own.clone()));
        stores.extend(own.into_iter().map(|p| (flag.clone(), p)));
    }
    // The audit log and each `--log` file: not a store, not inside a granted dir or the cache,
    // and not one another's file.
    let mut taken: Vec<(String, &str)> = Vec::new();
    let mut files_to_check: Vec<(String, &str)> = Vec::new();
    if let Some(path) = &audit {
        files_to_check.push(("--audit".into(), path));
    }
    files_to_check.extend(log_paths.iter().map(|(n, p)| (format!("--log {n}"), p.as_str())));
    for (flag, path) in files_to_check {
        if let Some(why) = file_refusal(&flag, path, &stores, &dir_paths, resolved.as_deref(), &taken) {
            return fail(why);
        }
        taken.push((flag, path));
    }
    let kvs = match kio_host::kv::open_all(&kv_specs, kv_mb) {
        Ok(kvs) => kvs,
        Err(e) => return fail(e),
    };
    match kio_host::db::open_all(&db_specs, db_mb) {
        Ok(dbs) => sources.dbs = dbs,
        Err(e) => return fail(e),
    }
    for (name, path) in &log_paths {
        match open_append(path) {
            Ok(f) => sources.logs.push((name.clone(), LogSink::file(f, path))),
            Err(e) => return fail(format!("--log {name}={path}: {e}")),
        }
    }
    if let Some(addr) = listen {
        let audit = audit.map(|path| (path, dir_paths));
        return serve(files, dirs, https, kvs, sources, addr, limits, ws_origins, hosts, audit);
    }
    let mut kio = match Kio::new(limits) {
        Ok(k) => k,
        Err(e) => return fail(format!("{e:#}")),
    };
    if let Some(path) = audit {
        match open_append(&path) {
            Ok(f) => kio.audit(f, dir_paths),
            Err(e) => return fail(format!("--audit {path}: {e}")),
        }
    }
    kio.grant_kvs(kvs);
    kio.grant_sources(sources);
    if let Some(dir) = cache.filter(|_| use_cache) {
        kio.cache(dir);
    }
    match kio.run_files(&files, dirs, https) {
        Err(e) => {
            // A limit or trap is the program failing, not a usage error.
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

/// `run --listen`: check and compile once, then answer requests until Ctrl-C. `audit` is the
/// log's path and each `Dir` grant's `(name, path)`.
#[allow(clippy::too_many_arguments)]
fn serve(
    files: Vec<(String, String)>,
    dirs: Vec<(String, cap_std::fs::Dir)>,
    https: Vec<(String, kio_host::Origin)>,
    kvs: kio_host::KvGrants,
    sources: Sources,
    addr: std::net::SocketAddr,
    limits: Limits,
    ws_origins: Vec<String>,
    hosts: Vec<String>,
    audit: Option<(String, kio_host::DirPaths)>,
) -> i32 {
    let mut server = match Server::new(files.clone(), limits) {
        Err(e) => return fail(format!("{e:#}")),
        Ok(Err(diags)) => return print_diags(&files, diags),
        Ok(Ok(s)) => s,
    };
    server.grant_https(https);
    server.grant_kvs(kvs);
    server.grant_sources(sources);
    if let Some((path, dir_paths)) = audit {
        let opened = open_append(&path).and_then(|f| server.audit(f, dir_paths));
        if let Err(e) = opened {
            return fail(format!("--audit {path}: {e}"));
        }
    }
    match listen::serve(server, dirs, addr, ws_origins, hosts) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

/// Whether `a` and `b` name one file: the same path once symlinks resolve, or the same device
/// and inode (so a hard link too).
fn same_file(a: &str, b: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let id = |p: &str| std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()));
    let resolve = |p: &str| kio_host::cache::resolve(p.as_ref());
    id(a).is_some() && id(a) == id(b) || resolve(a).is_some_and(|r| Some(r) == resolve(b))
}

/// Why the file `path` of `flag` (`--audit`, `--log name`) is refused, if it is: it must not be a
/// `--kv` store's or `--db` database's file (one of `stores`; writing it would corrupt it), lie
/// inside a granted `--dir` (the program could read or replace its own log) or inside the compile
/// cache, or be the file of another flag in `taken`. Checked before any of them is opened.
fn file_refusal(
    flag: &str,
    path: &str,
    stores: &[(String, String)],
    dir_paths: &[(String, String)],
    cache: Option<&std::path::Path>,
    taken: &[(String, &str)],
) -> Option<String> {
    let what = if flag == "--audit" { "the audit log" } else { "the log" };
    let Some(log) = kio_host::cache::resolve(path.as_ref()) else {
        return Some(format!(
            "`{flag} {path}` has `.` or `..` past the directories that exist\n  hint: name {what} by a plain path"
        ));
    };
    let same = |p: &str| same_file(path, p);
    for (other, p) in stores {
        if same(p) {
            return Some(format!(
                "`{flag} {path}` is the same file as `{other}`\n  hint: give {what} its own file; writing it would corrupt the store"
            ));
        }
    }
    for (other, p) in taken {
        if same(p) {
            return Some(format!(
                "`{flag} {path}` is the same file as `{other}`\n  hint: give each log its own file"
            ));
        }
    }
    if cache.is_some_and(|c| log.starts_with(c)) {
        return Some(format!(
            "`{flag} {path}` is inside kio's compile cache\n  hint: keep {what} outside `$XDG_CACHE_HOME/kio` (default `~/.cache/kio`)"
        ));
    }
    for (d, dir) in dir_paths {
        if std::fs::canonicalize(dir).is_ok_and(|root| log.starts_with(root)) {
            return Some(format!(
                "`{flag} {path}` is inside `--dir {d}`\n  hint: keep {what} outside granted directories, where the program cannot read or replace it"
            ));
        }
    }
    None
}

/// Open a log (`--audit`, `--log`) to append, creating it: an existing file is never truncated,
/// and a symlink is never followed (it could lead past `file_refusal`'s checks).
fn open_append(path: &str) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
}

/// The file index and 1-based line of each diag. Any file number or offset maps to some line,
/// never a panic.
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
