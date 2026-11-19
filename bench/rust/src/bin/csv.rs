// kio/csv.kio: CSV lines into structs, an email check, totals per city.
use std::collections::HashMap;
#[allow(dead_code)]
struct Order {
    id: i64,
    name: String,
    email: String,
    city: String,
    cents: i64,
    qty: i64,
}
#[derive(Clone, Copy, Default)]
struct Agg {
    orders: i64,
    cents: i64,
    units: i64,
    biggest: i64,
}
fn valid_email(e: &str) -> bool {
    let parts: Vec<&str> = e.split('@').collect();
    parts.len() == 2 && !parts[0].is_empty() && parts[1].contains('.')
}
fn parse(line: &str) -> Option<Order> {
    let f: Vec<&str> = line.split(',').collect();
    if f.len() != 6 {
        return None;
    }
    let id = f[0].parse().ok()?;
    let cents = f[4].parse().ok()?;
    let qty: i64 = f[5].parse().ok()?;
    if !valid_email(f[2]) || qty <= 0 {
        return None;
    }
    Some(Order {
        id,
        name: f[1].trim().to_string(),
        email: f[2].to_lowercase(),
        city: f[3].to_string(),
        cents,
        qty,
    })
}
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let mut orders = Vec::new();
    let mut bad = 0i64;
    for line in text.lines().skip(1) {
        match parse(line) {
            Some(o) => orders.push(o),
            None => bad += 1,
        }
    }
    let mut by_city: HashMap<String, Agg> = HashMap::new();
    for o in &orders {
        let a = by_city.entry(o.city.clone()).or_default();
        a.orders += 1;
        a.cents += o.cents * o.qty;
        a.units += o.qty;
        a.biggest = a.biggest.max(o.cents);
    }
    let mut keys: Vec<&String> = by_city.keys().collect();
    keys.sort();
    let mut check = 0i64;
    for k in keys {
        let a = by_city[k];
        check = (check * 31 + a.cents % 1000003 + a.orders + a.units + a.biggest) % 1000000007;
    }
    println!("{}", check * 1000 + bad);
}
