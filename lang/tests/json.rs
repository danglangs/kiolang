//! Built-in JSON (L9): kio-web's tests/json_test.kio ported to `Json`, `parse_json` and
//! `to_json`, and the checker's rules for them. The differential test runs these programs
//! compiled too.

mod common;

use lang::{check, run, CapTable, Caps, RunErr, Value};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

fn exec(src: &str) -> Result<Value, RunErr> {
    let p = check(src, &CapTable::v0()).unwrap_or_else(|d| panic!("{d:?}"));
    common::big_stack(move || run(&p, &mut NoCaps))
}

fn value(src: &str) -> Value {
    exec(src).unwrap()
}

fn str_of(src: &str) -> String {
    match value(src) {
        Value::Str(s) => s.to_string(),
        v => panic!("not a Str: {v:?}"),
    }
}

/// The checker's message and hint for `src`.
fn rejects(src: &str) -> (String, Option<String>) {
    let d = check(src, &CapTable::v0()).unwrap_err();
    (d[0].msg.clone(), d[0].hint.clone())
}

/// kio-web's json_test.kio, each `json::` call a built-in, and the depth limit 128 for 64.
/// Returns 0 when every check passes, else the number of the first that fails.
const JSON_TEST: &str = r#"
// The encoding of `s` parsed, or its error message.
fn norm(s: Str) -> Str {
    match s.parse_json() {
        Ok(j) => j.to_json(),
        Err(e) => e,
    }
}

// A one-byte string.
fn ch(c: Int) -> Str {
    let mut b = "".to_bytes();
    b.push(c);
    b.to_str()
}

// The parsed value, or Null on an error.
fn doc(s: Str) -> Json {
    match s.parse_json() {
        Ok(j) => j,
        Err(_) => Json::Null,
    }
}

// 1 for a Float, 2 for a Num, 0 otherwise.
fn is_float(s: Str) -> Int {
    match doc(s) {
        Json::Float(_) => 1,
        Json::Num(_) => 2,
        _ => 0,
    }
}

fn flt(s: Str) -> Float {
    doc(s).as_float().unwrap_or(-1.0)
}

fn nest(n: Int) -> Str {
    let mut s = "";
    for i in 0..n { s = s + "["; }
    for i in 0..n { s = s + "]"; }
    s
}

