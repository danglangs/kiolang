mod mcp;

use kio_host::{
    listen, open_dir, ClockSpec, Diag, FileId, HmacKey, Kio, Limits, LogSink, Origin, RngSpec, Server, Sources, Used,
};
use serde_json::json;
use std::cell::RefCell;

/// `kio run`'s flags.
const USAGE: &str = "[--grants file] [--dir name=path[:ro]]... [--http name=host]... [--http-methods name=POST,PUT,..]... [--http-header name=Header:env:VAR|file:path]... [--kv name=path[:ro]|:memory:]... [--kv-mb mb] [--db name=path[:ro]|:memory:]... [--db-mb mb] [--rng name[=seed:N]]... [--clock name[=fixed:T]]... [--log name=path|-]... [--hmac name=env:VAR|file:path[:verify]]... [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb] [--audit log] [--listen addr] [--ws-origin origin]... [--host name]...";

fn main() {
    // The host checks and lowers programs natively, and compiled code runs on this thread's stack
    // up to `--stack`: run on a stack well above the largest `--stack`, not the main thread's.
    let t = std::thread::Builder::new().stack_size(kio_host::RUN_STACK);
    std::process::exit(t.spawn(cli).unwrap().join().unwrap());
}

thread_local! {
    /// `--json`'s object while a command builds it; `None` for plain output.
    static JSON: RefCell<Option<JsonOut>> = const { RefCell::new(None) };
}

/// The object `--json` prints, one line on stdout (product-todo D2).
#[derive(Default)]
struct JsonOut {
    value: Option<String>,
    errors: Vec<serde_json::Value>,
    report: Vec<String>,
    diff: Option<serde_json::Value>,
    audit: Option<String>,
    used: Option<Used>,
}

/// Whether output is `--json`'s object.
fn json_out() -> bool {
    JSON.with(|j| j.borrow().is_some())
}

/// Change `--json`'s object; nothing for plain output.
fn set_json(f: impl FnOnce(&mut JsonOut)) {
    JSON.with(|j| j.borrow_mut().as_mut().map(f));
}

/// With `--json` anywhere, the command's output is one JSON object on stdout and nothing else.
fn cli() -> i32 {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `kio test --json` prints its own object (T1b).
    if let [cmd, path, flags @ ..] = &args[..] {
        if cmd == "test" {
            let json = flags.iter().any(|a| a == "--json");
            let flags: Vec<&str> = flags.iter().map(String::as_str).filter(|a| *a != "--json").collect();
            return test(path, &flags, json);
        }
    }
    if !args.iter().any(|a| a == "--json") {
        return command(&args);
    }
    args.retain(|a| a != "--json");
    JSON.with(|j| *j.borrow_mut() = Some(JsonOut::default()));
    let code = command(&args);
    let o = JSON.with(|j| j.borrow_mut().take()).unwrap_or_default();
    let used = o.used.map(|u| {
        let ms = (u.ms * 10.0).round() / 10.0;
        json!({"fuel": u.fuel, "calls": u.calls, "io": u.io, "peak_memory": u.peak_memory, "ms": ms})
    });
    let line = json!({
        "ok": o.errors.is_empty(),
        "value": o.value,
        "errors": o.errors,
        "report": o.report,
        "diff": o.diff,
        "audit": o.audit,
        "used": used,
    });
    println!("{line}");
    code
}

fn command(args: &[String]) -> i32 {
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check", path] => check(path),
        ["check", path, "--base", base] => check_base(path, base, false),
        ["check", path, "--base", base, "--detail"] => check_base(path, base, true),
        ["check", path, ref flags @ ..] if flags.contains(&"--grants") => run(path, flags, true),
        ["mcp"] if json_out() => {
            fail("`--json` reports one run or check; `kio mcp` serves many\n  hint: drop --json".into())
        }
        ["mcp"] => mcp::serve(),
        ["--version"] => {
            println!("kio {} ({})", env!("CARGO_PKG_VERSION"), env!("KIO_COMMIT"));
            0
        }
        ["run", path, ref flags @ ..] => run(path, flags, false),
        _ if json_out() => fail(format!(
            "usage: {}",
            HELP.replace("{USAGE}", USAGE).replace("{TEST_USAGE}", TEST_USAGE)
        )),
        _ => {
            let help = HELP.replace("{USAGE}", USAGE).replace("{TEST_USAGE}", TEST_USAGE);
            eprintln!("kio: a capability-typed language; code names what it needs, the operator grants what that is.\n\nusage: {help}");
            2
        }
    }
}

/// The commands, `{USAGE}` standing for `kio run`'s flags.
const HELP: &str = "kio --version\n       kio check <prog.kio> [--base <old.kio> [--detail]]\n       kio check <prog.kio> --grants <file> [flags]\n       kio mcp\n       kio run <prog.kio> {USAGE}\n       kio test <prog.kio> {TEST_USAGE}";

/// The root file, then `NAME.kio` beside it for each `mod NAME;` line. This is operator input,
/// read by the host like the root; the program never sees the filesystem.
fn read_files(path: &str) -> Result<Vec<(String, String)>, i32> {
    read_program(path).map(|(files, _)| files)
}

/// A program's `(name, source)` files, root first, and the identity of each file read.
type Program = (Vec<(String, String)>, Vec<kio_host::FileId>);

/// `read_files`, and the identity of each file read, in the same order.
fn read_program(path: &str) -> Result<Program, i32> {
    let read = kio_host::read_source;
    let (root, root_id) = read(path.as_ref()).map_err(|e| fail(format!("{path}: cannot read: {e}")))?;
    let dir = std::path::Path::new(path).parent().unwrap_or(".".as_ref());
    let (mut files, mut ids) = (vec![(path.to_string(), String::new())], vec![root_id]);
    for name in root.lines().filter_map(mod_name) {
        let file = dir.join(format!("{name}.kio"));
        let (src, id) = read(&file).map_err(|e| fail(format!("mod {name}: cannot read {}: {e}", file.display())))?;
        files.push((format!("{name}.kio"), src));
        ids.push(id);
    }
    files[0].1 = root;
    Ok((files, ids))
}

/// `web` for a line that is `mod web;`.
fn mod_name(line: &str) -> Option<&str> {
    let name = line.trim().strip_prefix("mod ")?.strip_suffix(';')?.trim();
    let mut chars = name.chars();
    let ident = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    ident.then_some(name)
}

