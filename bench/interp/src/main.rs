//! `kio-interp prog.kio [--dir name=path]...`: run a program in the native interpreter
//! (`kio_host::Interp`, the test oracle) with no fuel, memory or stack limit, and print its value.
//! `mod m;` in the root loads `m.kio` beside it, as `kio run` does.
//! `kio-interp --names prog.kio` prints the lowered functions' names in order instead (codegen
//! emits them in this order after the runtime's functions; bench/profile.py names frames by it).
//! `kio-interp --native OUT prog.kio` writes the program's native code, as `kio run` compiles it,
//! to `OUT` (an ELF image; bench/profile.py reads its symbols).

use kio_host::{open_dir, Interp, Kio, Limits};
use std::path::Path;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let names = args[0] == "--names";
    if names {
        args.remove(0);
    }
    let native = (args[0] == "--native").then(|| {
        args.remove(0);
        args.remove(0)
    });
    let root = Path::new(&args[0]);
    let src = std::fs::read_to_string(root).expect("read program");
    let mut files = vec![(args[0].clone(), src.clone())];
    for line in src.lines() {
        if let Some(m) = line.trim().strip_prefix("mod ").and_then(|l| l.strip_suffix(';')) {
            let path = root.with_file_name(format!("{m}.kio"));
            files.push((format!("{m}.kio"), std::fs::read_to_string(path).expect("read module")));
        }
    }
    if let Some(out) = native {
        let kio = Kio::new(Limits::default()).unwrap();
        let (_, code) = kio.wasm_and_native(&files).expect("compile");
        std::fs::write(out, code).expect("write native code");
        return;
    }
    if names {
        let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        let caps = lang::CapTable::v0();
        let program = lang::check_files(&pairs, &caps).unwrap_or_else(|_| panic!("check failed"));
        for f in lang::lower::lower(&program, &caps).funcs {
            println!("{}", f.name);
        }
        return;
    }
    let mut dirs = vec![];
    for pair in args[1..].chunks(2) {
        assert_eq!(pair[0], "--dir");
        let (name, path) = pair[1].split_once('=').expect("--dir name=path");
        dirs.push((name.to_string(), open_dir(Path::new(path)).expect("open dir")));
    }
    let limits = Limits {
        timeout_secs: 3600,
        ..Limits::default()
    };
    match Interp::new(limits).unwrap().run_files(&files, dirs, vec![]) {
        Ok(Ok(out)) => println!("{}", out.value),
        Ok(Err(_)) => {
            eprintln!("check or run error");
            std::process::exit(1)
        }
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1)
        }
    }
}
