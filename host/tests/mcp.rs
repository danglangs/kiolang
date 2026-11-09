use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// Send each line to `kio mcp`, close stdin, and return the reply lines.
fn session(lines: &[String]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for l in lines {
        writeln!(stdin, "{l}").unwrap();
    }
    drop(stdin); // EOF: the server must exit cleanly.
    let out = BufReader::new(child.stdout.take().unwrap());
    let replies = out
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    assert!(child.wait().unwrap().success());
    replies
}

fn req(id: u32, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

fn check_call(id: u32, source: &str) -> String {
    let files = json!([{ "name": "a.kio", "source": source }]);
    req(
        id,
        "tools/call",
        json!({ "name": "kio_check", "arguments": { "files": files } }),
    )
}

fn text(reply: &Value) -> &str {
    reply["result"]["content"][0]["text"].as_str().unwrap()
}

#[test]
fn mcp_session() {
    let replies = session(&[
        req(1, "initialize", json!({ "protocolVersion": "2024-11-05" })),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        req(2, "tools/list", json!({})),
        req(3, "tools/call", json!({ "name": "kio_primer", "arguments": {} })),
        check_call(4, "fn main() -> Int {\n    let x = 1;\n    x = 2;\n    x\n}\n"),
        check_call(5, "fn main() -> Int { 1 }\n"),
        "{not json".into(),
        req(6, "ping", json!({})),
        req(7, "nope", json!({})),
        req(8, "tools/call", json!({ "name": "kio_check", "arguments": {} })),
        req(9, "tools/call", json!({ "name": "kio_run", "arguments": {} })),
    ]);
    // The notification gets no reply: ten requests, ten replies.
    assert_eq!(replies.len(), 10);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "kio");

    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["kio_primer", "kio_check"]);
    assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));

    assert!(text(&replies[2]).contains("Rules the checker enforces"));

    let bad = &replies[3]["result"];
    assert_eq!(bad["isError"], true);
    let diags: Value = serde_json::from_str(text(&replies[3])).unwrap();
    assert_eq!(diags[0]["file"], "a.kio");
    assert_eq!(diags[0]["line"], 3);
    assert!(diags[0]["message"].as_str().unwrap().contains("not `mut`"));
    assert_eq!(diags[0]["hint"], "declare it with `let mut`");

    assert_eq!(replies[4]["result"]["isError"], false);
    assert!(text(&replies[4]).contains("main"));

    // Malformed JSON: parse error, then the server keeps answering.
    assert_eq!(replies[5]["error"]["code"], -32700);
    assert_eq!(replies[5]["id"], Value::Null);
    assert_eq!(replies[6]["result"], json!({}));
    assert_eq!(replies[7]["error"]["code"], -32601);
    assert_eq!(replies[8]["error"]["code"], -32602);
    assert_eq!(replies[9]["error"]["code"], -32602);
}

/// Checking runs in the host, outside the sandbox: `kio_check` refuses more than 1 MB of source.
#[test]
fn kio_check_refuses_over_a_megabyte() {
    let pad = format!("fn main() {{}}\n//{}\n", "x".repeat(1 << 20));
    let replies = session(&[check_call(1, &pad), check_call(2, &pad[..(1 << 20) - 5])]);
    assert_eq!(replies[0]["error"]["code"], -32602);
    assert_eq!(
        replies[0]["error"]["message"],
        "kio_check takes at most 1 MB of source per call"
    );
    assert_eq!(replies[1]["result"]["isError"], false, "{}", replies[1]);
}
