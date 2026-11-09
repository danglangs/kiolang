// Idiomatic Rust appends in place (`push_str`), where kio's `s = s + ..` builds a new string.
fn main() {
    let mut s = String::new();
    for i in 0..std::hint::black_box(100_000i64) {
        s.push_str(&i.to_string());
        s.push(',');
    }
    println!("{}", s.len());
}