fn check(path: &str) -> i32 {
    let files = match read_files(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    match kio_host::check_files(&files) {
        Ok(report) if json_out() => {
            set_json(|o| o.report = report.lines);
            0
        }
        Ok(report) => {
            println!("{}", report.lines.join("\n"));
            0
        }
        Err(diags) => print_diags(&files, diags, "check"),
    }
}

/// The authority `path` adds over `base` and the authority it drops: exit 3 when any was added.
/// One line per changed grant; then, with `detail` or when those lines do not show every
/// addition, a blank line and one per changed function.
fn check_base(path: &str, base: &str, detail: bool) -> i32 {
    let (old, new) = match (read_files(base), read_files(path)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    let diff = match kio_host::diff_files(&old, &new) {
        Ok(diff) => diff,
        Err((is_base, diags)) => {
            let (side, files) = if is_base { ("base", &old) } else { ("new", &new) };
            if !json_out() {
                eprintln!("kio: the {side} program {} does not check", files[0].0);
            }
            return print_diags(files, diags, "check");
        }
    };
    if json_out() {
        let report = kio_host::check_files(&new).map_or(vec![], |r| r.lines);
        let d = json!({"grants": diff.grants, "funcs": diff.funcs, "added": diff.added});
        set_json(|o| (o.report, o.diff) = (report, Some(d)));
        return if diff.added { 3 } else { 0 };
    }
    if diff.grants.is_empty() && diff.funcs.is_empty() {
        println!("no change in authority");
        return 0;
    }
    let funcs = detail || (diff.added && !diff.grant_added) || diff.grants.is_empty();
    let mut out = diff.grants.clone();
    if funcs {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.extend(diff.funcs.iter().cloned());
    }
    println!("{}", out.join("\n"));
    if !diff.added {
        // A gain a base method covers (P6) is shown but adds nothing; judged by the lines printed.
        match out.iter().any(|l| l.contains(" is covered by ")) {
            true => println!("no authority added"),
            false => println!("authority removed only"),
        }
        return 0;
    }
    let hint = if funcs || diff.funcs.is_empty() {
        ""
    } else {
        "; --detail lists each function"
    };
    println!("authority added (exit 3){hint}");
    3
}

/// Every flag of `kio run`, each with one value: what a grants file may hold.
const RUN_FLAGS: [&str; 22] = [
    "--dir",
    "--http",
    "--http-methods",
    "--http-header",
    "--kv",
    "--kv-mb",
    "--db",
    "--db-mb",
    "--rng",
    "--clock",
    "--log",
    "--hmac",
    "--fuel",
    "--timeout",
    "--memory",
    "--stack",
    "--calls",
    "--io",
    "--audit",
    "--listen",
    "--ws-origin",
    "--host",
];

/// The flags that take one value for the run: in a grants file at most once, and not also on
/// the command line.
const ONCE: [&str; 10] = [
    "--kv-mb",
    "--db-mb",
    "--fuel",
    "--timeout",
    "--memory",
    "--stack",
    "--calls",
    "--io",
    "--audit",
    "--listen",
];

/// `flags` with `--grants FILE` replaced by the file's flags, ahead of the others, and the file's
/// path. A line is one flag, split at the first space (no quoting: the value is the rest of the
/// line); blank lines and lines starting with `#` are skipped.
fn with_grants(flags: &[&str]) -> Result<(Vec<String>, Option<String>), i32> {
    let mut path = None;
    let mut rest: Vec<String> = Vec::new();
    for pair in flags.chunks(2) {
        match pair {
            ["--grants", _] if path.is_some() => {
                return Err(fail(
                    "--grants is given twice\n  hint: keep every grant in one file".into(),
                ))
            }
            ["--grants", p] => path = Some(p.to_string()),
            _ => rest.extend(pair.iter().map(|f| f.to_string())),
        }
    }
    let Some(path) = path else {
        return Ok((rest, None));
    };
    let text = std::fs::read_to_string(&path).map_err(|e| fail(format!("--grants {path}: cannot read: {e}")))?;
    let mut out: Vec<String> = Vec::new();
    // Each `ONCE` flag in the file and its line.
    let mut once: Vec<(&str, usize)> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = format!("{path}:{}", i + 1);
        let (flag, value) = line.split_once(' ').unwrap_or((line, ""));
        // Not shown: a line that is not a flag could be a key pasted by mistake.
        if !flag.starts_with("--") {
            return Err(fail(format!(
                "{at}: not a flag\n  hint: each line is one flag and its value, as on the command line: `--dir data=./data`"
            )));
        }
        if flag == "--grants" {
            return Err(fail(format!(
                "{at}: a grants file cannot name another\n  hint: put those grants in this file"
            )));
        }
        if !RUN_FLAGS.contains(&flag) {
            return Err(fail(format!(
                "{at}: unknown flag `{flag}`\n  hint: a line is one of `kio run`'s flags: {}",
                RUN_FLAGS.join(" ")
            )));
        }
        let value = value.trim_start();
        if value.is_empty() {
            return Err(fail(format!(
                "{at}: `{flag}` has no value\n  hint: write it after a space: `{flag} value`"
            )));
        }
        if ONCE.contains(&flag) {
            if let Some((_, n)) = once.iter().find(|(f, _)| *f == flag) {
                return Err(fail(format!(
                    "`{flag}` is given twice in {path}, lines {n} and {}\n  hint: give it once",
                    i + 1
                )));
            }
            if rest.iter().step_by(2).any(|f| f == flag) {
                return Err(fail(format!(
                    "`{flag}` is given in {at} and on the command line\n  hint: give it in one place"
                )));
            }
            once.push((flag, i + 1));
        }
        out.push(flag.into());
        out.push(value.into());
    }
    out.extend(rest);
    Ok((out, Some(path)))
}

/// Why the grants file `path` is refused, if it is: the program could change it, and so its own
/// grants for the next run, were it reached through a granted `--dir` (`kio_host::within`;
/// another hard link could be) or the file of a flag in `written` (`--audit`, `--log`, `--kv`,
/// `--db`).
fn grants_refusal(path: &str, dirs: &[Granted], written: &[(String, &str)]) -> Option<String> {
    let changes = "the program could change its own grants for the next run";
    if std::fs::metadata(path).is_ok_and(|m| std::os::unix::fs::MetadataExt::nlink(&m) > 1) {
        return Some(format!(
            "`--grants {path}` has other hard links; {changes} through one\n  hint: give the grants file no other links"
        ));
    }
    for (d, dir, id) in dirs {
        if kio_host::within(path.as_ref(), *id) {
            return Some(format!(
                "`--grants {path}` is inside `--dir {d}={dir}`; {changes}\n  hint: keep the grants file outside every granted directory"
            ));
        }
    }
    if let Some((flag, _)) = written.iter().find(|(_, f)| same_file(path, f)) {
        return Some(format!(
            "`--grants {path}` is the `{flag}` file; {changes}\n  hint: give the grants file a file of its own"
        ));
    }
    None
}

/// A granted `--dir`: its name, its path as given and the identity of the directory opened for
/// it (or, for `kio check --grants`, which opens nothing, of what the path names now).
type Granted = (String, String, FileId);

/// Whether the granted directory `id` holds `path`, which may not exist yet: compared by
/// identity along `path` resolved (`kio_host::resolve`).
fn holds(id: FileId, path: &str) -> bool {
    kio_host::resolve(path.as_ref()).is_some_and(|r| r.ancestors().any(|a| kio_host::path_id(a).is_ok_and(|i| i == id)))
}

/// Apply one `--http-header name=Header:env:VAR` or `name=Header:file:PATH` to the grant `name`:
/// the value is read now, once. A file's one trailing newline is dropped. The file must be a
/// regular file outside every granted `dirs` (the program could read it) and not the file of a
/// flag in `files` (`--audit`, `--log`). Errors never hold the value.
fn http_header(
    spec: &str,
    https: &mut [(String, Origin)],
    dirs: &[Granted],
    files: &[(String, &str)],
    dry: bool,
) -> Result<(), String> {
    let usage = "expected name=Header:env:VAR or name=Header:file:PATH";
    let Some((name, rest)) = spec.split_once('=') else {
        return Err(usage.into());
    };
    let Some((header, source)) = rest.split_once(':') else {
        return Err(usage.into());
    };
    let Some((_, origin)) = https.iter_mut().find(|(n, _)| n == name) else {
        return Err(format!(
            "no `--http {name}=host` grant\n  hint: --http-header adds a header to an Http grant of that name"
        ));
    };
    let value = secret(source, usage, "the header's value", dirs, files, dry)?;
    origin.inject(header, value)
}

/// Grant one `--hmac name=env:VAR` or `name=file:PATH`, either ending in `:verify` to refuse
/// `sign`: the key is read now, once, with `--http-header`'s rules (`secret`). Errors never hold
/// the key.
fn hmac(spec: &str, dirs: &[Granted], files: &[(String, &str)], dry: bool) -> Result<(String, HmacKey), String> {
    let usage = "expected name=env:VAR or name=file:PATH, with `:verify` at the end to refuse sign";
    let Some((name, source)) = spec.split_once('=').filter(|(n, _)| !n.is_empty()) else {
        return Err(usage.into());
    };
    let (source, verify_only) = match source.strip_suffix(":verify") {
        Some(s) => (s, true),
        None => (source, false),
    };
    let value = secret(source, usage, "the key", dirs, files, dry)?;
    Ok((name.into(), HmacKey::new(value, verify_only)?))
}

/// The secret `source` names, `env:VAR` or `file:PATH` (`read_secret`), read once; `usage` for
/// any other form; `what` names it in the hint for an empty one. Errors never hold the value.
/// With `dry` (`kio check --grants`) only the form is checked: nothing is read, and `UNREAD`
/// stands in for the value.
fn secret(
    source: &str,
    usage: &str,
    what: &str,
    dirs: &[Granted],
    files: &[(String, &str)],
    dry: bool,
) -> Result<String, String> {
    let value = match source.split_once(':') {
        Some(("env", var)) if dry && !var.is_empty() && !var.contains(['=', '\0']) => return Ok(UNREAD.into()),
        Some(("file", path)) if dry && !path.is_empty() => return Ok(UNREAD.into()),
        Some(("env", var)) if !var.is_empty() && !var.contains(['=', '\0']) => match std::env::var(var) {
            Ok(v) => v,
            Err(std::env::VarError::NotPresent) => {
                return Err(format!(
                    "`{var}` is not set\n  hint: export {var} in the environment that runs kio, or use file:PATH"
                ))
            }
            Err(std::env::VarError::NotUnicode(_)) => return Err(format!("`{var}` is not UTF-8")),
        },
        Some(("file", path)) if !path.is_empty() => read_secret(path, dirs, files)?,
        _ => return Err(usage.into()),
    };
    if value.trim().is_empty() {
        return Err(format!(
            "{source} is empty or only whitespace\n  hint: set it to {what}"
        ));
    }
    Ok(value)
}

/// The value in the file `path` for `--http-header` or `--hmac`, its one trailing newline
/// dropped. The file must be a regular file outside every granted `dirs` (the program could
/// read it) and not the file of a flag in `files` (`--audit`, `--log`). At most `SECRET_MAX` + 2
/// bytes are read: a longer value is refused by `Origin::inject` and `HmacKey::new`.
fn read_secret(path: &str, dirs: &[Granted], files: &[(String, &str)]) -> Result<String, String> {
    let cannot =
        |e: &dyn std::fmt::Display| format!("cannot read {path}: {e}\n  hint: the file holds the secret alone");
    // A FIFO or device would block or never end: check before opening.
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(cannot(&"not a regular file")),
        Err(e) => return Err(cannot(&e)),
    }
    // Another name for the file could be inside a granted `--dir`, where the check below misses it.
    if std::fs::metadata(path).is_ok_and(|m| std::os::unix::fs::MetadataExt::nlink(&m) > 1) {
        return Err(format!(
            "`{path}` has other hard links; the program could read one\n  hint: give kio a key file of its own"
        ));
    }
    for (d, dir, id) in dirs {
        if kio_host::within(path.as_ref(), *id) {
            return Err(format!(
                "`{path}` is inside `--dir {d}={dir}`; the program could read it\n  hint: keep the key outside every granted directory"
            ));
        }
    }
    if let Some((flag, _)) = files.iter().find(|(_, f)| same_file(path, f)) {
        return Err(format!(
            "`{path}` is the `{flag}` file\n  hint: keep the key in a file of its own"
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| cannot(&e))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, SECRET_MAX + 2), &mut bytes).map_err(|e| cannot(&e))?;
    let v = String::from_utf8(bytes).map_err(|_| cannot(&"not UTF-8"))?;
    let v = v.strip_suffix('\n').unwrap_or(&v);
    Ok(v.strip_suffix('\r').unwrap_or(v).to_string())
}

/// What `kio check --grants` passes on for a secret it does not read: a valid header value and key.
const UNREAD: &str = "unread";

/// The most of a secret read from a file (as `Origin::inject` and `HmacKey::new` allow).
const SECRET_MAX: u64 = 8 << 10;

