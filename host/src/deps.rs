//! `kio.deps` (PK1): beside the root, one line per module loaded from elsewhere, `NAME  PATH
//! sha256:HEX`, PATH relative to the file. A module listed is read from PATH and must have that
//! hash; `kio deps pin` is the only writer.
use super::*;
use std::io::ErrorKind::NotFound;
use std::path::{Path, PathBuf};

/// A dependency line: its 1-based number, the module's name, its path as written, and its pinned
/// sha256 (hex; `None` until pinned).
pub(crate) struct Line {
    at: usize,
    pub(crate) name: String,
    path: String,
    hash: Option<String>,
}

/// `kio.deps` beside a root: its lines, and the path and identity of the file read (`None` when
/// there is none, which is the same as an empty one).
pub(crate) type Pins = (Vec<Line>, Option<(PathBuf, FileId)>);

const FORM: &str =
    "write `NAME  PATH  sha256:HEX`: NAME a module name, PATH relative to kio.deps; `kio deps pin` writes the hash";

/// `kio.deps` in `dir` (the root's directory), read and parsed.
pub(crate) fn read(dir: &Path) -> Result<Pins, i32> {
    let path = dir.join("kio.deps");
    let shown = path.display().to_string();
    match kio_host::read_source(&path) {
        Ok((text, id)) => Ok((parse(&text, &shown).map_err(|e| error("deps", &e))?, Some((path, id)))),
        Err(e) if e.kind() == NotFound => Ok((vec![], None)),
        Err(e) => Err(error("deps", &cannot_read(&shown, &e))),
    }
}

/// Each dependency whose hash differs between `old` and `new`, by name: `(name, old hash, new
/// hash)`, `None` where it is absent.
pub(crate) fn changes(old: &[kio_host::Dep], new: &[kio_host::Dep]) -> Vec<(String, Option<String>, Option<String>)> {
    let hash = |deps: &[kio_host::Dep], name: &str| deps.iter().find(|d| d.name == name).map(|d| d.sha256.clone());
    let mut names: Vec<&str> = old.iter().chain(new).map(|d| d.name.as_str()).collect();
    names.sort();
    names.dedup();
    (names.into_iter())
        .map(|name| (name.to_string(), hash(old, name), hash(new, name)))
        .filter(|(_, o, n)| o != n)
        .collect()
}

/// The dependency lines of `text`, the file `shown`: `#` comments and blank lines skipped,
/// fields split on runs of spaces or tabs. A malformed or repeated line is an error.
fn parse(text: &str, shown: &str) -> Result<Vec<Line>, String> {
    let mut lines: Vec<Line> = Vec::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (i, l) in text.lines().enumerate().map(|(i, l)| (i + 1, l.trim())) {
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.split([' ', '\t']).filter(|f| !f.is_empty()).collect();
        let hex = |h: &str| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let hash = match f.get(2) {
            None => Some(None),
            Some(h) => h
                .strip_prefix("sha256:")
                .filter(|h| hex(h))
                .map(|h| Some(h.to_string())),
        };
        // PATH relative, so kio.deps reads the same on every checkout; nothing that could reach
        // the terminal raw.
        let plain =
            !f.iter().any(|x| x.chars().any(char::is_control)) && f.get(1).is_some_and(|p| !Path::new(p).is_absolute());
        let (Some(hash), true) = (hash, f.len() >= 2 && f.len() <= 3 && is_ident(f[0]) && plain) else {
            let l = kio_host::escape_line(l);
            return Err(format!("{shown}:{i}: `{l}` is not a dependency line\n  hint: {FORM}"));
        };
        if let Some(first) = lines.iter().find(|d| d.name == f[0]) {
            return Err(format!(
                "{shown}:{i}: `{}` is listed twice (also line {})\n  hint: keep one line per module",
                f[0], first.at
            ));
        }
        let (name, path) = (f[0].to_string(), f[1].to_string());
        lines.push(Line {
            at: i,
            name,
            path,
            hash,
        });
    }
    Ok(lines)
}

