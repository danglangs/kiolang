// kio/logscan.kio: a character-by-character `key=value` parser, counts and sums per route.
use std::collections::HashMap;
fn route(path: &str) -> String {
    let mut out = String::new();
    let mut seg = String::new();
    for c in path.chars() {
        if c == '/' {
            if !seg.is_empty() && seg.parse::<i64>().is_err() {
                out.push('/');
                out.push_str(&seg);
            }
            seg.clear();
        } else {
            seg.push(c);
        }
    }
    if !seg.is_empty() && seg.parse::<i64>().is_err() {
        out.push('/');
        out.push_str(&seg);
    }
    out
}
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let mut statuses: HashMap<String, i64> = HashMap::new();
    let mut dur: HashMap<String, i64> = HashMap::new();
    let mut errors = 0i64;
    for line in text.lines() {
        let (mut key, mut val) = (String::new(), String::new());
        let mut in_val = false;
        let (mut path, mut status) = (String::new(), String::new());
        let mut ms = 0i64;
        for c in line.chars().chain(std::iter::once(' ')) {
            if c == ' ' {
                if key == "path" {
                    path = std::mem::take(&mut val);
                } else if key == "status" {
                    status = std::mem::take(&mut val);
                } else if key == "dur_ms" {
                    ms = val.parse().unwrap_or(0);
                } else if key == "level" && val == "error" {
                    errors += 1;
                }
                key.clear();
                val.clear();
                in_val = false;
            } else if c == '=' && !in_val {
                in_val = true;
            } else if in_val {
                val.push(c);
            } else {
                key.push(c);
            }
        }
        *statuses.entry(status).or_insert(0) += 1;
        *dur.entry(route(&path)).or_insert(0) += ms;
    }
    let mut check = errors;
    let mut ks: Vec<_> = statuses.keys().collect();
    ks.sort();
    for k in ks {
        check = (check * 31 + statuses[k]) % 1000000007;
    }
    let mut ks: Vec<_> = dur.keys().collect();
    ks.sort();
    for k in ks {
        check = (check * 31 + dur[k] + k.len() as i64) % 1000000007;
    }
    println!("{}", check);
}
