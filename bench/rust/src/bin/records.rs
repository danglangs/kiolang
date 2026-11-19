// kio/records.kio: serde_json, then accessors over the records.
use std::collections::HashMap;
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let empty = vec![];
    let recs = doc.as_array().unwrap_or(&empty);
    let (mut active, mut score, mut tags) = (0i64, 0.0f64, 0i64);
    let mut zones: HashMap<String, i64> = HashMap::new();
    for r in recs {
        if r.get("active").and_then(|j| j.as_bool()).unwrap_or(false) {
            active += 1;
        }
        score += r.get("score").and_then(|j| j.as_f64()).unwrap_or(0.0);
        tags += r.get("tags").and_then(|j| j.as_array()).map_or(0, |a| a.len()) as i64;
        if let Some(m) = r.get("meta") {
            let z = m.get("zone").and_then(|j| j.as_str()).unwrap_or("");
            let rank = m.get("rank").and_then(|j| j.as_i64()).unwrap_or(0);
            if rank > 100 {
                *zones.entry(z.to_string()).or_insert(0) += 1;
            }
        }
    }
    println!(
        "{}",
        active * 1000000000 + tags * 100000 + zones.len() as i64 + (score as i64) % 1000
    );
}