/// `spec` of a `--http-header` as an error may show it: `name=Header`, never what follows the
/// first `:` (or, without one, the `=`), where a value given inline by mistake would be.
fn header_label(spec: &str) -> &str {
    if !spec.contains('=') {
        return "";
    }
    match spec.split_once(':') {
        Some((label, _)) => label,
        None => spec.split('=').next().unwrap_or(""),
    }
}

/// `--flag label: e`, or `--flag: e` for an empty label.
fn flag_error(flag: &str, label: &str, e: &str) -> String {
    if label.is_empty() {
        format!("{flag}: {e}")
    } else {
        format!("{flag} {label}: {e}")
    }
}

/// `kio run`, or with `dry` `kio check --grants`: check the flags and bind them to `main`'s
/// parameters, print the report and stop, reading no secret and opening no granted path.
fn run(path: &str, flags: &[&str], dry: bool) -> i32 {
    let (flags, grants_file) = match with_grants(flags) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    if json_out() && flags.iter().step_by(2).any(|f| *f == "--listen") {
        return fail("`--json` reports one run; `--listen` serves many\n  hint: drop --json, or drop --listen".into());
    }
    let (files, read_ids) = match read_program(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    run_program(&files, &read_ids, &flags, grants_file.as_deref(), dry, None)
}

/// One test `kio test` runs through `run_program`: the checked program, the test, and the
/// `Log` grants the flags cannot give (a buffer each).
struct TestRun<'a> {
    program: &'a lang::Program,
    test: &'a lang::TestFn,
    logs: Vec<(String, LogSink)>,
    /// A property's case: its values, and whether it runs in the interpreter (test builds only).
    case: Option<(lang::prop::Case, bool)>,
}

