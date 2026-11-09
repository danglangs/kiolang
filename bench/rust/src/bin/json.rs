// serde_json into an untyped `Value`, as json.kio parses into `Json`.
fn main() {
    let path = std::env::args().nth(1).expect("path to data.json");
    let text = std::fs::read_to_string(path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    println!("{}", serde_json::to_string(&v).unwrap().len());
}
