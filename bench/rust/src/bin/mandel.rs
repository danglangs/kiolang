fn main() {
    let n: i64 = std::hint::black_box(400);
    let max = 100;
    let mut inside: i64 = 0;
    let mut iters: i64 = 0;
    for py in 0..n {
        for px in 0..n {
            let cx = -2.0 + 2.5 * px as f64 / n as f64;
            let cy = -1.25 + 2.5 * py as f64 / n as f64;
            let (mut x, mut y) = (0.0f64, 0.0f64);
            let mut i = 0;
            while i < max && x * x + y * y <= 4.0 {
                let t = x * x - y * y + cx;
                y = 2.0 * x * y + cy;
                x = t;
                i += 1;
            }
            iters += i;
            if i == max {
                inside += 1;
            }
        }
    }
    println!("{}", inside * 100_000_000 + iters);
}