/// `run`'s work once the flags (`grants_file`'s first) and the program's `files`, with the
/// identity of each file read, are known. With `test`, the test is the program's `main`: its
/// parameters are bound, and its report line checked, as `main`'s are; nothing is printed but
/// through `--json`'s object, which the caller captures.
fn run_program(
    files: &[(String, String)],
    read_ids: &[FileId],
    flags: &[&str],
    grants_file: Option<&str>,
    dry: bool,
    mut test: Option<TestRun>,
) -> i32 {
    let mut dirs = Vec::new();
    let mut https: Vec<(String, Origin)> = Vec::new();
    // `(name, methods)` of each `--http-methods`, applied once every `--http` is known.
    let mut http_methods: Vec<(String, String)> = Vec::new();
    // Each `--http-header` as given, read once every `--http` is known.
    let mut http_headers: Vec<&str> = Vec::new();
    // Each `--hmac` as given, read once the `--dir`, `--audit` and `--log` paths are known.
    let mut hmac_specs: Vec<&str> = Vec::new();
    let mut limits = if flags.contains(&"--listen") {
        Limits::web()
    } else {
        Limits::default()
    };
    let mut audit = None;
    let mut listen = None;
    let mut ws_origins = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut dir_paths = Vec::new();
    let mut kv_specs: Vec<(String, String)> = Vec::new();
    let mut kv_mb = 64;
    let mut db_specs: Vec<(String, String)> = Vec::new();
    let mut db_mb = kio_host::db::DB_MB;
    let mut sources = Sources::default();
    // `(name, path)` of each `--log` to a file, opened once the paths are checked.
    let mut log_paths: Vec<(String, String)> = Vec::new();
    for pair in flags.chunks(2) {
        let (name, dir) = match pair {
            ["--http", spec] => {
                let Some((name, origin)) = spec.split_once('=') else {
                    return fail(format!("--http {spec}: expected name=host"));
                };
                match Origin::parse(origin) {
                    Ok(o) => https.push((name.to_string(), o)),
                    Err(e) => return fail(format!("--http {name}={origin}: {e}")),
                }
                continue;
            }
            ["--http-methods", spec] => {
                match spec.split_once('=') {
                    Some((name, list)) if !name.is_empty() && !list.is_empty() => {
                        http_methods.push((name.into(), list.into()))
                    }
                    _ => return fail(format!("--http-methods {spec}: expected name=GET,POST,..")),
                }
                continue;
            }
            ["--http-header", spec] => {
                http_headers.push(spec);
                continue;
            }
            ["--hmac", spec] => {
                hmac_specs.push(spec);
                continue;
            }
            [flag @ ("--fuel" | "--timeout" | "--memory" | "--stack" | "--calls" | "--io"), n] => {
                let Some(v) = n.parse::<u64>().ok().filter(|&v| v > 0) else {
                    return fail(format!("{flag} {n}: expected a positive integer"));
                };
                match *flag {
                    "--fuel" => limits.fuel = v,
                    "--timeout" => limits.timeout_secs = v,
                    "--memory" => limits.memory = (v as usize).saturating_mul(1 << 20),
                    "--calls" => limits.calls = u32::try_from(v).unwrap_or(u32::MAX),
                    "--io" => limits.io = v.saturating_mul(1 << 20),
                    _ if v > (kio_host::MAX_STACK >> 10) as u64 => {
                        return fail(format!("--stack {n}: at most {} KB", kio_host::MAX_STACK >> 10));
                    }
                    _ => limits.stack = (v as usize).saturating_mul(1 << 10),
                }
                continue;
            }
            ["--kv", spec] => {
                match spec.split_once('=') {
                    Some((_, ":memory::ro")) => {
                        return fail(format!(
                            "--kv {spec}: an in-memory store is never read-only\n  hint: drop `:ro`; only this process can write it"
                        ))
                    }
                    Some((name, s)) if !name.is_empty() && !s.is_empty() => kv_specs.push((name.into(), s.into())),
                    _ => return fail(format!("--kv {spec}: expected name=path, name=path:ro or name=:memory:")),
                }
                continue;
            }
            ["--kv-mb", n] => {
                match n.parse::<u64>().ok().filter(|&v| v > 0) {
                    Some(v) => kv_mb = v,
                    None => return fail(format!("--kv-mb {n}: expected a positive integer")),
                }
                continue;
            }
            ["--db-mb", n] => {
                match n.parse::<u64>().ok().filter(|&v| v > 0) {
                    Some(v) => db_mb = v,
                    None => return fail(format!("--db-mb {n}: expected a positive integer")),
                }
                continue;
            }
            ["--db", spec] => {
                match spec.split_once('=') {
                    Some((name, s)) if !name.is_empty() && !s.is_empty() => db_specs.push((name.into(), s.into())),
                    _ => {
                        return fail(format!(
                            "--db {spec}: expected name=path, name=path:ro or name=:memory:"
                        ))
                    }
                }
                continue;
            }
            ["--audit", path] => {
                // Created below, once the flags that conflict with it are known.
                audit = Some(path.to_string());
                continue;
            }
            ["--listen", spec] => {
                match spec.parse::<std::net::SocketAddr>() {
                    Ok(a) => listen = Some(a),
                    Err(e) => return fail(format!("--listen {spec}: {e}; expected ip:port")),
                }
                continue;
            }
            ["--ws-origin", origin] => {
                ws_origins.push(origin.to_string());
                continue;
            }
            ["--host", name] => {
                hosts.push(name.to_ascii_lowercase());
                continue;
            }
            [flag @ ("--rng" | "--clock"), spec] => {
                let (name, rest) = match spec.split_once('=') {
                    Some((n, r)) => (n, Some(r)),
                    None => (*spec, None),
                };
                if name.is_empty() {
                    return fail(format!("{flag} {spec}: expected a name"));
                }
                let parsed = match *flag {
                    "--rng" => RngSpec::parse(rest).map(|r| sources.rngs.push((name.into(), r))),
                    _ => ClockSpec::parse(rest).map(|c| sources.clocks.push((name.into(), c))),
                };
                if let Err(e) = parsed {
                    return fail(format!("{flag} {spec}: {e}"));
                }
                continue;
            }
            ["--log", spec] => {
                match spec.split_once('=') {
                    Some((name, "-")) if !name.is_empty() => sources.logs.push((name.into(), LogSink::stderr())),
                    Some((name, path)) if !name.is_empty() && !path.is_empty() => {
                        log_paths.push((name.into(), path.into()))
                    }
                    _ => return fail(format!("--log {spec}: expected name=path or name=- (stderr)")),
                }
                continue;
            }
            ["--dir", spec] => match spec.split_once('=') {
                Some((name, dir)) => {
                    let (path, ro) = kio_host::kv::split_spec(dir);
                    if ro {
                        sources.read_only_dirs.push(name.to_string());
                    }
                    (name, path)
                }
                None => return fail(format!("--dir {spec}: expected name=path or name=path:ro")),
            },
            _ => return fail(format!("usage: kio run <prog.kio> {USAGE}")),
        };
        dir_paths.push((name.to_string(), dir.to_string()));
        if dry {
            continue;
        }
        match open_dir(dir.as_ref()) {
            Ok(d) => dirs.push((name.to_string(), d)),
            Err(e) => return refuse(format!("--dir {name}={dir}: {e}")),
        }
    }
    // Each grant compared by the directory opened for it, not by its path, which may name
    // another by now. `kio check --grants` opens nothing: what each path names now.
    let mut granted: Vec<Granted> = Vec::new();
    for (i, (name, path)) in dir_paths.iter().enumerate() {
        let id = match dirs.get(i) {
            Some((_, d)) => kio_host::dir_id(d),
            None => kio_host::path_id(path.as_ref()),
        };
        match id {
            Ok(id) => granted.push((name.clone(), path.clone(), id)),
            Err(e) if !dry => return refuse(format!("--dir {name}={path}: {e}")),
            Err(_) => {}
        }
    }
    // Checked against the files read: a path changed since names another file.
    let (places, ends) = kio_host::source_places(files);
    if let Some(why) = kio_host::changed_refusal(&ends, read_ids)
        .or_else(|| kio_host::source_refusal(&places, &granted, &sources.read_only_dirs))
    {
        return refuse(why);
    }
    for (i, (name, list)) in http_methods.iter().enumerate() {
        if http_methods[..i].iter().any(|(n, _)| n == name) {
            return fail(format!(
                "--http-methods {name}={list}: `{name}` is given more than once\n  hint: list every method in one flag: `--http-methods {name}=GET,POST,PUT`"
            ));
        }
        let Some((_, origin)) = https.iter_mut().find(|(n, _)| n == name) else {
            return fail(format!(
                "--http-methods {name}={list}: no `--http {name}=host` grant\n  hint: --http-methods adds methods to an Http grant of that name; GET needs none"
            ));
        };
        if let Err(e) = origin.allow_methods(list) {
            return fail(format!("--http-methods {name}={list}: {e}"));
        }
    }
    let mut written: Vec<(String, &str)> = Vec::new();
    written.extend(audit.as_deref().map(|p| ("--audit".to_string(), p)));
    written.extend(log_paths.iter().map(|(n, p)| (format!("--log {n}"), p.as_str())));
    if let Some(g) = &grants_file {
        let stores = (kv_specs.iter().map(|(n, s)| (format!("--kv {n}"), s)))
            .chain(db_specs.iter().map(|(n, s)| (format!("--db {n}"), s)))
            .map(|(f, s)| (f, kio_host::kv::split_spec(s).0));
        let written: Vec<(String, &str)> = written.iter().cloned().chain(stores).collect();
        if let Some(why) = grants_refusal(g, &granted, &written) {
            return refuse(why);
        }
    }
    for spec in http_headers {
        if let Err(e) = http_header(spec, &mut https, &granted, &written, dry) {
            return refuse(flag_error("--http-header", header_label(spec), &e));
        }
    }
    // An error names the grant only: a key given inline by mistake would follow the `=`, or be
    // the whole spec when there is none.
    for spec in hmac_specs {
        match hmac(spec, &granted, &written, dry) {
            Ok(grant) => sources.hmacs.push(grant),
            Err(e) => {
                let name = spec.split_once('=').map_or("", |(n, _)| n);
                return refuse(flag_error("--hmac", name, &e));
            }
        }
    }
    // A program that fails to check says so before any check of `main`'s shape (H20).
    let owned;
    let checked = match &test {
        Some(t) => t.program,
        None => {
            owned = match lang::check_files(
                &files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect::<Vec<_>>(),
                &lang::CapTable::v0(),
            ) {
                Ok(p) => p,
                Err(diags) => return print_diags(files, diags.into_iter().map(Diag::from).collect(), "check"),
            };
            &owned
        }
    };
    // A test is held to its own authority, as `main` is to its (`lang::entry_authority`).
    let (params, report, authority) = match &test {
        Some(t) => (
            t.test.params.clone(),
            lang::entry_report(checked, &t.test.name),
            lang::entry_authority(checked, &t.test.name),
        ),
        None => (checked.main_params(), lang::report(checked), lang::authority(checked)),
    };
    // A read-only store or database is refused to a program that can write it (H12, H17); the
    // host also stops any write that reaches it.
    set_json(|o| o.report = report.lines().map(String::from).collect());
    type WriteIn = fn(&str, &str) -> Option<&'static str>;
    let readers = (kv_specs
        .iter()
        .map(|(n, s)| (n, s, kio_host::kv::write_in_report as WriteIn)))
    .chain(
        db_specs
            .iter()
            .map(|(n, s)| (n, s, kio_host::db::write_in_report as WriteIn)),
    );
    for (name, spec, write_in_report) in readers {
        if !kio_host::kv::split_spec(spec).1 {
            continue;
        }
        if let Some(m) = write_in_report(&report, name) {
            return refuse(format!(
                "`{name}` is granted read-only but `main` can call `{name} {m}`\n  hint: drop `:ro` or remove the writes"
            ));
        }
    }
    // A `:verify` key is refused to a program that can sign with it; the host also refuses a
    // `sign` that reaches it.
    for (name, key) in &sources.hmacs {
        if key.verify_only() && kio_host::signs_in_report(&report, name) {
            return refuse(format!(
                "`{name}` is granted `:verify` but the program can sign with it\n  hint: drop `:verify`, or remove the `sign` call"
            ));
        }
    }
    // A `:ro` dir is refused to a program that can write it; the host also refuses any write that
    // reaches it (H27).
    if let Some(why) = kio_host::read_only_refusal(&report, &sources.read_only_dirs) {
        return refuse(why);
    }
    // A literal verb the grant does not allow is refused (P7); one the report shows as `*` is
    // checked per request, as every request is.
    for (name, v) in lang::main_http_verbs(&authority) {
        if !lang::HTTP_METHODS.contains(&v.as_str()) {
            return refuse(format!(
                "`{name}` sends {v}, not an HTTP method\n  hint: a method is one of {}",
                lang::HTTP_METHODS.join(", ")
            ));
        }
        let Some((_, origin)) = https.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        if origin.allows(&v) {
            continue;
        }
        return refuse(match http_methods.iter().find(|(n, _)| *n == name) {
            Some((_, list)) => format!(
                "`{name}` sends {v} but `--http-methods {name}={list}` grants only {}\n  hint: add {v} to --http-methods, or remove the call",
                list.to_ascii_uppercase()
            ),
            None => format!(
                "`{name}` sends {v} but `--http {name}` grants only GET\n  hint: add `--http-methods {name}={v}`, or remove the call"
            ),
        });
    }
    let takes = |ty: &str| params.iter().filter(|(_, t)| t == ty).count();
    let http = takes("Incoming") + takes("Outgoing") > 0;
    let ws = takes("WsMessage") + takes("WsSend") > 0;
    if ws && http {
        return refuse("main takes both WebSocket and HTTP grants\n  hint: a WebSocket server answers through WsSend; drop the Incoming and Outgoing parameters".into());
    }
    if ws && (takes("WsMessage"), takes("WsSend")) != (1, 1) {
        return refuse("a WebSocket server's main takes one WsMessage and one WsSend\n  hint: write `fn main(msg: WsMessage, conn: WsSend, ..)`".into());
    }
    if http && (takes("Incoming"), takes("Outgoing")) != (1, 1) {
        return refuse("an HTTP server's main takes one Incoming and one Outgoing\n  hint: write `fn main(request: Incoming, response: Outgoing, ..)`".into());
    }
    if listen.is_none() && !hosts.is_empty() {
        return fail("--host without --listen\n  hint: --host names a Host the server answers to; add `--listen 127.0.0.1:8080` or drop it".into());
    }
    match (listen, http, ws) {
        (None, true, _) => return refuse("main takes Incoming/Outgoing but there is no address\n  hint: add `--listen 127.0.0.1:8080`".into()),
        (None, _, true) => return refuse("main takes WsMessage/WsSend but there is no address\n  hint: add `--listen 127.0.0.1:8080`".into()),
        (Some(_), false, false) => return fail("--listen needs a `main` that takes Incoming and Outgoing, or WsMessage and WsSend\n  hint: add `req: Incoming, res: Outgoing` (HTTP) or `msg: WsMessage, conn: WsSend` (WebSocket) to main, or drop --listen".into()),
        _ => {}
    }
    if !ws && !ws_origins.is_empty() {
        return fail("--ws-origin without a WebSocket server\n  hint: --ws-origin lets browser pages from that origin connect to a `main` that takes WsMessage and WsSend; drop it".into());
    }
    if dry {
        let mut named: Vec<(String, &str)> = Vec::new();
        named.extend(dir_paths.iter().map(|(n, _)| (n.clone(), "Dir")));
        named.extend(https.iter().map(|(n, _)| (n.clone(), "Http")));
        named.extend(kv_specs.iter().map(|(n, _)| (n.clone(), "Kv")));
        named.extend(db_specs.iter().map(|(n, _)| (n.clone(), "Db")));
        named.extend(sources.rngs.iter().map(|(n, _)| (n.clone(), "Rng")));
        named.extend(sources.clocks.iter().map(|(n, _)| (n.clone(), "Clock")));
        named.extend(sources.logs.iter().map(|(n, _)| (n.clone(), "Log")));
        named.extend(log_paths.iter().map(|(n, _)| (n.clone(), "Log")));
        named.extend(sources.hmacs.iter().map(|(n, _)| (n.clone(), "Hmac")));
        // `--listen` grants these by type, to the parameters of that type.
        let socket = ["Incoming", "Outgoing", "WsMessage", "WsSend"];
        for (n, ty) in params.iter().filter(|_| listen.is_some()) {
            if let Some(kind) = socket.iter().find(|k| *k == ty) {
                named.push((n.clone(), kind));
            }
        }
        return match kio_host::bind_grants(checked, &named) {
            Ok(()) if json_out() => 0,
            Ok(()) => {
                println!("{}", report.lines().collect::<Vec<_>>().join("\n"));
                0
            }
            Err(diags) => print_diags(files, diags, "grant"),
        };
    }
    // A program that can write a store's log or a database through a `Dir` could replace or
    // craft it. `stores` is each `--kv` and `--db` file with the files beside it that are its own
    // (a compaction's `.tmp`, SQLite's `-wal`, `-shm` and `-journal`), by flag.
    let mut stores: Vec<(String, String)> = Vec::new();
    // Each grant's files, its own path first.
    let mut grants: Vec<(String, Vec<String>)> = Vec::new();
    let on_disk = |specs: &[(String, String)], flag: &str| -> Vec<(String, String)> {
        (specs.iter().filter(|(_, s)| s != ":memory:"))
            .map(|(n, s)| (format!("{flag} {n}"), kio_host::kv::split_spec(s).0.to_string()))
            .collect()
    };
    for (flag, path) in on_disk(&kv_specs, "--kv").into_iter().chain(on_disk(&db_specs, "--db")) {
        if let Some((d, _, _)) = granted.iter().find(|(_, _, id)| holds(*id, &path)) {
            return refuse(format!(
                "`{flag}` is inside `--dir {d}`\n  hint: keep stores outside granted directories"
            ));
        }
        // A database granted on a store's file (or the other way round) would corrupt it.
        if let Some((other, _)) = stores
            .iter()
            .find(|(o, p)| !o.starts_with(&flag[..4]) && same_file(p, &path))
        {
            return refuse(format!(
                "`{flag}` is the same file as `{other}`\n  hint: give each its own file"
            ));
        }
        let own: &[&str] = if flag.starts_with("--kv") {
            &["", ".tmp"]
        } else {
            &["", "-wal", "-shm", "-journal"]
        };
        let own: Vec<String> = own.iter().map(|s| format!("{path}{s}")).collect();
        // A grant naming another's `-wal` (or the other way round) shares a file with it. Two
        // grants of one file are `open_all`'s to refuse.
        for (other, theirs) in grants.iter().filter(|(o, _)| o.starts_with(&flag[..4])) {
            if same_file(&own[0], &theirs[0]) {
                continue;
            }
            let pairs = own.iter().flat_map(|m| theirs.iter().map(move |t| (m, t)));
            if let Some((m, t)) = pairs.into_iter().find(|(m, t)| same_file(m, t)) {
                return refuse(format!(
                    "`{flag}` and `{other}` share the file {}\n  hint: a database's `-wal`, `-shm` and `-journal` (a store's `.tmp`) are its own: give each grant its own file",
                    if *m == path { t } else { m }
                ));
            }
        }
        grants.push((flag.clone(), own.clone()));
        stores.extend(own.into_iter().map(|p| (flag.clone(), p)));
    }
    // The audit log and each `--log` file: not a store, not inside a granted dir, and not one
    // another's file.
    let mut taken: Vec<(String, &str)> = Vec::new();
    let mut files_to_check: Vec<(String, &str)> = Vec::new();
    if let Some(path) = &audit {
        files_to_check.push(("--audit".into(), path));
    }
    files_to_check.extend(log_paths.iter().map(|(n, p)| (format!("--log {n}"), p.as_str())));
    for (flag, path) in files_to_check {
        if let Some(why) = file_refusal(&flag, path, &stores, &granted, &taken) {
            return refuse(why);
        }
        taken.push((flag, path));
    }
    let kvs = match kio_host::kv::open_all(&kv_specs, kv_mb) {
        Ok(kvs) => kvs,
        Err(e) => return refuse(e),
    };
    match kio_host::db::open_all(&db_specs, db_mb) {
        Ok(dbs) => sources.dbs = dbs,
        Err(e) => return refuse(e),
    }
    for (name, path) in &log_paths {
        match open_append(path) {
            Ok(f) => sources.logs.push((name.clone(), LogSink::file(f, path))),
            Err(e) => return refuse(format!("--log {name}={path}: {e}")),
        }
    }
    if let Some(addr) = listen {
        let audit = audit.map(|path| (path, dir_paths));
        return serve(
            files.to_vec(),
            dirs,
            https,
            kvs,
            sources,
            addr,
            limits,
            ws_origins,
            hosts,
            audit,
        );
    }
    let mut kio = match Kio::new(limits) {
        Ok(k) => k,
        Err(e) => return internal(format!("{e:#}")),
    };
    if let Some(path) = audit {
        match open_append(&path) {
            Ok(f) => {
                kio.audit(f, dir_paths);
                set_json(|o| o.audit = Some(path.clone()));
            }
            Err(e) => return refuse(format!("--audit {path}: {e}")),
        }
    }
    kio.grant_kvs(kvs);
    if let Some(t) = &mut test {
        sources.logs.append(&mut t.logs);
    }
    kio.grant_sources(sources);
    let result = match &test {
        #[cfg(feature = "test-hooks")]
        Some(TestRun {
            test,
            case: Some((c, true)),
            ..
        }) => kio.interpret_case(files, &test.name, c, dirs, https),
        Some(TestRun {
            test, case: Some(c), ..
        }) => kio.run_case(files, &test.name, &c.0, dirs, https),
        Some(t) => kio.run_test(files, &t.test.name, dirs, https),
        None => kio.run_files(files, dirs, https),
    };
    set_json(|o| o.used = kio.used());
    match result {
        Err(e) if e.downcast_ref::<kio_host::Refused>().is_some() => refuse(format!("{e}")),
        Err(e) => {
            // A limit or trap is the program failing, not a usage error.
            match kio.explain(&e) {
                Some(msg) => stopped(&msg),
                None if json_out() => push_error("internal", &format!("{e:#}"), None, None),
                None => eprintln!("kio: {e:#}"),
            }
            1
        }
        // A test's `Err(e)` fails it, shown as `{:?}` shows it, as lang's `eval::debug` does.
        Ok(Ok(out)) if test.is_some() => {
            if out.failed {
                use lang::check::Ty;
                let str_err = matches!(test.as_ref().map(|t| &t.test.ret), Some(Ty::Result(_, e)) if **e == Ty::Str);
                let shown = if str_err { format!("{:?}", out.value) } else { out.value };
                push_error("err", &format!("Err({shown})"), None, None);
            }
            i32::from(out.failed)
        }
        Ok(Ok(out)) if json_out() => {
            match out.failed {
                true => push_error("err", &out.value, None, None),
                false => set_json(|o| o.value = json_value(&out.value, checked.main_ret())),
            }
            i32::from(out.failed)
        }
        Ok(Ok(out)) => {
            if !params.is_empty() {
                eprintln!("{}", out.report.lines.join("\n"));
            }
            // Like Rust's `main`: `()` and `Ok(())` print nothing, `Err(e)` goes to stderr and fails the run.
            if out.failed {
                eprintln!("error: {}", out.value);
            } else if out.value != "()" && out.value != "Ok(())" {
                println!("{}", out.value);
            }
            i32::from(out.failed)
        }
        // A grant that does not bind stops the run before `main` starts; any other is a
        // run-time error.
        Ok(Err(diags)) => {
            let kind = if kio.used().is_some() { "runtime" } else { "grant" };
            print_diags(files, diags, kind)
        }
    }
}

