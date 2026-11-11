mod mcp;

use kio_host::{listen, open_dir, ClockSpec, Diag, HmacKey, Kio, Limits, LogSink, Origin, RngSpec, Server, Sources};

/// `kio run`'s flags.
const USAGE: &str = "[--grants file] [--dir name=path]... [--http name=host]... [--http-methods name=POST,PUT,..]... [--http-header name=Header:env:VAR|file:path]... [--kv name=path[:ro]|:memory:]... [--kv-mb mb] [--db name=path[:ro]|:memory:]... [--db-mb mb] [--rng name[=seed:N]]... [--clock name[=fixed:T]]... [--log name=path|-]... [--hmac name=env:VAR|file:path[:verify]]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb] [--audit log] [--listen addr] [--ws-origin origin]... [--host name]...";

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
        ["check", path, "--base", base] => return check_base(path, base, false),
        ["check", path, "--base", base, "--detail"] => return check_base(path, base, true),
        ["check", path, ref flags @ ..] if flags.contains(&"--grants") => return run(path, flags, true),
        ["mcp"] => return mcp::serve(),
        ["run", path, ref flags @ ..] => return run(path, flags, false),
        _ => eprintln!(
            "kio: a capability-typed language; code names what it needs, the operator grants what that is.\n\nusage: kio check <prog.kio> [--base <old.kio> [--detail]]\n       kio check <prog.kio> --grants <file> [flags]\n       kio mcp\n       kio run <prog.kio> {USAGE}"
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
/// One line per changed grant; then, with `detail` or when those lines do not show every
/// addition, a blank line and one per changed function.
fn check_base(path: &str, base: &str, detail: bool) -> i32 {
    let (old, new) = match (read_files(base), read_files(path)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    let diff = match kio_host::diff_files(&old, &new) {
        Ok(diff) => diff,
        Err((is_base, diags)) => {
            let (side, files) = if is_base { ("base", &old) } else { ("new", &new) };
            eprintln!("kio: the {side} program {} does not check", files[0].0);
            return print_diags(files, diags);
        }
    };
    if diff.grants.is_empty() && diff.funcs.is_empty() {
        println!("no change in authority");
        return 0;
    }
    // A grant line adds authority when it has a `+`: no name, type or method has one.
    let funcs = detail || (diff.added && !diff.grants.iter().any(|l| l.contains('+'))) || diff.grants.is_empty();
    let mut out = diff.grants.clone();
    if funcs {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.extend(diff.funcs.iter().cloned());
    }
    println!("{}", out.join("\n"));
    if !diff.added {
        println!("authority removed only");
        return 0;
    }
    let hint = if funcs || diff.funcs.is_empty() {
        ""
    } else {
        "; --detail lists each function"
    };
    println!("authority added (exit 3){hint}");
    3
}

/// Every flag of `kio run`, each with one value: what a grants file may hold.
const RUN_FLAGS: [&str; 22] = [
    "--dir",
    "--http",
    "--http-methods",
    "--http-header",
    "--kv",
    "--kv-mb",
    "--db",
    "--db-mb",
    "--rng",
    "--clock",
    "--log",
    "--hmac",
    "--fuel",
    "--timeout",
    "--memory",
    "--stack",
    "--calls",
    "--io",
    "--audit",
    "--listen",
    "--ws-origin",
    "--host",
];

/// The flags that take one value for the run: in a grants file at most once, and not also on
/// the command line.
const ONCE: [&str; 10] = [
    "--kv-mb",
    "--db-mb",
    "--fuel",
    "--timeout",
    "--memory",
    "--stack",
    "--calls",
    "--io",
    "--audit",
    "--listen",
];

/// `flags` with `--grants FILE` replaced by the file's flags, ahead of the others, and the file's
/// path. A line is one flag, split at the first space (no quoting: the value is the rest of the
/// line); blank lines and lines starting with `#` are skipped.
fn with_grants(flags: &[&str]) -> Result<(Vec<String>, Option<String>), i32> {
    let mut path = None;
    let mut rest: Vec<String> = Vec::new();
    for pair in flags.chunks(2) {
        match pair {
            ["--grants", _] if path.is_some() => {
                return Err(fail(
                    "--grants is given twice\n  hint: keep every grant in one file".into(),
                ))
            }
            ["--grants", p] => path = Some(p.to_string()),
            _ => rest.extend(pair.iter().map(|f| f.to_string())),
        }
    }
    let Some(path) = path else {
        return Ok((rest, None));
    };
    let text = std::fs::read_to_string(&path).map_err(|e| fail(format!("--grants {path}: cannot read: {e}")))?;
    let mut out: Vec<String> = Vec::new();
    // Each `ONCE` flag in the file and its line.
    let mut once: Vec<(&str, usize)> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = format!("{path}:{}", i + 1);
        let (flag, value) = line.split_once(' ').unwrap_or((line, ""));
        // Not shown: a line that is not a flag could be a key pasted by mistake.
        if !flag.starts_with("--") {
            return Err(fail(format!(
                "{at}: not a flag\n  hint: each line is one flag and its value, as on the command line: `--dir data=./data`"
            )));
        }
        if flag == "--grants" {
            return Err(fail(format!(
                "{at}: a grants file cannot name another\n  hint: put those grants in this file"
            )));
        }
        if !RUN_FLAGS.contains(&flag) {
            return Err(fail(format!(
                "{at}: unknown flag `{flag}`\n  hint: a line is one of `kio run`'s flags: {}",
                RUN_FLAGS.join(" ")
            )));
        }
        let value = value.trim_start();
        if value.is_empty() {
            return Err(fail(format!(
                "{at}: `{flag}` has no value\n  hint: write it after a space: `{flag} value`"
            )));
        }
        if ONCE.contains(&flag) {
            if let Some((_, n)) = once.iter().find(|(f, _)| *f == flag) {
                return Err(fail(format!(
                    "`{flag}` is given twice in {path}, lines {n} and {}\n  hint: give it once",
                    i + 1
                )));
            }
            if rest.iter().step_by(2).any(|f| f == flag) {
                return Err(fail(format!(
                    "`{flag}` is given in {at} and on the command line\n  hint: give it in one place"
                )));
            }
            once.push((flag, i + 1));
        }
        out.push(flag.into());
        out.push(value.into());
    }
    out.extend(rest);
    Ok((out, Some(path)))
}

/// Why the grants file `path` is refused, if it is: the program could change it, and so its own
/// grants for the next run, were it inside a granted `--dir` (another hard link could be) or the
/// file of a flag in `written` (`--audit`, `--log`, `--kv`, `--db`).
fn grants_refusal(path: &str, dirs: &[(String, String)], written: &[(String, &str)]) -> Option<String> {
    let changes = "the program could change its own grants for the next run";
    if std::fs::metadata(path).is_ok_and(|m| std::os::unix::fs::MetadataExt::nlink(&m) > 1) {
        return Some(format!(
            "`--grants {path}` has other hard links; {changes} through one\n  hint: give the grants file no other links"
        ));
    }
    // The file, and the name it was given: a symlink inside a `--dir` could be pointed elsewhere.
    let p = std::path::Path::new(path);
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(".".as_ref());
    let named = std::fs::canonicalize(parent)
        .ok()
        .zip(p.file_name())
        .map(|(d, f)| d.join(f));
    let places: Vec<_> = [std::fs::canonicalize(path).ok(), named]
        .into_iter()
        .flatten()
        .collect();
    for (d, dir) in dirs {
        if std::fs::canonicalize(dir).is_ok_and(|root| places.iter().any(|p| p.starts_with(&root))) {
            return Some(format!(
                "`--grants {path}` is inside `--dir {d}={dir}`; {changes}\n  hint: keep the grants file outside every granted directory"
            ));
        }
    }
    if let Some((flag, _)) = written.iter().find(|(_, f)| same_file(path, f)) {
        return Some(format!(
            "`--grants {path}` is the `{flag}` file; {changes}\n  hint: give the grants file a file of its own"
        ));
    }
    None
}

/// Apply one `--http-header name=Header:env:VAR` or `name=Header:file:PATH` to the grant `name`:
/// the value is read now, once. A file's one trailing newline is dropped. The file must be a
/// regular file outside every granted `dirs` (the program could read it) and not the file of a
/// flag in `files` (`--audit`, `--log`). Errors never hold the value.
fn http_header(
    spec: &str,
    https: &mut [(String, Origin)],
    dirs: &[(String, String)],
    files: &[(String, &str)],
    dry: bool,
) -> Result<(), String> {
    let usage = "expected name=Header:env:VAR or name=Header:file:PATH";
    let Some((name, rest)) = spec.split_once('=') else {
        return Err(usage.into());
    };
    let Some((header, source)) = rest.split_once(':') else {
        return Err(usage.into());
    };
    let Some((_, origin)) = https.iter_mut().find(|(n, _)| n == name) else {
        return Err(format!(
            "no `--http {name}=host` grant\n  hint: --http-header adds a header to an Http grant of that name"
        ));
    };
    let value = secret(source, usage, "the header's value", dirs, files, dry)?;
    origin.inject(header, value)
}

/// Grant one `--hmac name=env:VAR` or `name=file:PATH`, either ending in `:verify` to refuse
/// `sign`: the key is read now, once, with `--http-header`'s rules (`secret`). Errors never hold
/// the key.
fn hmac(
    spec: &str,
    dirs: &[(String, String)],
    files: &[(String, &str)],
    dry: bool,
) -> Result<(String, HmacKey), String> {
    let usage = "expected name=env:VAR or name=file:PATH, with `:verify` at the end to refuse sign";
    let Some((name, source)) = spec.split_once('=').filter(|(n, _)| !n.is_empty()) else {
        return Err(usage.into());
    };
    let (source, verify_only) = match source.strip_suffix(":verify") {
        Some(s) => (s, true),
        None => (source, false),
    };
    let value = secret(source, usage, "the key", dirs, files, dry)?;
    Ok((name.into(), HmacKey::new(value, verify_only)?))
}

/// The secret `source` names, `env:VAR` or `file:PATH` (`read_secret`), read once; `usage` for
/// any other form; `what` names it in the hint for an empty one. Errors never hold the value.
/// With `dry` (`kio check --grants`) only the form is checked: nothing is read, and `UNREAD`
/// stands in for the value.
fn secret(
    source: &str,
    usage: &str,
    what: &str,
    dirs: &[(String, String)],
    files: &[(String, &str)],
    dry: bool,
) -> Result<String, String> {
    let value = match source.split_once(':') {
        Some(("env", var)) if dry && !var.is_empty() && !var.contains(['=', '\0']) => return Ok(UNREAD.into()),
        Some(("file", path)) if dry && !path.is_empty() => return Ok(UNREAD.into()),
        Some(("env", var)) if !var.is_empty() && !var.contains(['=', '\0']) => match std::env::var(var) {
            Ok(v) => v,
            Err(std::env::VarError::NotPresent) => {
                return Err(format!(
                    "`{var}` is not set\n  hint: export {var} in the environment that runs kio, or use file:PATH"
                ))
            }
            Err(std::env::VarError::NotUnicode(_)) => return Err(format!("`{var}` is not UTF-8")),
        },
        Some(("file", path)) if !path.is_empty() => read_secret(path, dirs, files)?,
        _ => return Err(usage.into()),
    };
    if value.trim().is_empty() {
        return Err(format!(
            "{source} is empty or only whitespace\n  hint: set it to {what}"
        ));
    }
    Ok(value)
}

/// The value in the file `path` for `--http-header` or `--hmac`, its one trailing newline
/// dropped. The file must be a regular file outside every granted `dirs` (the program could
/// read it) and not the file of a flag in `files` (`--audit`, `--log`). At most `SECRET_MAX` + 2
/// bytes are read: a longer value is refused by `Origin::inject` and `HmacKey::new`.
fn read_secret(path: &str, dirs: &[(String, String)], files: &[(String, &str)]) -> Result<String, String> {
    let cannot =
        |e: &dyn std::fmt::Display| format!("cannot read {path}: {e}\n  hint: the file holds the secret alone");
    // A FIFO or device would block or never end: check before opening.
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(cannot(&"not a regular file")),
        Err(e) => return Err(cannot(&e)),
    }
    // Another name for the file could be inside a granted `--dir`, where the check below misses it.
    if std::fs::metadata(path).is_ok_and(|m| std::os::unix::fs::MetadataExt::nlink(&m) > 1) {
        return Err(format!(
            "`{path}` has other hard links; the program could read one\n  hint: give kio a key file of its own"
        ));
    }
    let real = std::fs::canonicalize(path).map_err(|e| cannot(&e))?;
    for (d, dir) in dirs {
        if std::fs::canonicalize(dir).is_ok_and(|root| real.starts_with(root)) {
            return Err(format!(
                "`{path}` is inside `--dir {d}={dir}`; the program could read it\n  hint: keep the key outside every granted directory"
            ));
        }
    }
    if let Some((flag, _)) = files.iter().find(|(_, f)| same_file(path, f)) {
        return Err(format!(
            "`{path}` is the `{flag}` file\n  hint: keep the key in a file of its own"
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| cannot(&e))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, SECRET_MAX + 2), &mut bytes).map_err(|e| cannot(&e))?;
    let v = String::from_utf8(bytes).map_err(|_| cannot(&"not UTF-8"))?;
    let v = v.strip_suffix('\n').unwrap_or(&v);
    Ok(v.strip_suffix('\r').unwrap_or(v).to_string())
}