fn main() -> Int {
    // literals and whitespace.
    if norm(" \t" + ch(13) + "\n null ") != "null" { return 1; }
    if norm("true") != "true" { return 2; }
    if norm("false") != "false" { return 3; }
    if norm("nul") != "expected 'null' at byte 0" { return 4; }
    if norm("[1, tru]") != "expected 'true' at byte 4" { return 5; }
    if norm("") != "expected a value at byte 0" { return 6; }
    // numbers.
    if norm("0") != "0" { return 7; }
    if norm("-12") != "-12" { return 8; }
    if norm("9223372036854775807") != "9223372036854775807" { return 9; }
    if norm("-9223372036854775808") != "-9223372036854775808" { return 10; }
    // too big for Int: a Float.
    if norm("9223372036854775808") != "9223372036854776000.0" { return 11; }
    if is_float("-9223372036854775809") != 1 { return 12; }
    if norm("1.5") != "1.5" { return 13; }
    if norm("2e3") != "2000.0" { return 14; }
    if norm("-") != "expected a digit at byte 1" { return 15; }
    if norm("01") != "expected end of input at byte 1" { return 16; }
    // strings and escapes.
    if norm("\"a\\\"\\\\\\/\\b\\f\\n\\r\\t\"") != "\"a\\\"\\\\/\\u0008\\u000c\\n\\u000d\\t\"" { return 17; }
    if norm("\"\\u00e9\"") != "\"é\"" { return 18; }
    if doc("\"\\u00e9\\u20ac\"").as_str().unwrap_or("") != "é€" { return 19; }
    if doc("\"\\ud83d\\ude00\"").as_str().unwrap_or("").len() != 4 { return 20; }
    if norm("\"\\ud83d\"") != "lone surrogate at byte 1" { return 21; }
    if norm("\"\\ud83dx\"") != "lone surrogate at byte 1" { return 22; }
    if norm("\"\\ude00\"") != "lone surrogate at byte 1" { return 23; }
    if norm("\"\\ud83d\\u0041\"") != "lone surrogate at byte 1" { return 24; }
    if norm("\"\\uzz00\"") != "expected 4 hex digits at byte 3" { return 25; }
    if norm("\"\\q\"") != "invalid escape at byte 1" { return 26; }
    if norm("\"abc") != "unterminated string starting at byte 0" { return 27; }
    if norm("\"a\nb\"") != "unescaped control character at byte 2" { return 28; }
    // arrays and objects.
    if norm("[]") != "[]" { return 29; }
    if norm("{}") != "{}" { return 30; }
    if norm(" [ 1 , [ 2 ] , { \"a\" : null } ] ") != "[1,[2],{\"a\":null}]" { return 31; }
    if norm("{\"b\":1,\"a\":2}") != "{\"a\":2,\"b\":1}" { return 32; }
    if norm("{\"a\":1,\"a\":2}") != "{\"a\":2}" { return 33; }
    if norm("[1 2]") != "expected ',' or ']' at byte 3" { return 34; }
    if norm("[1,") != "expected a value at byte 3" { return 35; }
    if norm("{\"a\" 1}") != "expected ':' at byte 5" { return 36; }
    if norm("{1:2}") != "expected a string key at byte 1" { return 37; }
    if norm("{\"a\":1 \"b\":2}") != "expected ',' or '}' at byte 7" { return 38; }
    if norm("{\"a\":1,}") != "expected a string key at byte 7" { return 39; }
    if norm("[1] x") != "expected end of input at byte 4" { return 40; }
    // depth: 128 nested arrays parse, 129 do not.
    if norm(nest(128)) != nest(128) { return 41; }
    if norm(nest(129)) != "nesting deeper than 128 at byte 128" { return 42; }
    // encoding escapes.
    let raw = Json::Str("q\"b\\n\nt\tc" + ch(1));
    if raw.to_json() != "\"q\\\"b\\\\n\\nt\\tc\\u0001\"" { return 43; }
    if doc(raw.to_json()).as_str().unwrap_or("") != raw.as_str().unwrap_or("x") { return 44; }
    // round trip of a mixed document.
    let text = "{\"a\":[1,-2,true,null,\"x\\ny\"],\"b\":{\"c\":\"€\"}}";
    if norm(text) != text { return 45; }
    // accessors.
    let j = doc("{\"n\":7,\"s\":\"hi\",\"t\":true,\"l\":[10,20]}");
    if j.get("n").unwrap_or(Json::Null).as_int().unwrap_or(0) != 7 { return 46; }
    if j.get("s").unwrap_or(Json::Null).as_str().unwrap_or("") != "hi" { return 47; }
    if !j.get("t").unwrap_or(Json::Null).as_bool().unwrap_or(false) { return 48; }
    let l = j.get("l").unwrap_or(Json::Null);
    if l.at(1).unwrap_or(Json::Null).as_int().unwrap_or(0) != 20 { return 49; }
    if l.as_list().unwrap_or([]).len() != 2 { return 50; }
    match l.at(2) { Some(_) => { return 51; } None => {} }
    match l.at(-1) { Some(_) => { return 52; } None => {} }
    match j.get("zz") { Some(_) => { return 53; } None => {} }
    match l.get("n") { Some(_) => { return 54; } None => {} }
    match j.as_int() { Some(_) => { return 55; } None => {} }
    match j.as_str() { Some(_) => { return 56; } None => {} }
    match j.as_bool() { Some(_) => { return 57; } None => {} }
    match j.as_list() { Some(_) => { return 58; } None => {} }
    // float grammar.
    if flt("0.5") != 0.5 || is_float("0.5") != 1 { return 59; }
    if flt("-1.5e3") != -1500.0 { return 60; }
    if flt("1E+2") != 100.0 || is_float("1E+2") != 1 { return 61; }
    if flt("1e-7") != 0.0000001 { return 62; }
    if is_float("1") != 2 || is_float("-0") != 2 || is_float("1.0") != 1 { return 63; }
    if norm("01") != "expected end of input at byte 1" { return 64; }
    if norm("1.") != "expected a digit at byte 2" { return 65; }
    if norm(".5") != "expected a value at byte 0" { return 66; }
    if norm("-") != "expected a digit at byte 1" { return 67; }
    if norm("-.5") != "expected a digit at byte 1" { return 68; }
    if norm("1e") != "expected a digit at byte 2" { return 69; }
    if norm("1e+") != "expected a digit at byte 3" { return 70; }
    if norm("0.5.5") != "expected end of input at byte 3" { return 71; }
    if norm("1e999") != "number out of range at byte 0" { return 72; }
    // encoding keeps Float distinct from Num.
    if norm("1.0") != "1.0" { return 73; }
    if norm("0.1") != "0.1" { return 74; }
    if norm("1e-7") != "0.0000001" { return 75; }
    if norm("[2.50,-0.0,1e2]") != "[2.5,-0.0,100.0]" { return 76; }
    let samples = ["0.1", "1.0", "-1.5e3", "1e300", "5e-324", "1.7976931348623157e308", "123456789.123456789", "9223372036854775808", "[0.5,{\"x\":-2.25e-5}]"];
    for k in 0..samples.len() {
        let a = doc(samples[k]);
        if doc(a.to_json()) != a { return 77; }
    }
    // NaN and infinity encode as null.
    if Json::Float(0.0 / 0.0).to_json() != "null" { return 78; }
    if Json::Float(1.0 / 0.0).to_json() != "null" { return 79; }
    if Json::Float(-1.0 / 0.0).to_json() != "null" { return 80; }
    if Json::Arr([Json::Float(1.5), Json::Float(1.0 / 0.0)]).to_json() != "[1.5,null]" { return 81; }
    // as_float accepts Float and Num only.
    if flt("7") != 7.0 { return 82; }
    match doc("\"1.5\"").as_float() { Some(_) => { return 83; } None => {} }
    match doc("1.5").as_int() { Some(_) => { return 84; } None => {} }
    0
}
"#;

