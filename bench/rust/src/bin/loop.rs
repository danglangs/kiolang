fn main() {
    let n: i64 = std::hint::black_box(30_000_000);
    let mut s: i64 = 0;
    for i in 0..n {
        s = (s * 31 + i) % 1_000_003;
    }
    println!("{s}");
}