/// `kio test`'s flags.
const TEST_USAGE: &str =
    "[--grants file] [--filter substr] [--cases n] [--seed s] [--json] [--fuel n] [--timeout secs] [--memory mb] [--stack kb] [--calls n] [--io mb]";

/// The limit flags `kio test` takes on its command line; grants come from `--grants`.
const LIMIT_FLAGS: [&str; 6] = ["--fuel", "--timeout", "--memory", "--stack", "--calls", "--io"];

/// The flags of a grant to one parameter, `--flag name=..` (`--rng name` and `--clock name`
/// too), and the parameter's capability type.
const NAMED: [(&str, &str); 10] = [
    ("--dir", "Dir"),
    ("--http", "Http"),
    ("--http-methods", "Http"),
    ("--http-header", "Http"),
    ("--kv", "Kv"),
    ("--db", "Db"),
    ("--rng", "Rng"),
    ("--clock", "Clock"),
    ("--log", "Log"),
    ("--hmac", "Hmac"),
];

/// The time a test's `Clock` reads unless the grants file names it.
const TEST_CLOCK: &str = "fixed:2026-01-01T00:00:00Z";

/// A checked program and the flags each of its tests is run with.
struct Tests {
    files: Vec<(String, String)>,
    read_ids: Vec<FileId>,
    /// The grants file's flags, then the command line's limits.
    flags: Vec<String>,
    grants_file: Option<String>,
    program: lang::Program,
    filter: String,
    /// The program's path as given, for a property's rerun line.
    path: String,
    /// Cases per property (`--cases`) and the seed they are generated from (`--seed`).
    cases: usize,
    seed: u64,
}

/// One test's result: its error as `--json` shows one, the time it took and its `Log` lines;
/// for a property, how its cases ran.
struct TestResult {
    name: String,
    error: Option<serde_json::Value>,
    ms: f64,
    log: String,
    prop: Option<PropInfo>,
}

/// How a property's cases ran: generated cases (each compiled), discards, shrink runs, and the
/// input shown with a failure (`(name, literal)`, shrunk or not).
#[derive(Default)]
struct PropInfo {
    compiled: usize,
    seed: u64,
    discarded: usize,
    input: Option<(Vec<(String, String)>, bool)>,
    /// Compiled runs shrinking took; the minimal input then ran once more.
    shrinks: usize,
}

impl PropInfo {
    /// `100 cases compiled; seed 1`, and the discards if any.
    fn summary(&self) -> String {
        let cases = if self.compiled == 1 { "case" } else { "cases" };
        let mut line = format!("{} {cases} compiled; seed {}", self.compiled, self.seed);
        if self.discarded > 0 {
            line += &format!("; {} discarded", self.discarded);
        }
        line
    }
}

/// Runs `f` with `--json`'s object capturing what it reports when `capture`, and that object.
fn captured<T>(capture: bool, f: impl FnOnce() -> T) -> (T, JsonOut) {
    if !capture {
        return (f(), JsonOut::default());
    }
    let before = JSON.with(|j| j.borrow_mut().replace(JsonOut::default()));
    let r = f();
    let out = JSON.with(|j| std::mem::replace(&mut *j.borrow_mut(), before));
    (r, out.unwrap_or_default())
}

