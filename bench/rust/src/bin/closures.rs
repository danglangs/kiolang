fn main() {
    let mut xs: Vec<i64> = Vec::new();
    for i in 0..std::hint::black_box(1_000_000) {
        xs.push(i);
    }
    let k = 3;
    let ys: Vec<i64> = xs.iter().map(|x| x * 2).filter(|x| x % k == 0).collect();
    let s: i64 = ys.iter().sum();
    println!("{}", s + ys.len() as i64);
}
