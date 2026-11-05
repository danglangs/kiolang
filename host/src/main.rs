use host::{open_dir, Diag, Kio, Limits, Origin};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check", path] => std::process::exit(check(path)),
        ["run", path, ref flags @ ..] => std::process::exit(run(path, flags)),
        _ => eprintln!(
            "usage: kio check <prog.kio>\n       kio run <prog.kio> [--dir name=path]... [--http name=host]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb]"
        ),
    }
    std::process::exit(2);
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
    let mut limits = Limits::default();
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
                    _ => limits.stack = (v as usize).saturating_mul(1 << 10),
                }
                continue;
            }
            ["--dir", spec] => match spec.split_once('=') {
                Some(nd) => nd,
                None => return fail(format!("--dir {spec}: expected name=path")),
            },
            _ => return fail(
                "usage: kio run <prog.kio> [--dir name=path]... [--http name=host]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb]"
                    .into(),
            ),
        };
        match open_dir(dir.as_ref()) {
            Ok(d) => dirs.push((name.to_string(), d)),
            Err(e) => return fail(format!("--dir {name}={dir}: {e}")),
        }
    }
    let mut kio = match Kio::new(limits) {
        Ok(k) => k,
        Err(e) => return fail(format!("{e:#}")),
    };
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
            println!("{}\n{}", out.report.lines.join("\n"), out.value);
            // `main` returning `Err` is a failed run, like a nonzero exit from any Unix tool.
            i32::from(out.failed)
        }
        Ok(Err(diags)) => print_diags(&files, diags),
    }
}

fn print_diags(files: &[(String, String)], diags: Vec<Diag>) -> i32 {
    // Byte offset of each line start, one table per file: per-diag scans are quadratic in the source.
    let starts: Vec<Vec<usize>> = files
        .iter()
        .map(|(_, src)| {
            std::iter::once(0)
                .chain(src.match_indices('\n').map(|(i, _)| i + 1))
                .collect()
        })
        .collect();
    for d in diags {
        // The guest is untrusted: any file number or offset maps to some line, never a panic.
        let file = (d.file as usize).min(files.len() - 1);
        let line = starts[file].partition_point(|&s| s <= d.start as usize).max(1);
        eprintln!("{}:{line}: error: {}", files[file].0, d.message);
        if let Some(h) = d.hint {
            eprintln!("  hint: {h}");
        }
    }
    1
}

fn fail(msg: String) -> i32 {
    eprintln!("kio: {msg}");
    2
}