/// `kio test`: every test function (sorted by key, those containing `--filter`), each compiled
/// as the entry and run in a fresh instance with default limits. Exit 0 when all pass, 1 when
/// any fails, 2 for a check error or usage.
fn test(path: &str, flags: &[&str], json: bool) -> i32 {
    let (tests, out) = captured(json, || test_setup(path, flags));
    let tests = match tests {
        Ok(t) => t,
        Err(_) => {
            if json {
                let line = json!({"ok": false, "passed": 0, "failed": 0, "tests": [], "errors": out.errors});
                println!("{line}");
            }
            return 2;
        }
    };
    let mut results = Vec::new();
    for t in tests.program.tests().iter().filter(|t| t.name.contains(&tests.filter)) {
        results.push(run_test(&tests, t));
    }
    let failed = results.iter().filter(|r| r.error.is_some()).count();
    let passed = results.len() - failed;
    if json {
        let rows: Vec<_> = (results.iter())
            .map(|r| {
                let ms = (r.ms * 10.0).round() / 10.0;
                let mut row =
                    json!({"name": r.name, "ok": r.error.is_none(), "ms": ms, "error": r.error, "log": r.log});
                if let Some(p) = &r.prop {
                    let input = (p.input.as_ref()).map(|(vs, _)| {
                        vs.iter()
                            .map(|(k, v)| (k.clone(), json!(v)))
                            .collect::<serde_json::Map<_, _>>()
                    });
                    row["cases"] = json!(p.compiled);
                    row["compiled"] = json!(p.compiled);
                    row["seed"] = json!(p.seed);
                    row["discarded"] = json!(p.discarded);
                    row["shrinks"] = json!(p.shrinks);
                    row["input"] = json!(input);
                }
                row
            })
            .collect();
        println!(
            "{}",
            json!({"ok": failed == 0, "passed": passed, "failed": failed, "tests": rows})
        );
    } else {
        for r in &results {
            let summary = r
                .prop
                .as_ref()
                .map_or(String::new(), |p| format!("  ({})", p.summary()));
            let Some(e) = &r.error else {
                println!("ok   {}{summary}", r.name);
                continue;
            };
            println!("FAIL {}{summary}", r.name);
            if let Some((input, shrunk)) = r.prop.as_ref().and_then(|p| p.input.as_ref()) {
                let shown: Vec<String> = input.iter().map(|(n, v)| format!("{n} = {v}")).collect();
                let label = if *shrunk { "minimal input" } else { "input" };
                let line = lang::eval::clip(&shown.join(", ")).into_owned();
                println!("  {label}: {}", kio_host::escape_line(&line));
                if *shrunk {
                    let n = r.prop.as_ref().map_or(0, |p| p.shrinks);
                    let stopped = if n == MAX_SHRINK_RUNS {
                        "; shrinking stopped after 200 runs"
                    } else {
                        ""
                    };
                    println!("  shrunk in {n} compiled runs{stopped}");
                }
            }
            // The program writes the message: escaped as a log line is, it is one line and no
            // terminal escape, so it cannot print a forged `ok` or count.
            let message = kio_host::escape_line(e["message"].as_str().unwrap_or_default());
            match (e["file"].as_str(), e["line"].as_u64(), e["col"].as_u64()) {
                (Some(f), Some(l), Some(c)) => println!("  {f}:{l}:{c}: {message}"),
                _ => println!("  {message}"),
            }
            if let Some(h) = e["hint"].as_str() {
                println!("  hint: {}", kio_host::escape_line(h));
            }
            if !r.log.is_empty() {
                println!("  log:");
                r.log.lines().for_each(|l| println!("    {l}"));
            }
            if let Some(p) = &r.prop {
                let cases = if tests.cases == DEFAULT_CASES {
                    String::new()
                } else {
                    format!(" --cases {}", tests.cases)
                };
                let (file, name) = (shell_word(&tests.path), shell_word(&r.name));
                println!("  rerun: kio test {file} --filter {name} --seed {}{cases}", p.seed);
            }
        }
        println!("{passed} passed, {failed} failed");
    }
    i32::from(failed > 0)
}

/// The flags, program and filter of `kio test path flags`, or the exit code once the error is
/// reported.
fn test_setup(path: &str, flags: &[&str]) -> Result<Tests, i32> {
    let usage = || fail(format!("usage: kio test <prog.kio> {TEST_USAGE}"));
    if !flags.len().is_multiple_of(2) {
        return Err(usage());
    }
    let mut filter = String::new();
    let (mut cases, mut seed) = (DEFAULT_CASES, 1);
    let mut rest = Vec::new();
    for pair in flags.chunks(2) {
        match pair {
            ["--filter", f] => filter = f.to_string(),
            ["--cases", n] => match n.parse::<usize>() {
                Ok(n) if (1..=MAX_CASES).contains(&n) => cases = n,
                _ => return Err(fail(format!("--cases {n}: expected 1 to {MAX_CASES}"))),
            },
            ["--seed", s] => match s.parse::<u64>() {
                Ok(s) => seed = s,
                Err(_) => return Err(fail(format!("--seed {s}: expected a non-negative integer"))),
            },
            [flag, _] if *flag == "--grants" || LIMIT_FLAGS.contains(flag) => rest.extend(pair),
            [flag, _] if RUN_FLAGS.contains(flag) => {
                return Err(fail(format!(
                    "`{flag}` is not a flag of kio test\n  hint: name grants in a --grants file, one flag per line"
                )))
            }
            _ => return Err(usage()),
        }
    }
    let (flags, grants_file) = with_grants(&rest)?;
    let server = ["--audit", "--listen", "--ws-origin", "--host"];
    if let Some(flag) = flags.iter().step_by(2).find(|f| server.contains(&f.as_str())) {
        return Err(fail(format!(
            "`{flag}` is not for kio test\n  hint: remove it from the grants file; a test runs once, without a server or an audit log"
        )));
    }
    // Checked once here, as `run_program` checks them, so a bad value is a usage error, not a
    // failure of every test.
    for pair in flags.chunks(2) {
        let [flag, n] = pair else { continue };
        if !LIMIT_FLAGS.contains(&flag.as_str()) && flag != "--kv-mb" && flag != "--db-mb" {
            continue;
        }
        let Some(v) = n.parse::<u64>().ok().filter(|&v| v > 0) else {
            return Err(fail(format!("{flag} {n}: expected a positive integer")));
        };
        if flag == "--stack" && v > (kio_host::MAX_STACK >> 10) as u64 {
            return Err(fail(format!("--stack {n}: at most {} KB", kio_host::MAX_STACK >> 10)));
        }
    }
    let (files, read_ids) = read_program(path)?;
    let pairs: Vec<(&str, &str)> = files.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
    let program = match lang::check_files_for_tests(&pairs, &lang::CapTable::v0()) {
        Ok(p) => p,
        Err(diags) => {
            print_diags(&files, diags.into_iter().map(Diag::from).collect(), "check");
            return Err(2);
        }
    };
    // A filter that matches nothing is a typo, not a pass.
    if !filter.is_empty() && !program.tests().iter().any(|t| t.name.contains(&filter)) {
        return Err(fail(format!(
            "no test matches `{filter}`\n  hint: a test runs when its name (`util::t` in a module) contains the filter"
        )));
    }
    Ok(Tests {
        files,
        read_ids,
        flags,
        grants_file,
        program,
        filter,
        path: path.to_string(),
        cases,
        seed,
    })
}

/// Runs test `t` of `tests`: an example test once, a property on its cases (`run_property`).
fn run_test(tests: &Tests, t: &lang::TestFn) -> TestResult {
    let start = std::time::Instant::now();
    let (error, log, prop) = match t.inputs.is_empty() {
        true => {
            let r = run_once(tests, t, None);
            (r.error, r.log, None)
        }
        false => {
            let (error, log, info) = run_property(tests, t);
            (error, log, Some(info))
        }
    };
    TestResult {
        name: t.name.clone(),
        error,
        ms: start.elapsed().as_secs_f64() * 1000.0,
        log,
        prop,
    }
}

/// One run of a test: its exit code, its error as `--json` shows one, and its `Log` lines.
struct Once {
    code: i32,
    error: Option<serde_json::Value>,
    log: String,
}

/// Runs test `t` of `tests` once, on `case` for a property: the grants file's grant for each
/// parameter it names, else a fresh one by type (a new empty temporary `Dir`, `:memory:` `Kv`
/// and `Db`, `Clock` at `TEST_CLOCK`, `Rng` seed 1, a `Log` into a buffer), all bound and checked
/// by `run_program` as `kio run` binds and checks `main`'s.
fn run_once(tests: &Tests, t: &lang::TestFn, case: Option<(lang::prop::Case, bool)>) -> Once {
    // A grant to a parameter of another test, or of another type, is not this test's: one file
    // can grant `k: Kv` to one test and `k: Hmac` to another.
    let mut flags: Vec<String> = Vec::new();
    let mut named = Vec::new();
    for pair in tests.flags.chunks(2) {
        if let Some((_, ty)) = NAMED.iter().find(|(f, _)| *f == pair[0]) {
            let name = pair[1].split('=').next().unwrap_or_default();
            if !t.params.iter().any(|(n, t)| n == name && t == ty) {
                continue;
            }
            named.push(name.to_string());
        }
        flags.extend(pair.iter().cloned());
    }
    let mut temps = Vec::new();
    let mut logs = Vec::new();
    let log = SharedBuf::default();
    let mut missing = None;
    for (name, ty) in t.params.iter().filter(|(n, _)| !named.contains(n)) {
        let default = match ty.as_str() {
            "Dir" => match TempDir::new() {
                Ok(d) => {
                    let spec = format!("{name}={}", d.0.display());
                    temps.push(d);
                    Some(("--dir", spec))
                }
                Err(e) => {
                    missing = Some(("internal", format!("cannot make a temporary directory: {e}"), None));
                    break;
                }
            },
            "Kv" => Some(("--kv", format!("{name}=:memory:"))),
            "Db" => Some(("--db", format!("{name}=:memory:"))),
            "Clock" => Some(("--clock", format!("{name}={TEST_CLOCK}"))),
            "Rng" => Some(("--rng", format!("{name}=seed:1"))),
            "Log" => {
                logs.push((name.clone(), LogSink::new(Box::new(log.clone()), "test")));
                None
            }
            "Http" | "Hmac" => {
                let msg = format!("test `{}` needs `{name}: {ty}`", t.name);
                missing = Some(("grant", msg, Some("name it in a --grants file")));
                break;
            }
            _ => {
                let msg = format!("test `{}` needs `{name}: {ty}`", t.name);
                let hint =
                    "only `kio run --listen` grants a server's capabilities; test the handler's logic in a function";
                missing = Some(("grant", msg, Some(hint)));
                break;
            }
        };
        if let Some((flag, spec)) = default {
            flags.extend([flag.to_string(), spec]);
        }
    }
    let (code, out) = captured(true, || match missing {
        Some((kind, msg, hint)) => {
            push_error(kind, &msg, hint, None);
            1
        }
        None => {
            let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
            let run = TestRun {
                program: &tests.program,
                test: t,
                logs,
                case,
            };
            let grants = tests.grants_file.as_deref();
            run_program(&tests.files, &tests.read_ids, &flags, grants, false, Some(run))
        }
    });
    drop(temps);
    let text = String::from_utf8_lossy(&log.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).into_owned();
    let mut error = out.errors.into_iter().next();
    if code != 0 && error.is_none() {
        error = Some(
            json!({"kind": "internal", "message": "the test failed without an error", "hint": null,
            "file": null, "line": null, "col": null}),
        );
    }
    Once { code, error, log: text }
}

