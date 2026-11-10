//! Front-end fuzzing: the host checks adversarial source outside the sandbox, so
//! `lang::check_files` must return `Ok` or `Err` on any input, never panic or overflow its stack.
//! `KIO_FUZZ_N` inputs (default 2,000, at least one per corpus program), each a corpus program
//! with one to three mutations, from seed `KIO_FUZZ_SEED`. A panic prints the input; a stack
//! overflow aborts the process, so each input is first written to `fuzz_front_last.kio` under
//! Cargo's test tmpdir.

use kio_host::MAX_STACK;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

#[path = "../../lang/tests/common/corpus.rs"]
mod corpus;

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// `src` split into token-sized spans: a run of word characters, of whitespace, or one other
/// character.
fn tokens(src: &str) -> Vec<String> {
    let class = |c: char| match c {
        c if c.is_alphanumeric() || c == '_' => 0,
        c if c.is_whitespace() => 1,
        _ => 2,
    };
    let mut out: Vec<String> = Vec::new();
    let mut prev = None;
    for c in src.chars() {
        let k = class(c);
        match out.last_mut() {
            Some(t) if prev == Some(k) && k != 2 => t.push(c),
            _ => out.push(c.to_string()),
        }
        prev = Some(k);
    }
    out
}

const INSERTS: &[&str] = &[
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    "<",
    ">",
    ";",
    ",",
    ":",
    "::",
    ".",
    "..",
    "?",
    "!",
    "-",
    "=",
    "+=",
    "==",
    "&&",
    "|",
    "&",
    "&mut ",
    "#",
    "#[",
    "'",
    "'a",
    "/*",
    "as",
    "println!(",
    "\"",
    "\\",
    "/",
    "//",
    "fn",
    "let",
    "mut",
    "if",
    "else",
    "match",
    "for",
    "in",
    "while",
    "loop",
    "return",
    "break",
    "continue",
    "struct",
    "enum",
    "mod",
    "pub",
    "impl",
    "self",
    "Self",
    "main",
    "Int",
    "Str",
    "List",
    "Map",
    "Option",
    "Result",
    "Dir",
    "Some",
    "None",
    "Ok",
    "Err",
    "_",
    "x",
    "0",
    "-1",
    "9223372036854775807",
    "9223372036854775808",
    "Float",
    "1.5",
    "1e309",
    "\"s\"",
    "\"\\u{0}\"",
    "é",
    "\u{0}",
    "\u{feff}",
    "\n",
];

/// One random mutation of `src`.
fn mutate(rng: &mut Rng, src: &str) -> String {
    let mut t = tokens(src);
    if t.is_empty() {
        t.push(String::new());
    }
    let n = t.len();
    let at = rng.below(n);
    let len = 1 + rng.below(4.min(n - at));
    match rng.below(6) {
        0 => {
            t.drain(at..at + len);
        }
        1 => {
            let span: Vec<String> = t[at..at + len].to_vec();
            t.splice(at..at, span);
        }
        2 => {
            let other = rng.below(n);
            t.swap(at, other);
        }
        3 | 4 => {
            for _ in 0..1 + rng.below(3) {
                let i = rng.below(t.len() + 1);
                t.insert(i, INSERTS[rng.below(INSERTS.len())].to_string());
            }
        }
        _ => {
            let (open, close) = [("(", ")"), ("{", "}"), ("[", "]"), ("-", ""), ("!", ""), ("if x ", "")][rng.below(6)];
            let depth = [10, 100, 1_000, 100_000][rng.below(4)];
            let close = if rng.below(2) == 0 { close } else { "" };
            t.insert(at, close.repeat(depth));
            t.insert(at, open.repeat(depth));
        }
    }
    t.concat()
}

fn fuzz() {
    let env = |name: &str, default: usize| std::env::var(name).map_or(default, |v| v.parse().expect(name));
    let mut corpus = corpus::sources(Path::new("../"));
    corpus.sort();
    corpus.dedup();
    let n = env("KIO_FUZZ_N", 2_000).max(corpus.len());
    let seed = env("KIO_FUZZ_SEED", 0x006b_696f) as u64;
    let mut rng = Rng(seed | 1);
    let last = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fuzz_front_last.kio");
    let caps = lang::CapTable::v0();
    let mut panics = Vec::new();
    for i in 0..n {
        let mut files = corpus[i % corpus.len()].clone();
        let f = rng.below(files.len());
        for _ in 0..1 + rng.below(3) {
            files[f].1 = mutate(&mut rng, &files[f].1);
        }
        std::fs::write(&last, &files[f].1).unwrap();
        let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        if catch_unwind(AssertUnwindSafe(|| lang::check_files(&pairs, &caps))).is_err() {
            panics.push(format!(
                "input {i} of seed {seed}, file {}:\n{}",
                files[f].0, files[f].1
            ));
        }
    }
    println!(
        "{n} mutated inputs from {} corpus programs from seed {seed}",
        corpus.len()
    );
    assert!(
        panics.is_empty(),
        "{} panics:\n{}",
        panics.len(),
        panics.join("\n----\n")
    );
}

#[test]
fn check_never_panics_on_mutated_programs() {
    // The stack `kio` checks on (host/src/main.rs).
    let t = std::thread::Builder::new().stack_size(4 * MAX_STACK);
    t.spawn(fuzz).unwrap().join().unwrap();
}
