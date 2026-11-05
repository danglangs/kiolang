//! Temporary runner until the `kio` host exists: `cargo run -p lang --example run -- prog.kio`.
use lang::{check, report, run, CapTable, Caps, Value};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: run <prog.kio>");
    let src = std::fs::read_to_string(&path).expect("cannot read file");
    let line = |at: usize| src[..at].lines().count().max(1);
    match check(&src, &CapTable::v0()) {
        Err(diags) => {
            for d in diags {
                eprintln!("{path}:{}: error: {}", line(d.span.start), d.msg);
                if let Some(h) = d.hint {
                    eprintln!("  hint: {h}");
                }
            }
            std::process::exit(1);
        }
        Ok(p) => {
            println!("{}\n", report(&p));
            match run(&p, &mut NoCaps) {
                Ok(v) => println!("{}", p.display(&v)),
                Err(e) => {
                    eprintln!("{path}:{}: runtime error: {}", line(e.span.start), e.msg);
                    std::process::exit(1);
                }
            }
        }
    }
}
