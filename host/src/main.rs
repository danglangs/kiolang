use host::{open_dir, Diag, Kio, Limits};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check", path] => std::process::exit(check(path)),
        ["run", path, ref flags @ ..] => std::process::exit(run(path, flags)),
        _ => eprintln!(
            "usage: kio check <prog.kio>\n       kio run <prog.kio> [--dir name=path]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb]"
        ),
    }
    std::process::exit(2);
}

fn read_source(path: &str) -> Result<String, i32> {
    std::fs::read_to_string(path).map_err(|e| fail(format!("{path}: cannot read: {e}")))
}

fn check(path: &str) -> i32 {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    match Kio::new(Limits::default()).and_then(|mut k| k.check(&src)) {
        Err(e) => fail(format!("{e:#}")),
        Ok(Ok(report)) => {
            println!("{}", report.lines.join("\n"));
            0
        }
        Ok(Err(diags)) => print_diags(path, &src, diags),
    }
}

fn run(path: &str, flags: &[&str]) -> i32 {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let mut dirs = Vec::new();
    let mut limits = Limits::default();
    for pair in flags.chunks(2) {
        let (name, dir) = match pair {
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
                "usage: kio run <prog.kio> [--dir name=path]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb]"
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
    match kio.run(&src, dirs) {
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
        Ok(Err(diags)) => print_diags(path, &src, diags),
    }
}

fn print_diags(path: &str, src: &str, diags: Vec<Diag>) -> i32 {
    // Byte offset of each line start, computed once: per-diag scans are quadratic in the source.
    let starts: Vec<usize> = std::iter::once(0)
        .chain(src.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    for d in diags {
        // The guest is untrusted: any offset maps to some line, never a panic.
        let line = starts.partition_point(|&s| s <= d.start as usize).max(1);
        eprintln!("{path}:{line}: error: {}", d.message);
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
