use host::Kio;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check", path] => std::process::exit(check(path)),
        ["run", ..] => eprintln!("kio run: not implemented yet"),
        _ => eprintln!("usage: kio check <prog.kio>"),
    }
    std::process::exit(2);
}

fn check(path: &str) -> i32 {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return fail(format!("{path}: cannot read: {e}")),
    };
    match Kio::new().and_then(|mut k| k.check(&src)) {
        Err(e) => fail(format!("{e:#}")),
        Ok(Ok(report)) => {
            println!("{}", report.lines.join("\n"));
            0
        }
        Ok(Err(diags)) => {
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
    }
}

fn fail(msg: String) -> i32 {
    eprintln!("kio: {msg}");
    2
}
