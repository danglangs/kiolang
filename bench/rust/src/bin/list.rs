// The benchmark sums once by index and once by iterator, as kio's does.
#[allow(clippy::needless_range_loop)]
fn main() {
    let n: i64 = std::hint::black_box(1_000_000);
    let mut xs: Vec<i64> = Vec::new();
    let mut x: i64 = 12345;
    for i in 0..n {
        x = (x * 1103515245 + 12345 + i) % 2147483648;
        xs.push(x);
    }
    xs.sort();
    let mut a = 0;
    for i in 0..xs.len() {
        a += xs[i] % 1000;
    }
    let mut b = 0;
    for v in &xs {
        b += v % 7;
    }
    println!("{}", a + b + xs[(n / 2) as usize]);
}