/// Read the module `line` lists into `prog`, for the root's `mod` line at `span`: from its path,
/// relative to `dir` (the root's, where `kio.deps` is), and only if its contents have the pinned
/// hash. The bytes hashed are the bytes compiled. An error is shown at `span`.
pub(crate) fn load(prog: &mut Program, dir: &Path, line: &Line, span: (usize, usize)) -> Result<(), i32> {
    let (shown, at, name) = (dir.join("kio.deps").display().to_string(), line.at, &line.name);
    let root = &prog.files[0].0;
    let pin = format!("`kio deps pin {root}`");
    let file = Path::new(root)
        .file_name()
        .map_or(root.clone(), |f| f.to_string_lossy().into());
    let err = |prog: &Program, msg: String| {
        let (message, hint) = match msg.split_once("\n  hint: ") {
            Some((m, h)) => (m.to_string(), Some(h.to_string())),
            None => (msg, None),
        };
        let (start, end) = (span.0 as u32, span.1 as u32);
        print_diags(
            prog,
            vec![Diag {
                start,
                end,
                message,
                hint,
                file: 0,
            }],
            "deps",
        );
        Err(2)
    };
    if dir.join(format!("{name}.kio")).symlink_metadata().is_ok() {
        return err(
            prog,
            format!("`{name}` is in {shown}:{at} and beside {file}\n  hint: delete one"),
        );
    }
    let Some(pinned) = &line.hash else {
        return err(
            prog,
            format!("`{name}` has no hash in {shown}:{at}\n  hint: review it, then {pin}"),
        );
    };
    let path = dir.join(&line.path);
    let (src, id) = match kio_host::read_source(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == NotFound => {
            let hint = "a path in kio.deps is relative to the directory kio.deps is in";
            return err(
                prog,
                format!("{shown}:{at}: {} does not exist\n  hint: {hint}", line.path),
            );
        }
        Err(e) => return err(prog, format!("{shown}:{at}: {}", cannot_read(&line.path, &e))),
    };
    let found = sha256_hex(src.as_bytes());
    if found != *pinned {
        let (p, f) = (short(pinned), short(&found));
        return err(
            prog,
            format!(
                "{} does not match {shown}:{at} ({p} pinned, {f} found)\n  hint: review the change, then {pin}",
                line.path
            ),
        );
    }
    prog.files.push((format!("{name}.kio"), src));
    prog.paths.push(path);
    prog.ids.push(id);
    let (name, path) = (name.clone(), line.path.clone());
    prog.deps.push(kio_host::Dep {
        name,
        path,
        sha256: found,
    });
    Ok(())
}

/// `kio deps pin [ROOT.kio]`: rewrite each line's hash in `kio.deps` beside the root (`./kio.deps`
/// without one) from its file as it is now, keeping comments and order; print each line changed.
pub(crate) fn pin(root: Option<&str>) -> i32 {
    let dir = root.map_or("".as_ref(), |r| Path::new(r).parent().unwrap_or("".as_ref()));
    let path = dir.join("kio.deps");
    let shown = path.display().to_string();
    let text = match kio_host::read_source(&path) {
        Ok((t, _)) => t,
        Err(e) => return fail(format!("{shown}: cannot read: {e}\n  hint: create it beside the root, a line `NAME  PATH` per module; `kio deps pin` adds the hashes")),
    };
    let lines = match parse(&text, &shown) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    let (mut out, mut changed) = (String::new(), Vec::new());
    for (i, raw) in text.split_inclusive('\n').enumerate() {
        let Some(l) = lines.iter().find(|l| l.at == i + 1) else {
            out.push_str(raw);
            continue;
        };
        // Read as `load` reads it: a file it would refuse is refused here.
        let found = match kio_host::read_source(&dir.join(&l.path)) {
            Ok((src, _)) => sha256_hex(src.as_bytes()),
            Err(e) if e.kind() == NotFound => {
                return fail(format!(
                "{shown}:{}: {} does not exist\n  hint: a path in kio.deps is relative to the directory kio.deps is in",
                l.at, l.path
            ))
            }
            Err(e) => return fail(format!("{shown}:{}: {}", l.at, cannot_read(&l.path, &e))),
        };
        if l.hash.as_ref() == Some(&found) {
            out.push_str(raw);
            continue;
        }
        let body = raw.trim_end_matches(['\n', '\r']);
        match l.hash.as_ref().and_then(|h| body.rfind(h.as_str())) {
            Some(k) => out.extend([&body[..k], &found, &body[k + found.len()..]]),
            None => out.extend([body.trim_end(), "  sha256:", &found]),
        }
        out.push_str(&raw[body.len()..]);
        let old = l.hash.as_deref().map_or("none".into(), short);
        changed.push(format!("{}  {old} -> {}", l.name, short(&found)));
    }
    if changed.is_empty() {
        println!("nothing changed");
        return 0;
    }
    if let Err(e) = std::fs::write(&path, out) {
        return fail(format!("{shown}: cannot write: {e}"));
    }
    changed.iter().for_each(|c| println!("{c}"));
    0
}

/// `sha256:` and the first 12 hex digits of `hash`, for a message.
pub(crate) fn short(hash: &str) -> String {
    format!("sha256:{}..", &hash[..12.min(hash.len())])
}

fn sha256_hex(bytes: &[u8]) -> String {
    lang::digest::to_hex(&lang::digest::sha256(bytes))
}
