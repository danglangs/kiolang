//! Built-in JSON, compiled against interpreted (L9, C12). Random `Json` values (depth at most 6;
//! strings with escapes and non-ASCII; `Int`s at the edges; floats such as 1e300, -0.0 and
//! 5e-324) are encoded by the interpreter's `lang::json::encode`; one program per batch parses
//! and re-encodes each text, compiled and interpreted, and both must give the text back. Then
//! random byte mutations of those texts: both paths give the same value or the same error
//! message and offset. `KIO_JSON_SEED` and `KIO_JSON_N` (batches, default 10) vary the run.

use kio_host::{Interp, Kio, Limits, MAX_STACK};
use lang::ir::{EnumId, EnumRef};
use lang::{Key, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

fn limits() -> Limits {
    Limits {
        fuel: 20_000_000_000,
        ..Limits::default()
    }
}

/// The displayed result, or the error message.
fn compiled(src: &str) -> String {
    match Kio::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(o) => o.value,
        Err(d) => format!("error: {}", d[0].message),
    }
}

fn interpreted(src: &str) -> String {
    match Interp::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(o) => o.value,
        Err(d) => format!("error: {}", d[0].message),
    }
}

/// xorshift64: the same cases on every run of a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

fn json(v: usize, payload: Vec<Value>) -> Value {
    Value::Enum(EnumRef::User(EnumId(0)), v, payload)
}

fn text(r: &mut Rng) -> String {
    let parts = [
        "a",
        "key",
        " ",
        "\"",
        "\\",
        "/",
        "\n",
        "\t",
        "\r",
        "\u{0}",
        "\u{1f}",
        "\u{7f}",
        "é",
        "€",
        "😀",
        "\u{2028}",
        "\\u0041",
        "0123456789",
    ];
    (0..r.below(6)).map(|_| *r.pick(&parts)).collect()
}

fn float(r: &mut Rng) -> f64 {
    let edges = [
        0.0,
        -0.0,
        1e300,
        -1e300,
        5e-324,
        f64::MAX,
        f64::MIN_POSITIVE,
        0.1,
        1.5,
        9_223_372_036_854_775_808.0,
        123_456_789.123_456_79,
    ];
    match r.below(3) {
        0 => *r.pick(&edges),
        1 => (r.next() as i64 as f64) / 1e6,
        _ => loop {
            let x = f64::from_bits(r.next());
            if x.is_finite() {
                break x;
            }
        },
    }
}

/// A random `Json` value nested at most `depth` more levels.
fn value(r: &mut Rng, depth: u32) -> Value {
    let kinds = if depth == 0 { 5 } else { 7 };
    match r.below(kinds) {
        0 => json(0, vec![]),
        1 => json(1, vec![Value::Bool(r.below(2) == 1)]),
        2 => {
            let edges = [i64::MIN, i64::MAX, i64::MIN + 1, i64::MAX - 1, 0, -1, 1];
            let n = if r.below(2) == 0 {
                *r.pick(&edges)
            } else {
                r.next() as i64 >> r.below(64)
            };
            json(2, vec![Value::Int(n)])
        }
        3 => json(3, vec![Value::Float(float(r))]),
        4 => json(4, vec![Value::Str(text(r).into())]),
        5 => json(
            5,
            vec![Value::List(Rc::new(
                (0..r.below(5)).map(|_| value(r, depth - 1)).collect(),
            ))],
        ),
        _ => {
            let m: BTreeMap<Key, Value> = (0..r.below(5))
                .map(|_| (Key::Str(text(r)), value(r, depth - 1)))
                .collect();
            json(6, vec![Value::Map(Rc::new(m))])
        }
    }
}

/// `s` as a kio string literal.
fn literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A program printing each input parsed and re-encoded (or its error), one per line.
fn program(inputs: &[String]) -> String {
    let items: Vec<String> = inputs.iter().map(|s| literal(s)).collect();
    format!(
        "fn norm(s: Str) -> Str {{ match s.parse_json() {{ Ok(j) => j.to_json(), Err(e) => \"E \" + e }} }}
         fn main() -> Str {{
             let mut out = \"\";
             for x in [{}] {{ out = out + norm(x) + \"|\"; }}
             out
         }}",
        items.join(", ")
    )
}

/// Runs `inputs` both ways; they must agree, and the output is returned split per input.
fn both(inputs: &[String]) -> Vec<String> {
    let src = program(inputs);
    let (c, i) = (compiled(&src), interpreted(&src));
    if c != i {
        let (c, i): (Vec<&str>, Vec<&str>) = (c.split('|').collect(), i.split('|').collect());
        let k = (0..c.len().min(i.len())).find(|&k| c[k] != i[k]).unwrap_or(0);
        panic!(
            "input {:?}\ncompiled:    {:?}\ninterpreted: {:?}",
            inputs.get(k),
            c.get(k),
            i.get(k)
        );
    }
    let c = c.strip_suffix('|').expect("one line per input");
    c.split('|').map(str::to_string).collect()
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).map_or(default, |v| v.parse().expect(name))
}

fn fuzz() {
    let seed = env("KIO_JSON_SEED", 0x6a73_6f6e);
    let batches = env("KIO_JSON_N", 10);
    let mut r = Rng(seed | 1);
    let (mut docs, mut oks, mut errs) = (0, 0, 0);
    for _ in 0..batches {
        let texts: Vec<String> = (0..60).map(|_| lang::json::encode(&value(&mut r, 6))).collect();
        let out = both(&texts);
        for (t, o) in texts.iter().zip(&out) {
            assert_eq!(o, t, "parse(to_json(j)) re-encodes to the same text");
        }
        docs += texts.len();
        let alphabet = [
            "{", "}", "[", "]", "\"", ":", ",", ".", "-", "+", "e", "0", "9", "\\", "u", "d83d", " ", "\n", "é", "n",
            "t",
        ];
        let mut mutated = Vec::new();
        for t in &texts {
            for _ in 0..4 {
                let mut s = t.clone();
                for _ in 0..1 + r.below(3) {
                    let at = r.below(s.len() + 1);
                    if !s.is_char_boundary(at) {
                        continue;
                    }
                    match r.below(3) {
                        0 if at < s.len() => {
                            let end = (at + 1..=s.len()).find(|&e| s.is_char_boundary(e)).unwrap();
                            s.replace_range(at..end, "");
                        }
                        _ => s.insert_str(at, r.pick(&alphabet)),
                    }
                }
                mutated.push(s);
            }
        }
        for o in both(&mutated) {
            match o.starts_with("E ") {
                true => errs += 1,
                false => oks += 1,
            }
        }
    }
    println!("{docs} generated documents round-tripped; mutations: {oks} parsed, {errs} errors, all agreeing");
    assert!(errs > 0 && oks > 0);
}

#[test]
fn generated_json_agrees() {
    // Wasm runs on this thread's stack, so it needs room above `--stack`, as `kio` gives it.
    let t = std::thread::Builder::new().stack_size(4 * MAX_STACK);
    t.spawn(fuzz).unwrap().join().unwrap();
}
