// kio's `Map` is ordered; idiomatic Rust word count uses a `HashMap`.
use std::collections::HashMap;

fn main() {
    let path = std::env::args().nth(1).expect("path to words.txt");
    let text = std::fs::read_to_string(path).unwrap();
    let mut counts: HashMap<&str, i64> = HashMap::new();
    let mut total = 0;
    for line in text.lines() {
        for w in line.split(' ') {
            if !w.is_empty() {
                *counts.entry(w).or_insert(0) += 1;
                total += 1;
            }
        }
    }
    let most = counts.values().copied().max().unwrap_or(0);
    println!("{}", counts.len() as i64 * 1_000_000_000 + most * 1_000_000 + total);
}
