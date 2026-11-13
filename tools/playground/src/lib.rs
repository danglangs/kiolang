//! `kio check` for the browser playground (SITE B): one file in, JSON out. Nothing runs.
//!
//! The wasm build exports `alloc`, `free`, `check` and `diff`; `check` and `diff` return a
//! pointer to a 4-byte little-endian length and that many bytes of JSON, which the caller frees
//! with `free(ptr, 4 + length)`.

use lang::{CapTable, Diag, Program};

/// The largest source the playground checks.
pub const MAX_INPUT: usize = 64 * 1024;

/// `kio check` of `src`: `{"ok", "errors", "report", "text", "grants"}`. `report` is `main`'s
/// section as `kio check --json` gives it, `text` what `kio check` prints, `grants` each `main`
/// parameter with the flags that grant it (`--http-methods` for each verb but GET it sends).
pub fn check_json(src: &str) -> String {
    let p = match checked(src) {
        Ok(p) => p,
        Err(errors) => return failed(&errors),
    };
    let shown = lang::shown(&p);
    let verbs = lang::main_http_verbs(&lang::authority(&p));
    let grants: Vec<String> = (p.main_params().iter())
        .map(|(name, ty)| {
            let mut methods: Vec<&str> = (verbs.iter())
                .filter(|(n, v)| n == name && v != "GET")
                .map(|(_, v)| v.as_str())
                .collect();
            methods.sort();
            methods.dedup();
            let more = match methods.is_empty() {
                true => String::new(),
                false => format!(" --http-methods {name}={}", methods.join(",")),
            };
            let flag = flag(name, ty).map_or("null".into(), |f| string(&(f + &more)));
            format!("{{\"param\":{},\"flag\":{flag}}}", string(&format!("{name}: {ty}")))
        })
        .collect();
    format!(
        "{{\"ok\":true,\"errors\":[],\"report\":{},\"text\":{},\"grants\":[{}]}}",
        strings(&shown.main),
        string(&shown.text()),
        grants.join(",")
    )
}

/// `kio check new --base old`: `{"ok", "errors", "grants", "funcs", "added", "text"}`, the middle
/// three as `--json`'s `diff`, `text` what the command prints. Errors are those of the first side
/// that fails, base first, each with its `side`.
pub fn diff_json(old: &str, new: &str) -> String {
    let (old, new) = match (checked(old), checked(new)) {
        (Err(e), _) => return failed(&side(e, "base")),
        (_, Err(e)) => return failed(&side(e, "new")),
        (Ok(o), Ok(n)) => (o, n),
    };
    let d = lang::report_diff(&old, &new);
    format!(
        "{{\"ok\":true,\"errors\":[],\"grants\":{},\"funcs\":{},\"added\":{},\"text\":{}}}",
        strings(&d.grants),
        strings(&d.funcs),
        d.added,
        string(&d.text(false))
    )
}

/// The checked program, or its errors as JSON objects.
fn checked(src: &str) -> Result<Program, Vec<String>> {
    if src.len() > MAX_INPUT {
        let msg = format!("the playground checks up to {} KB", MAX_INPUT / 1024);
        return Err(vec![error(src, None, &msg, Some("use `kio check` for a larger file"))]);
    }
    let mut at = 0;
    for line in src.split_inclusive('\n') {
        let start = at + line.len() - line.trim_start().len();
        at += line.len();
        if is_mod(line) {
            let span = Some((start, start + line.trim().len()));
            let hint = Some("use `kio check` for modules");
            return Err(vec![error(src, span, "the playground checks one file", hint)]);
        }
    }
    lang::check_files(&[("playground.kio", src)], &CapTable::v0())
        .map_err(|diags| diags.iter().map(|d| diag(src, d)).collect())
}

