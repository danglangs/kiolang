//! A kio program inserting and looking up `n * n` `Str` keys that share one hash
//! (`lang::hmap::colliding_parts`: every `a + b` collides), shared by the interpreter's and the
//! compiled path's tests (L10, C13). It returns `n * n * 10^12` plus the sum of `0..n * n`.

pub fn colliding_program(n: usize) -> String {
    let lit = |p: &[u8; 16]| {
        let s = std::str::from_utf8(p).unwrap();
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    };
    let parts = lang::hmap::colliding_parts(2, n);
    let list = |i: usize| parts[i].iter().map(lit).collect::<Vec<_>>().join(", ");
    format!(
        "fn main() -> Int {{
             let a = [{}];
             let b = [{}];
             let mut m: HashMap<Str, Int> = HashMap::new();
             let mut i = 0;
             for x in a {{ for y in b {{ m.insert(x + y, i); i += 1; }} }}
             let mut sum = 0;
             for x in a {{ for y in b {{ sum += m.get(x + y).unwrap_or(-1000000); }} }}
             m.len() * 1000000000000 + sum
         }}",
        list(0),
        list(1)
    )
}

/// What `colliding_program(n)` returns.
pub fn colliding_result(n: i64) -> i64 {
    n * n * 1_000_000_000_000 + n * n * (n * n - 1) / 2
}
