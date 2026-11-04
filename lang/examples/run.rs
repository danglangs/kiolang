//! Temporary runner until the `kio` host exists: `cargo run -p lang --example run -- prog.kio`.
use lang::{check, run, CapTable, Caps};

struct NoCaps;
impl Caps for NoCaps {}

fn main() {
    let path = std::env::args().nth(1).expect("usage: run <prog.kio>");
    let src = std::fs::read_to_string(&path).expect("cannot read file");
    let line = |at: usize| src[..at].lines().count().max(1);
    match check(&src, &CapTable::default()) {
        Err(diags) => {
            for d in diags {
                eprintln!("{path}:{}: error: {}", line(d.span.start), d.msg);
            }
            std::process::exit(1);
        }
        Ok(p) => match run(&p, &mut NoCaps) {
            Ok(v) => println!("{v:?}"),
            Err(e) => {
                eprintln!("{path}:{}: runtime error: {}", line(e.span.start), e.msg);
                std::process::exit(1);
            }
        },
    }
}