/// Whether `line` is `mod name;`, as `kio check` reads a module's file for it.
fn is_mod(line: &str) -> bool {
    let Some(name) = line.trim().strip_prefix("mod ").and_then(|l| l.strip_suffix(';')) else {
        return false;
    };
    let mut chars = name.trim().chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn diag(src: &str, d: &Diag) -> String {
    error(src, Some((d.span.start, d.span.end)), &d.msg, d.hint.as_deref())
}

/// `{message, hint, line, col, end_line, end_col}`; the position fields are `null` without a span.
fn error(src: &str, span: Option<(usize, usize)>, msg: &str, hint: Option<&str>) -> String {
    let at = match span {
        Some((start, end)) => {
            let ((line, col), (end_line, end_col)) = (lang::line_col(src, start), lang::line_col(src, end));
            format!("\"line\":{line},\"col\":{col},\"end_line\":{end_line},\"end_col\":{end_col}")
        }
        None => "\"line\":null,\"col\":null,\"end_line\":null,\"end_col\":null".into(),
    };
    let hint = hint.map_or("null".into(), string);
    format!("{{\"message\":{},\"hint\":{hint},{at}}}", string(msg))
}

/// `errors` with `"side": side` added to each.
fn side(errors: Vec<String>, side: &str) -> Vec<String> {
    (errors.into_iter())
        .map(|e| format!("{{\"side\":\"{side}\",{}", &e[1..]))
        .collect()
}

fn failed(errors: &[String]) -> String {
    format!("{{\"ok\":false,\"errors\":[{}]}}", errors.join(","))
}

/// The flag that grants `main` parameter `name` of capability type `ty`, as guide.md writes it.
fn flag(name: &str, ty: &str) -> Option<String> {
    let rest = match ty {
        "Dir" => "=PATH",
        "Http" => "=HOST",
        "Kv" | "Db" => "=PATH",
        "Rng" | "Clock" => "",
        "Log" => "=PATH",
        "Hmac" => "=env:VAR",
        "Incoming" | "Outgoing" | "WsMessage" | "WsSend" => return Some("--listen ADDR".into()),
        _ => return None,
    };
    Some(format!("--{} {name}{rest}", ty.to_lowercase()))
}

/// `s` as a JSON string.
fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn strings(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| string(s)).collect::<Vec<_>>().join(","))
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::slice;

    /// A buffer of `len` bytes for the caller to fill; free it with `free(ptr, len)`.
    #[no_mangle]
    pub extern "C" fn alloc(len: usize) -> *mut u8 {
        Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8
    }

    /// # Safety
    /// `ptr` and `len` are a buffer from `alloc`, or a result of `check` or `diff` and 4 plus its
    /// length, not freed before.
    #[no_mangle]
    pub unsafe extern "C" fn free(ptr: *mut u8, len: usize) {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }

    /// # Safety
    /// `ptr` and `len` are a buffer from `alloc`.
    #[no_mangle]
    pub unsafe extern "C" fn check(ptr: *const u8, len: usize) -> *mut u8 {
        let json = match std::str::from_utf8(slice::from_raw_parts(ptr, len)) {
            Ok(src) => super::check_json(src),
            Err(_) => not_utf8(),
        };
        result(json)
    }

    /// # Safety
    /// Both pairs are buffers from `alloc`.
    #[no_mangle]
    pub unsafe extern "C" fn diff(old: *const u8, old_len: usize, new: *const u8, new_len: usize) -> *mut u8 {
        let text = |p, n| std::str::from_utf8(slice::from_raw_parts(p, n));
        let json = match (text(old, old_len), text(new, new_len)) {
            (Ok(o), Ok(n)) => super::diff_json(o, n),
            _ => not_utf8(),
        };
        result(json)
    }

    fn not_utf8() -> String {
        super::failed(&[super::error("", None, "the input is not UTF-8", None)])
    }

    fn result(json: String) -> *mut u8 {
        let mut out = (json.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(json.as_bytes());
        Box::into_raw(out.into_boxed_slice()) as *mut u8
    }
}
