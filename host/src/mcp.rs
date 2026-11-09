//! `kio mcp`: an MCP server on stdio, one JSON-RPC message per line. It can teach and check,
//! never run: running needs grants, and grants are the operator's decision (open question 31).

use crate::locate;
use serde_json::{json, Value};
use std::io::BufRead;

const PRIMER: &str = include_str!("../../docs/language.md");
const VERSION: &str = "2025-06-18";
/// Source `kio_check` accepts per call, names and sources together: checking runs in the host,
/// outside any sandbox, so its input is bounded (with the checker's own budgets, open question 58).
const MAX_SOURCE: usize = 1 << 20;

/// Answer requests from stdin until EOF. A bad message gets an error reply, never an exit.
pub fn serve() -> i32 {
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&line) {
            println!("{reply}");
        }
    }
    0
}

/// The reply to one line, or `None` for a notification.
fn handle(line: &str) -> Option<Value> {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Some(error(Value::Null, -32700, "parse error"));
    };
    let id = msg.get("id")?.clone();
    let params = msg.get("params").unwrap_or(&Value::Null);
    let result = match msg.get("method").and_then(Value::as_str) {
        Some("initialize") => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "kio", "version": env!("CARGO_PKG_VERSION") },
        })),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({ "tools": tools() })),
        Some("tools/call") => call(params),
        _ => return Some(error(id, -32601, "method not found")),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(m) => error(id, -32602, m),
    })
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Value {
    json!([
        {
            "name": "kio_primer",
            "description": "The kio language guide: syntax, types and the rules the checker enforces. Read it before writing kio.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "kio_check",
            "description": "Check kio source in a sandbox. Returns the authority report, or errors as a JSON array of {file, line, message, hint}.",
            "inputSchema": {
                "type": "object",
                "properties": { "files": {
                    "type": "array",
                    "description": "The root file first, then the files it loads with `mod`.",
                    "items": {
                        "type": "object",
                        "properties": { "name": { "type": "string" }, "source": { "type": "string" } },
                        "required": ["name", "source"],
                    },
                } },
                "required": ["files"],
            },
        },
    ])
}

/// A `tools/call` result, or the error message for an unknown tool or bad arguments.
fn call(params: &Value) -> Result<Value, &'static str> {
    let bad = "expected a known tool name and valid arguments";
    let (text, is_error) = match params.get("name").and_then(Value::as_str) {
        Some("kio_primer") => (PRIMER.to_string(), false),
        Some("kio_check") => {
            let list = (params.get("arguments"))
                .and_then(|a| a.get("files")?.as_array())
                .ok_or(bad)?;
            check(list).ok_or(bad)??
        }
        _ => return Err(bad),
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

/// Check in the host, as `kio check` does. Returns the text and whether it is an error; `None`
/// for bad arguments, `Some(Err)` for files over `MAX_SOURCE`.
fn check(list: &[Value]) -> Option<Result<(String, bool), &'static str>> {
    let files = list
        .iter()
        .map(|f| {
            Some((
                f.get("name")?.as_str()?.to_string(),
                f.get("source")?.as_str()?.to_string(),
            ))
        })
        .collect::<Option<Vec<_>>>()
        .filter(|f| !f.is_empty())?;
    if files.iter().map(|(n, s)| n.len() + s.len()).sum::<usize>() > MAX_SOURCE {
        return Some(Err("kio_check takes at most 1 MB of source per call"));
    }
    Some(Ok(match kio_host::check_files(&files) {
        Ok(report) => (report.lines.join("\n"), false),
        Err(diags) => {
            let found: Vec<Value> = diags
                .iter()
                .zip(locate(&files, &diags))
                .map(|(d, (f, line))| json!({ "file": files[f].0, "line": line, "message": d.message, "hint": d.hint }))
                .collect();
            (Value::Array(found).to_string(), true)
        }
    }))
}
