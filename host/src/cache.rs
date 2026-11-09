//! `kio run`'s cache of compiled programs (C0 decide 1). A program is compiled to native code
//! once; later runs of the same program load it. Loading native code runs it as trusted, so the
//! cache is a directory only this user can write, checked on every use, and each entry carries
//! a SHA-256 of its contents, so a torn or corrupted file is recompiled, never loaded.
//!
//! An entry is `<sha256 of kio's version and the program's Wasm>.cwasm`: the native code, then
//! the SHA-256 of it. Wasmtime refuses (and the cache then replaces) an entry made by another
//! Wasmtime version or with other engine settings.

use sha2::{Digest, Sha256};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use wasmtime::component::Component;
use wasmtime::{Engine, Result};

/// Entries kept; storing one more removes the least recently written. About 0.7 MB each.
const MAX_ENTRIES: usize = 64;

/// The cache directory: `$XDG_CACHE_HOME/kio`, else `~/.cache/kio`. `None` without a home.
pub fn default_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").filter(|v| !v.is_empty())?).join(".cache"),
    };
    Some(base.join("kio"))
}

/// `path` with symlinks resolved, though it may not exist yet: its nearest existing ancestor made
/// canonical, then the rest. `None` when the rest has a `..` or `.`, which could lead anywhere.
pub fn resolve(path: &Path) -> Option<PathBuf> {
    let path = std::path::absolute(path).ok()?;
    let (base, rest) = path
        .ancestors()
        .find_map(|a| Some((std::fs::canonicalize(a).ok()?, path.strip_prefix(a).ok()?)))?;
    let normal = rest.components().all(|c| matches!(c, std::path::Component::Normal(_)));
    normal.then(|| base.join(rest))
}

/// Whether a program granted the directory at `root` could write inside the cache at `dir`, or
/// `root` is inside the cache: then the cache is not used, since an entry the program wrote
/// would be loaded as native code. Both paths as `resolve` gives them.
pub fn overlaps(dir: &Path, root: &Path) -> bool {
    dir.starts_with(root) || root.starts_with(dir)
}

/// The compiled component for `wasm`, from `dir` if it holds a good entry; otherwise compiled,
/// and stored there when `dir` is usable. A cache that cannot be used only costs the compile.
pub fn component(engine: &Engine, wasm: &[u8], dir: &Path) -> Result<Component> {
    let usable = private_dir(dir);
    let mut sha = Sha256::new();
    sha.update(env!("CARGO_PKG_VERSION"));
    sha.update([0]);
    sha.update(wasm);
    let key: String = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("{key}.cwasm"));
    if usable {
        if let Some(native) = read_entry(&path) {
            // SAFETY: the entry is in a directory only this user can write, owned by this user,
            // and its contents match the hash written with them: it is native code this process's
            // user compiled. Wasmtime still checks its version and settings match `engine`.
            if let Ok(c) = unsafe { Component::deserialize(engine, native) } {
                return Ok(c);
            }
        }
    }
    let native = engine.precompile_component(wasm)?;
    if usable {
        evict(dir);
        write_entry(dir, &path, &native);
    }
    // SAFETY: `native` was just compiled by `engine` from `wasm`.
    unsafe { Component::deserialize(engine, &native) }
}

/// Whether `dir` exists or can be made, is a real directory owned by this user, and no one else
/// can write it (mode 0700 when made here).
fn private_dir(dir: &Path) -> bool {
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
    let Ok(m) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    m.is_dir() && m.uid() == rustix::process::getuid().as_raw() && m.permissions().mode() & 0o022 == 0
}

/// The native code in the entry at `path`, if it is a regular file owned by this user whose last
/// 32 bytes are the SHA-256 of the rest.
fn read_entry(path: &Path) -> Option<Vec<u8>> {
    let m = std::fs::symlink_metadata(path).ok()?;
    if !m.is_file() || m.uid() != rustix::process::getuid().as_raw() {
        return None;
    }
    let mut bytes = std::fs::read(path).ok()?;
    let split = bytes.len().checked_sub(32)?;
    if Sha256::digest(&bytes[..split]).as_slice() != &bytes[split..] {
        return None;
    }
    bytes.truncate(split);
    Some(bytes)
}

/// Remove the oldest entries until there is room for one more.
fn evict(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".cwasm"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    entries.sort();
    let extra = (entries.len() + 1).saturating_sub(MAX_ENTRIES);
    for (_, path) in entries.into_iter().take(extra) {
        let _ = std::fs::remove_file(path);
    }
}

/// Store `native` at `path`: written to a temporary file in `dir`, then renamed into place, so a
/// concurrent run reads the old entry, the new one, or none. Failures are ignored.
fn write_entry(dir: &Path, path: &Path, native: &[u8]) {
    let temp = dir.join(format!(".tmp-{}-{:?}", std::process::id(), std::thread::current().id()));
    let mut bytes = native.to_vec();
    bytes.extend_from_slice(&Sha256::digest(native));
    let written = std::fs::write(&temp, &bytes)
        .and_then(|()| std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)))
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}
