//! `kio mcp`: an MCP server on stdio, one JSON-RPC message per line. It can teach and check,
//! never run: running needs grants, and grants are the operator's decision (open question 31).

use crate::locate;
use serde_json::{json, Value};
use std::io::{BufRead, Read};

/// Idioms first, then the reference: E4 measured 45/45 first try (43 with the reference alone)
/// and 18% lower cost (open question 218).
const PRIMER: &str = concat!(
    include_str!("../../docs/primer.md"),
    "\n\n",
    include_str!("../../docs/language.md")
);
const VERSION: &str = "2025-06-18";
/// Source `kio_check` accepts per call, names and sources together: checking runs in the host,
/// outside any sandbox, so its input is bounded (with the checker's own budgets, open question 58).
const MAX_SOURCE: usize = 1 << 20;
/// Bytes of one message, read no further: room for `MAX_SOURCE` escaped in JSON.
const MAX_MESSAGE: usize = 2 << 20;

/// Answer requests from stdin until EOF. A bad message gets an error reply, never an exit; a
/// line over `MAX_MESSAGE` is refused unparsed and skipped without being held.
pub fn serve() -> i32 {
    let mut stdin = std::io::stdin().lock();
    loop {
        let mut line = Vec::new();
        match (&mut stdin).take(MAX_MESSAGE as u64 + 1).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let reply = if line.len() > MAX_MESSAGE && line.last() != Some(&b'\n') {
            if skip_line(&mut stdin).is_err() {
                break;
            }
            Some(error(Value::Null, -32600, "a message takes at most 2 MB"))
        } else if line.iter().all(u8::is_ascii_whitespace) {
            None
        } else {
            handle(&line)
        };
        if let Some(reply) = reply {
            println!("{reply}");
        }
    }
    0
}

/// Drop input through the next newline, a buffer at a time.
fn skip_line(r: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let buf = r.fill_buf()?;
        let (n, end) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), buf.is_empty()),
        };
        r.consume(n);
        if end {
            return Ok(());
        }
    }
}

/// The reply to one line, or `None` for a notification.
fn handle(line: &[u8]) -> Option<Value> {
    let Ok(msg) = serde_json::from_slice::<Value>(line) else {
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
            "description": "Check kio source in a sandbox. Returns the authority report as text: `main`'s line and one `name : param methods` line per function main reaches that uses a capability; then, if any, a blank line, `reached only from tests:` and those lines; then, if any, a blank line and a count of the functions not listed (`N more use no capability, M are unreached (K use a capability); --all lists every function`). On failure, errors as a JSON array of {file, line, message, hint}.",
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
        .map(|f| Some((f.get("name")?.as_str()?, f.get("source")?.as_str()?)))
        .collect::<Option<Vec<_>>>()
        .filter(|f| !f.is_empty())?;
    if files.iter().map(|(n, s)| n.len() + s.len()).sum::<usize>() > MAX_SOURCE {
        return Some(Err("kio_check takes at most 1 MB of source per call"));
    }
    let files: Vec<(String, String)> = (files.into_iter())
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
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