/// `--cases` unless given, and its most.
const DEFAULT_CASES: usize = 100;
const MAX_CASES: usize = 100_000;

/// The most runs shrinking a failing case may take.
const MAX_SHRINK_RUNS: usize = 200;

/// What one case did: passed, was discarded by `assume`, or failed with an error as `--json`
/// shows one.
#[derive(Debug, PartialEq)]
enum Verdict {
    Pass,
    Discard,
    Fail(serde_json::Value),
}

impl Verdict {
    fn of(r: &Once) -> Verdict {
        match &r.error {
            _ if r.code == 0 => Verdict::Pass,
            Some(e) if e["kind"] == "runtime" && e["message"] == lang::eval::DISCARDED => Verdict::Discard,
            Some(e) => Verdict::Fail(e.clone()),
            None => Verdict::Fail(test_error("internal", "the test failed without an error", None)),
        }
    }

    /// A failure the two engines need not agree on: a limit (compiled code has fuel, memory and
    /// stack limits the interpreter lacks), the interpreter's call depth, or a host error.
    fn unchecked(&self) -> bool {
        let Verdict::Fail(e) = self else { return false };
        let kinds = ["fuel", "memory", "timeout", "stack", "calls", "io", "internal", "grant"];
        kinds.iter().any(|k| e["kind"] == *k) || e["message"] == "evaluation is too deep"
    }

    fn describe(&self) -> String {
        match self {
            Verdict::Pass => "passed".to_string(),
            Verdict::Discard => "discarded".to_string(),
            Verdict::Fail(e) => {
                let at = match (e["line"].as_u64(), e["col"].as_u64()) {
                    (Some(l), Some(c)) => format!(" at {l}:{c}"),
                    _ => String::new(),
                };
                format!("{}{at}", e["message"].as_str().unwrap_or_default())
            }
        }
    }
}

fn test_error(kind: &str, message: &str, hint: Option<&str>) -> serde_json::Value {
    json!({"kind": kind, "message": message, "hint": hint, "file": null, "line": null, "col": null})
}

/// Why property `t` may not take the grant the grants file gives one of its parameters: an
/// `Http` (every case would send real requests), a writable `Dir`, `Kv` or `Db` (cases would
/// share its state), or a `Log` to a file (cases would share it). A parameter the file leaves out gets a fresh grant per case.
fn property_effect(tests: &Tests, t: &lang::TestFn) -> Option<(String, String)> {
    for (name, ty) in &t.params {
        if ty == "Http" {
            let msg = format!("a property runs many times; `{name}: Http` would repeat real requests");
            return Some((
                msg,
                "test the call in an example test, or pass the data as a parameter".into(),
            ));
        }
        let flag = match ty.as_str() {
            "Log" => {
                let to_file = |pair: &[String]| {
                    pair[0] == "--log" && pair[1].split_once('=').is_some_and(|(n, t)| n == name && t != "-")
                };
                if tests.flags.chunks(2).any(to_file) {
                    let msg = format!("a property runs many times; `{name}: Log` from the grants file would share one file across cases");
                    let hint =
                        format!("use `--log {name}=-`, or leave it out of the grants file for a buffer per case");
                    return Some((msg, hint));
                }
                continue;
            }
            "Dir" => "--dir",
            "Kv" => "--kv",
            "Db" => "--db",
            _ => continue,
        };
        for pair in tests.flags.chunks(2) {
            let Some((n, target)) = pair[1].split_once('=') else {
                continue;
            };
            if pair[0] == flag && n == name && !target.ends_with(":ro") && target != ":memory:" {
                let msg = format!("a property runs many times; `{name}: {ty}` from the grants file would share its writes across cases");
                let hint = "grant it `:ro`, or leave it out of the grants file for a fresh one per case";
                return Some((msg, hint.into()));
            }
        }
    }
    None
}

/// Runs property `t` on `tests.cases` generated cases, each compiled in a fresh instance with
/// fresh grants. A failing case is shrunk, each candidate a compiled run, and the smallest runs
/// once more. Its error, the failing run's log, and how the cases ran.
fn run_property(tests: &Tests, t: &lang::TestFn) -> (Option<serde_json::Value>, String, PropInfo) {
    let mut info = PropInfo {
        seed: tests.seed,
        ..PropInfo::default()
    };
    if let Some((msg, hint)) = property_effect(tests, t) {
        return (Some(test_error("grant", &msg, Some(&hint))), String::new(), info);
    }
    let m = lang::lower::lower_entry(&tests.program, &lang::CapTable::v0(), &t.name);
    let (names, tys): (Vec<String>, Vec<lang::ir::Ty>) = lang::prop::inputs(&m).into_iter().unzip();
    // Clipped as assert text is (open question 240), all literals together: the program chose
    // the types.
    let shown = |vs: &[lang::Value]| -> Vec<(String, String)> {
        let mut left = lang::eval::ASSERT_TEXT_MAX;
        let mut each = Vec::new();
        for (name, v) in names.iter().zip(vs) {
            let literal = lang::prop::debug_literal(&m, v);
            each.push((name.clone(), cut(&literal, left)));
            left = left.saturating_sub(literal.len());
        }
        each
    };
    for i in 0..tests.cases {
        let mut rng = lang::prop::Rng::for_case(tests.seed, i as u64);
        let size = lang::prop::size_of_case(i);
        let values: Vec<lang::Value> = tys
            .iter()
            .map(|ty| lang::prop::generate(&m, ty, &mut rng, size))
            .collect();
        info.compiled += 1;
        let (verdict, log) = run_case(tests, t, &values);
        let error = match verdict {
            Verdict::Pass => continue,
            Verdict::Discard => {
                info.discarded += 1;
                continue;
            }
            Verdict::Fail(e) => e,
        };
        if Verdict::Fail(error.clone()).unchecked() || error["kind"] == "internal" {
            info.input = Some((shown(&values), false));
            return (Some(error), log, info);
        }
        // Shrink to a smaller case failing the same way, then run that again.
        let kind = error["kind"].clone();
        let fails = |vs: &[lang::Value]| {
            let r = run_once(tests, t, Some((lang::prop::Case::new(vs), false)));
            matches!(Verdict::of(&r), Verdict::Fail(e) if e["kind"] == kind)
        };
        let (minimal, runs) = lang::prop::minimize(&m, &tys, values, MAX_SHRINK_RUNS, fails);
        info.shrinks = runs;
        let (verdict, log) = run_case(tests, t, &minimal);
        info.input = Some((shown(&minimal), true));
        let error = match verdict {
            Verdict::Fail(e) => e,
            // The smallest case passed when run again: the property is not deterministic.
            other => test_error(
                "internal",
                &format!("the shrunk input {} when run again", other.describe()),
                Some("make the test depend only on its inputs and grants"),
            ),
        };
        return (Some(error), log, info);
    }
    if info.discarded * 10 > info.compiled * 9 {
        let msg = format!("`{}` discarded {} of {} inputs", t.name, info.discarded, info.compiled);
        let hint = "loosen `assume`, or build the inputs inside the test";
        return (Some(test_error("discard", &msg, Some(hint))), String::new(), info);
    }
    (None, String::new(), info)
}

/// Runs one case compiled, in a fresh instance with fresh grants: its verdict and log. In test
/// builds the interpreter runs it too, as the oracle (`oracle`).
fn run_case(tests: &Tests, t: &lang::TestFn, values: &[lang::Value]) -> (Verdict, String) {
    let case = lang::prop::Case::new(values);
    let compiled = run_once(tests, t, Some((case.clone(), false)));
    let verdict = Verdict::of(&compiled);
    #[cfg(feature = "test-hooks")]
    let verdict = oracle(tests, t, case, verdict);
    #[cfg(not(feature = "test-hooks"))]
    drop(case);
    (verdict, compiled.log)
}

/// Test builds only: kio's own CI checks compiled code against the interpreter on every property
/// case its tests run. Unless compiled code stopped at a limit, runs the case in the interpreter
/// with fresh grants; a different verdict is a failure naming both.
#[cfg(feature = "test-hooks")]
fn oracle(tests: &Tests, t: &lang::TestFn, case: lang::prop::Case, compiled: Verdict) -> Verdict {
    if compiled.unchecked() {
        return compiled;
    }
    let interpreted = Verdict::of(&run_once(tests, t, Some((case, true))));
    if interpreted.unchecked() || interpreted == compiled {
        return compiled;
    }
    let msg = format!(
        "compiled and interpreted runs disagree: compiled {}, interpreted {}",
        compiled.describe(),
        interpreted.describe()
    );
    let hint = "a kio bug: please report it with the input and the rerun line";
    Verdict::Fail(test_error("internal", &msg, Some(hint)))
}

