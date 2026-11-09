//! Wall time of a program compiled (`Kio::run`) and interpreted natively (`Interp::run`), 3 runs each.
//! `cargo run --release -p kio --example bench_compiled -- prog.kio...`
//! A compiled run includes compiling; `fn main() -> Int { 0 }` measures that alone.

use kio_host::{Interp, Kio, Limits};
use std::time::Instant;

fn main() {
    let limits = Limits {
        fuel: u64::MAX,
        timeout_secs: 600,
        ..Limits::default()
    };
    let mut progs: Vec<(String, String)> = vec![("empty".into(), "fn main() -> Int { 0 }".into())];
    for p in std::env::args().skip(1) {
        let src = std::fs::read_to_string(&p).unwrap();
        progs.push((p, src));
    }
    for (name, src) in &progs {
        for _ in 0..3 {
            let t = Instant::now();
            let c = Kio::new(limits).unwrap().run(src, vec![]).unwrap().unwrap().value;
            let tc = t.elapsed();
            let t = Instant::now();
            let i = Interp::new(limits).unwrap().run(src, vec![]).unwrap().unwrap().value;
            let ti = t.elapsed();
            assert_eq!(c, i);
            println!("{name}: compiled {tc:.3?}, interpreted {ti:.3?} ({c})");
        }
    }
}