/// What `kio check --grants` passes on for a secret it does not read: a valid header value and key.
const UNREAD: &str = "unread";

/// The most of a secret read from a file (as `Origin::inject` and `HmacKey::new` allow).
const SECRET_MAX: u64 = 8 << 10;

/// `spec` of a `--http-header` as an error may show it: `name=Header`, never what follows the
/// first `:` (or, without one, the `=`), where a value given inline by mistake would be.
fn header_label(spec: &str) -> &str {
    if !spec.contains('=') {
        return "";
    }
    match spec.split_once(':') {
        Some((label, _)) => label,
        None => spec.split('=').next().unwrap_or(""),
    }
}

/// `--flag label: e`, or `--flag: e` for an empty label.
fn flag_error(flag: &str, label: &str, e: &str) -> String {
    if label.is_empty() {
        format!("{flag}: {e}")
    } else {
        format!("{flag} {label}: {e}")
    }
}

/// `kio run`, or with `dry` `kio check --grants`: check the flags and bind them to `main`'s
/// parameters, print the report and stop, reading no secret and opening no granted path.
fn run(path: &str, flags: &[&str], dry: bool) -> i32 {
    let (flags, grants_file) = match with_grants(flags) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let files = match read_files(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let mut dirs = Vec::new();
    let mut https: Vec<(String, Origin)> = Vec::new();
    // `(name, methods)` of each `--http-methods`, applied once every `--http` is known.
    let mut http_methods: Vec<(String, String)> = Vec::new();
    // Each `--http-header` as given, read once every `--http` is known.
    let mut http_headers: Vec<&str> = Vec::new();
    // Each `--hmac` as given, read once the `--dir`, `--audit` and `--log` paths are known.
    let mut hmac_specs: Vec<&str> = Vec::new();
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
            ["--http-header", spec] => {
                http_headers.push(spec);
                continue;
            }
            ["--hmac", spec] => {
                hmac_specs.push(spec);
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
        dir_paths.push((name.to_string(), dir.to_string()));
        if dry {
            continue;
        }
        match open_dir(dir.as_ref()) {
            Ok(d) => dirs.push((name.to_string(), d)),
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
    let mut written: Vec<(String, &str)> = Vec::new();
    written.extend(audit.as_deref().map(|p| ("--audit".to_string(), p)));
    written.extend(log_paths.iter().map(|(n, p)| (format!("--log {n}"), p.as_str())));
    if let Some(g) = &grants_file {
        let stores = (kv_specs.iter().map(|(n, s)| (format!("--kv {n}"), s)))
            .chain(db_specs.iter().map(|(n, s)| (format!("--db {n}"), s)))
            .map(|(f, s)| (f, kio_host::kv::split_spec(s).0));
        let written: Vec<(String, &str)> = written.iter().cloned().chain(stores).collect();
        if let Some(why) = grants_refusal(g, &dir_paths, &written) {
            return fail(why);
        }
    }
    for spec in http_headers {
        if let Err(e) = http_header(spec, &mut https, &dir_paths, &written, dry) {
            return fail(flag_error("--http-header", header_label(spec), &e));
        }
    }
    // An error names the grant only: a key given inline by mistake would follow the `=`, or be
    // the whole spec when there is none.
    for spec in hmac_specs {
        match hmac(spec, &dir_paths, &written, dry) {
            Ok(grant) => sources.hmacs.push(grant),
            Err(e) => {
                let name = spec.split_once('=').map_or("", |(n, _)| n);
                return fail(flag_error("--hmac", name, &e));
            }
        }
    }
    // A program that fails to check says so before any check of `main`'s shape (H20).
    let checked = match lang::check_files(
        &files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect::<Vec<_>>(),
        &lang::CapTable::v0(),
    ) {
        Ok(p) => p,
        Err(diags) => return print_diags(&files, diags.into_iter().map(Diag::from).collect()),
    };
    let params = checked.main_params();
    // A read-only store or database is refused to a program that can write it (H12, H17); the
    // host also stops any write that reaches it.
    let report = lang::report(&checked);
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
    // A `:verify` key is refused to a program that can sign with it; the host also refuses a
    // `sign` that reaches it.
    for (name, key) in &sources.hmacs {
        if key.verify_only() && kio_host::signs_in_report(&report, name) {
            return fail(format!(
                "`{name}` is granted `:verify` but the program can sign with it\n  hint: drop `:verify`, or remove the `sign` call"
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
    if dry {
        let mut named: Vec<(String, &str)> = Vec::new();
        named.extend(dir_paths.iter().map(|(n, _)| (n.clone(), "Dir")));
        named.extend(https.iter().map(|(n, _)| (n.clone(), "Http")));
        named.extend(kv_specs.iter().map(|(n, _)| (n.clone(), "Kv")));
        named.extend(db_specs.iter().map(|(n, _)| (n.clone(), "Db")));
        named.extend(sources.rngs.iter().map(|(n, _)| (n.clone(), "Rng")));
        named.extend(sources.clocks.iter().map(|(n, _)| (n.clone(), "Clock")));
        named.extend(sources.logs.iter().map(|(n, _)| (n.clone(), "Log")));
        named.extend(log_paths.iter().map(|(n, _)| (n.clone(), "Log")));
        named.extend(sources.hmacs.iter().map(|(n, _)| (n.clone(), "Hmac")));
        // `--listen` grants these by type, to the parameters of that type.
        let socket = ["Incoming", "Outgoing", "WsMessage", "WsSend"];
        for (n, ty) in params.iter().filter(|_| listen.is_some()) {
            if let Some(kind) = socket.iter().find(|k| *k == ty) {
                named.push((n.clone(), kind));
            }
        }
        return match kio_host::bind_grants(&checked, &named) {
            Ok(()) => {
                println!("{}", report.lines().collect::<Vec<_>>().join("\n"));
                0
            }
            Err(diags) => print_diags(&files, diags),
        };
    }
    // A program that can write a store's log or a database through a `Dir` could replace or
    // craft it. `stores` is each `--kv` and `--db` file with the files beside it that are its own
    // (a compaction's `.tmp`, SQLite's `-wal`, `-shm` and `-journal`), by flag.
    let mut stores: Vec<(String, String)> = Vec::new();
    // Each grant's files, its own path first.
    let mut grants: Vec<(String, Vec<String>)> = Vec::new();
    let on_disk = |specs: &[(String, String)], flag: &str| -> Vec<(String, String)> {
        (specs.iter().filter(|(_, s)| s != ":memory:"))
            .map(|(n, s)| (format!("{flag} {n}"), kio_host::kv::split_spec(s).0.to_string()))
            .collect()
    };
    for (flag, path) in on_disk(&kv_specs, "--kv").into_iter().chain(on_disk(&db_specs, "--db")) {
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
    // The audit log and each `--log` file: not a store, not inside a granted dir, and not one
    // another's file.
    let mut taken: Vec<(String, &str)> = Vec::new();
    let mut files_to_check: Vec<(String, &str)> = Vec::new();
    if let Some(path) = &audit {
        files_to_check.push(("--audit".into(), path));
    }
    files_to_check.extend(log_paths.iter().map(|(n, p)| (format!("--log {n}"), p.as_str())));
    for (flag, path) in files_to_check {
        if let Some(why) = file_refusal(&flag, path, &stores, &dir_paths, &taken) {
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
    // A grant `main` does not take is refused before binding, exit 1 as plain `kio run` (H21).
    let names: Vec<String> = dirs.iter().map(|(n, _)| n.clone()).collect();
    if let Err(diags) = server.check_grants(&names) {
        return print_diags(&files, diags);
    }
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
    let resolve = |p: &str| kio_host::resolve(p.as_ref());
    id(a).is_some() && id(a) == id(b) || resolve(a).is_some_and(|r| Some(r) == resolve(b))
}

/// Why the file `path` of `flag` (`--audit`, `--log name`) is refused, if it is: it must not be a
/// `--kv` store's or `--db` database's file (one of `stores`; writing it would corrupt it), lie
/// inside a granted `--dir` (the program could read or replace its own log), or be the file of
/// another flag in `taken`. Checked before any of them is opened.
fn file_refusal(
    flag: &str,
    path: &str,
    stores: &[(String, String)],
    dir_paths: &[(String, String)],
    taken: &[(String, &str)],
) -> Option<String> {
    let what = if flag == "--audit" { "the audit log" } else { "the log" };
    let Some(log) = kio_host::resolve(path.as_ref()) else {
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
