use host::{open_dir, Diag, Kio, Limits};
use wasmtime::Trap;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check", path] => std::process::exit(check(path)),
        ["run", path, ref flags @ ..] => std::process::exit(run(path, flags)),
        _ => eprintln!(
            "usage: kio check <prog.kio>\n       kio run <prog.kio> [--dir name=path]... [--fuel n] [--timeout secs]"
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
            ["--fuel", n] | ["--timeout", n] => {
                let Some(v) = n.parse::<u64>().ok().filter(|&v| v > 0) else {
                    return fail(format!("{} {n}: expected a positive integer", pair[0]));
                };
                match pair[0] {
                    "--fuel" => limits.fuel = v,
                    _ => limits.timeout_secs = v,
                }
                continue;
            }
            ["--dir", spec] => match spec.split_once('=') {
                Some(nd) => nd,
                None => return fail(format!("--dir {spec}: expected name=path")),
            },
            _ => return fail("usage: kio run <prog.kio> [--dir name=path]... [--fuel n] [--timeout secs]".into()),
        };
        match open_dir(dir.as_ref()) {
            Ok(d) => dirs.push((name.to_string(), d)),
            Err(e) => return fail(format!("--dir {name}={dir}: {e}")),
        }
    }
    match Kio::new(limits).and_then(|mut k| k.run(&src, dirs)) {
        Err(e) => {
            // A trap is the guest failing, not a usage error.
            if let Some(msg) = limits.explain(&e) {
                eprintln!("{msg}");
            } else if e.downcast_ref::<Trap>() == Some(&Trap::StackOverflow) {
                eprintln!("stack overflow");
            } else {
                eprintln!("kio: {e:#}");
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
    for d in diags {
        // The guest is untrusted: a bad offset gives line 1, never a panic.
        let line = src.get(..d.start as usize).map_or(1, |s| s.lines().count().max(1));
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