#[test]
fn json_test_kio_ported() {
    assert_eq!(value(JSON_TEST), Value::Int(0));
}

#[test]
fn as_map_and_built_values() {
    let src = r#"fn main() -> Str {
        let mut m: Map<Str, Json> = Map::new();
        m.insert("k", Json::Arr([Json::Num(1), Json::Bool(false), Json::Null, Json::Str("é\0")]));
        m.insert("a", Json::Obj(Map::new()));
        let j = Json::Obj(m);
        let keys = j.as_map().unwrap_or(Map::new()).keys();
        j.to_json() + " " + keys.len().to_str() + " " + j.get("k").unwrap_or(Json::Null).at(3).unwrap_or(Json::Null).to_json()
    }"#;
    assert_eq!(str_of(src), r#"{"a":{},"k":[1,false,null,"é\u0000"]} 2 "é\u0000""#);
}

#[test]
fn errors_and_equality() {
    let src = r#"fn main() -> Bool {
        let a = "{\"x\": [1, 2.5, \"s\"], \"y\": null}".parse_json();
        let b = "{\"y\":null,\"x\":[1,2.5,\"s\"]}".parse_json();
        let bad = "[1, 2".parse_json();
        a == b && bad.err().unwrap_or("") == "expected ',' or ']' at byte 5" && a.is_ok()
    }"#;
    assert_eq!(value(src), Value::Bool(true));
}

#[test]
fn deep_values_encode() {
    // Built deeper than `parse_json` accepts: encoding has no depth limit.
    let src = r#"fn main() -> Int {
        let mut j = Json::Null;
        for i in 0..2000 { j = Json::Arr([j]); }
        let t = j.to_json();
        match t.parse_json() { Ok(_) => -1, Err(e) => t.len() + e.len() }
    }"#;
    // 4004 bytes of text (`null` inside), and "nesting deeper than 128 at byte 128" (35 bytes).
    assert_eq!(value(src), Value::Int(4004 + 35));
}

#[test]
fn objects_nest_to_the_limit() {
    let src = r#"fn main() -> Str {
        let mut s = "";
        for i in 0..128 { s = s + "{\"a\":"; }
        s = s + "1";
        for i in 0..128 { s = s + "}"; }
        let ok = match s.parse_json() { Ok(j) => j.to_json() == s, Err(_) => false };
        match ("{\"a\":" + s + "}").parse_json() { Ok(_) => "parsed", Err(e) => if ok { e } else { "lost" } }
    }"#;
    assert_eq!(str_of(src), "nesting deeper than 128 at byte 640");
}

#[test]
fn json_is_reserved() {
    let (msg, hint) = rejects("enum Json { A } fn main() {}");
    assert_eq!(msg, "`Json` is a built-in type");
    assert_eq!(hint.as_deref(), Some("rename your type"));
    let (msg, _) = rejects("struct Json { a: Int } fn main() {}");
    assert_eq!(msg, "`Json` is a built-in type");
}

#[test]
fn json_methods_are_typed() {
    let (msg, hint) = rejects("fn main() { let j = Json::Null; let x = j.get(1); }");
    assert_eq!(msg, "expected Str, found Int");
    assert_eq!(
        hint.as_deref(),
        Some("`Json.get` is declared as `get(Str) -> Option<Json>`")
    );
    let (msg, hint) = rejects("fn main() { let j = Json::Null; j.as_strr(); }");
    assert_eq!(msg, "no method `as_strr` on Json");
    assert!(hint.unwrap().contains("as_str"));
    let (msg, _) = rejects("fn main() { let x: Int = \"1\".parse_json(); }");
    assert_eq!(msg, "expected Int, found Result<Json, Str>");
}

#[test]
fn modules_share_the_one_json() {
    let api = "pub fn name(j: Json) -> Option<Str> { j.get(\"name\").and_then(|n| n.as_str()) }
               pub fn wrap(s: Str) -> Json { Json::Str(s) }";
    let app = "mod api; fn main() -> Str { api::name(\"{\\\"name\\\":\\\"kio\\\"}\".parse_json().unwrap_or(Json::Null)).unwrap_or(\"\") + api::wrap(\"x\").to_json() }";
    let p = lang::check_files(&[("app", app), ("api", api)], &CapTable::v0()).unwrap_or_else(|d| panic!("{d:?}"));
    let v = common::big_stack(move || run(&p, &mut NoCaps).unwrap());
    assert_eq!(v, Value::Str("kio\"x\"".into()));
    let own = [("app", "mod api; fn main() {}"), ("api", "pub enum Json { A }")];
    let d = lang::check_files(&own, &CapTable::v0()).unwrap_err();
    assert_eq!(d[0].msg, "`Json` is a built-in type");
}