/// `s`, or its first `max` bytes cut back to a character boundary and `...` added.
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = (0..=max).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}...", &s[..end])
}

/// `s` as one shell word: as is if it is plain, else single-quoted.
fn shell_word(s: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./:".contains(c);
    match !s.is_empty() && s.chars().all(plain) {
        true => s.to_string(),
        false => format!("'{}'", s.replace('\'', "'\\''")),
    }
}

/// A buffer a test's `Log` writes to, shown if the test fails.
#[derive(Clone, Default)]
struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let mut v = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A test's own directory: new, empty and private (mode 0700), under the system's temporary
/// directory with a name no other process chose (`mkdir` fails on any existing name, a symlink
/// included). Removed when dropped; `remove_dir_all` never follows a symlink inside it.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        use std::hash::BuildHasher;
        use std::os::unix::fs::DirBuilderExt;
        static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        loop {
            let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let salt = std::collections::hash_map::RandomState::new().hash_one(n);
            let path = std::env::temp_dir().join(format!("kio-test-{}-{n}-{salt:016x}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(TempDir(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `run --listen`: check and compile once, then answer requests until Ctrl-C. `audit` is the
/// log's path and each `Dir` grant's `(name, path)`.
#[allow(clippy::too_many_arguments)]
fn serve(
    files: Vec<(String, String)>,
    dirs: Vec<(String, cap_std::fs::Dir)>,
    https: Vec<(String, kio_host::Origin)>,
    kvs: kio_host::KvGrants,
    sources: Sources,
    addr: std::net::SocketAddr,
    limits: Limits,
    ws_origins: Vec<String>,
    hosts: Vec<String>,
    audit: Option<(String, kio_host::DirPaths)>,
) -> i32 {
    let mut server = match Server::new(files.clone(), limits) {
        Err(e) => return internal(format!("{e:#}")),
        Ok(Err(diags)) => return print_diags(&files, diags, "check"),
        Ok(Ok(s)) => s,
    };
    server.grant_https(https);
    server.grant_kvs(kvs);
    server.grant_sources(sources);
    // A grant `main` does not take is refused before binding, exit 1 as plain `kio run` (H21).
    let names: Vec<String> = dirs.iter().map(|(n, _)| n.clone()).collect();
    if let Err(diags) = server.check_grants(&names) {
        return print_diags(&files, diags, "grant");
    }
    if let Some((path, dir_paths)) = audit {
        let opened = open_append(&path).and_then(|f| server.audit(f, dir_paths));
        if let Err(e) = opened {
            return refuse(format!("--audit {path}: {e}"));
        }
    }
    match listen::serve(server, dirs, addr, ws_origins, hosts) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

/// Whether `a` and `b` name one file: the same path once symlinks resolve, or the same device
/// and inode (so a hard link too).
fn same_file(a: &str, b: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let id = |p: &str| std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()));
    let resolve = |p: &str| kio_host::resolve(p.as_ref());
    id(a).is_some() && id(a) == id(b) || resolve(a).is_some_and(|r| Some(r) == resolve(b))
}

/// Why the file `path` of `flag` (`--audit`, `--log name`) is refused, if it is: it must not be a
/// `--kv` store's or `--db` database's file (one of `stores`; writing it would corrupt it), lie
/// inside a granted `--dir` (the program could read or replace its own log), or be the file of
/// another flag in `taken`. Checked before any of them is opened.
fn file_refusal(
    flag: &str,
    path: &str,
    stores: &[(String, String)],
    dirs: &[Granted],
    taken: &[(String, &str)],
) -> Option<String> {
    let what = if flag == "--audit" { "the audit log" } else { "the log" };
    if kio_host::resolve(path.as_ref()).is_none() {
        return Some(format!(
            "`{flag} {path}` has `.` or `..` past the directories that exist\n  hint: name {what} by a plain path"
        ));
    }
    let same = |p: &str| same_file(path, p);
    for (other, p) in stores {
        if same(p) {
            return Some(format!(
                "`{flag} {path}` is the same file as `{other}`\n  hint: give {what} its own file; writing it would corrupt the store"
            ));
        }
    }
    for (other, p) in taken {
        if same(p) {
            return Some(format!(
                "`{flag} {path}` is the same file as `{other}`\n  hint: give each log its own file"
            ));
        }
    }
    for (d, _, id) in dirs {
        if holds(*id, path) {
            return Some(format!(
                "`{flag} {path}` is inside `--dir {d}`\n  hint: keep {what} outside granted directories, where the program cannot read or replace it"
            ));
        }
    }
    None
}

/// Open a log (`--audit`, `--log`) to append, creating it: an existing file is never truncated,
/// and a symlink is never followed (it could lead past `file_refusal`'s checks).
fn open_append(path: &str) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
}

/// The file index and 1-based line of each diag. Any file number or offset maps to some line,
/// never a panic.
fn locate(files: &[(String, String)], diags: &[Diag]) -> Vec<(usize, usize)> {
    // Byte offset of each line start, one table per file: per-diag scans are quadratic in the source.
    let starts: Vec<Vec<usize>> = files
        .iter()
        .map(|(_, src)| {
            std::iter::once(0)
                .chain(src.match_indices('\n').map(|(i, _)| i + 1))
                .collect()
        })
        .collect();
    diags
        .iter()
        .map(|d| {
            let file = (d.file as usize).min(files.len() - 1);
            (file, starts[file].partition_point(|&s| s <= d.start as usize).max(1))
        })
        .collect()
}

/// Each diag, of `kind` for `--json`; exit 1.
fn print_diags(files: &[(String, String)], diags: Vec<Diag>, kind: &str) -> i32 {
    for (d, (file, line)) in diags.iter().zip(locate(files, &diags)) {
        if json_out() {
            let at = (json_path(files, file), line, column(&files[file].1, d.start as usize));
            push_error(kind, &d.message, d.hint.as_deref(), Some(at));
            continue;
        }
        eprintln!("{}:{line}: error: {}", files[file].0, d.message);
        if let Some(h) = &d.hint {
            eprintln!("  hint: {h}");
        }
    }
    1
}

/// A malformed command line: exit 2.
fn fail(msg: String) -> i32 {
    error("usage", &msg)
}

/// A grant the host refuses before running: exit 2.
fn refuse(msg: String) -> i32 {
    error("grant", &msg)
}

/// The engine or server cannot start: exit 2.
fn internal(msg: String) -> i32 {
    error("internal", &msg)
}

/// `msg` (its hint after `\n  hint: `) without a span, of `kind` for `--json`; exit 2.
fn error(kind: &str, msg: &str) -> i32 {
    if !json_out() {
        eprintln!("kio: {msg}");
        return 2;
    }
    let (message, hint) = match msg.split_once("\n  hint: ") {
        Some((m, h)) => (m, Some(h)),
        None => (msg, None),
    };
    push_error(kind, message, hint, None);
    2
}

/// Add an error to `--json`'s object; `at` is its file, line and column.
fn push_error(kind: &str, message: &str, hint: Option<&str>, at: Option<(String, usize, usize)>) {
    let (file, line, col) = at.map_or((None, None, None), |(f, l, c)| (Some(f), Some(l), Some(c)));
    let e = json!({"kind": kind, "message": message, "hint": hint, "file": file, "line": line, "col": col});
    set_json(|o| o.errors.push(e));
}

/// A run stopped by a limit or by the host (`Kio::explain`'s `msg`). For `--json` its kind is
/// the limit, and "raise it with --x" is the hint.
fn stopped(msg: &str) {
    if !json_out() {
        eprintln!("{msg}");
        return;
    }
    let raise = |h: &str| format!("raise it with {}", h.trim_end_matches(')'));
    let (message, hint) = match (msg.split_once(" (raise it with "), msg.split_once("; raise it with ")) {
        (Some((m, h)), _) => (m.to_string(), Some(raise(h))),
        (_, Some((m, h))) => (format!("{m})"), Some(raise(h))),
        _ => (msg.to_string(), None),
    };
    push_error(kio_host::stop_kind(msg), &message, hint.as_deref(), None);
}

/// The path of source file `i` for `--json`: the root as given, a module beside it.
fn json_path(files: &[(String, String)], i: usize) -> String {
    if i == 0 {
        return files[0].0.clone();
    }
    let dir = std::path::Path::new(&files[0].0).parent().unwrap_or(".".as_ref());
    dir.join(&files[i].0).display().to_string()
}

/// The 1-based column of byte `at` in `src`, in characters from its line's start.
fn column(src: &str, at: usize) -> usize {
    let at = (0..=at.min(src.len()))
        .rev()
        .find(|&i| src.is_char_boundary(i))
        .unwrap_or(0);
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    src[start..at].chars().count() + 1
}

/// `main`'s value as `--json` shows it (D2): `printed` as `kio run` prints it, but `null` for
/// `()`, and for a `Result` its `Ok` payload alone, a `Str` payload unquoted.
fn json_value(printed: &str, ret: Option<&lang::check::Ty>) -> Option<String> {
    use lang::check::Ty;
    let Some(Ty::Result(ok, _)) = ret else {
        return (ret != Some(&Ty::Unit)).then(|| printed.to_string());
    };
    let inner = printed
        .strip_prefix("Ok(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(printed);
    match **ok {
        Ty::Unit => None,
        Ty::Str => Some(unquote(inner).unwrap_or_else(|| inner.to_string())),
        _ => Some(inner.to_string()),
    }
}

/// `s`, a string as Rust's `{:?}` prints it (as the runtime prints a nested `Str`), unquoted;
/// `None` if it is not one.
fn unquote(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.strip_suffix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            'u' => {
                let hex: String = chars.by_ref().skip(1).take_while(|&c| c != '}').collect();
                char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
            }
            c => c,
        });
    }
    Some(out)
}
