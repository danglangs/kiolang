//! The `Kv` capability (K1): a key-value store the host keeps for the process, in memory or in an
//! append-only log file. One `Store` per `--kv` grant, shared by every run and request.
//!
//! File: the 8-byte `MAGIC`, then records `[len u32][crc32 u32][body]` (little-endian), where the
//! body is `[tag u8][key len u32][key][value]`: tag 1 sets `key` to `value`, tag 2 deletes it.
//!
//! A read-only grant (`path:ro`, H12) takes no lock, so another process may write the file: before
//! each `get` and `keys` the reader applies the whole records appended since, and replays the file
//! again when the path names a new file (the writer compacted it) or the file shrank.
use crate::kio::caps::kv::{HostKv, Kv, KvErr};
use crate::{outcome, State};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Bound;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use wasmtime::component::Resource;
use wasmtime::Result;

const MAGIC: &[u8; 8] = b"kio-kv1\n";
const SET: u8 = 1;
const DELETE: u8 = 2;
/// Longest key, in bytes.
const KEY_MAX: usize = 1024;
/// Largest value.
const VALUE_MAX: usize = 1 << 20;
/// Most keys one `keys` call returns.
const KEYS_MAX: usize = 10_000;
/// Largest record body a valid file holds.
const BODY_MAX: usize = 1 + 4 + KEY_MAX + VALUE_MAX;
/// Record framing plus body header: what a record adds to its key and value.
const OVERHEAD: u64 = 8 + 1 + 4;
/// A log is compacted while running once it is over twice its compacted size plus this.
const COMPACT_SLACK: u64 = 1 << 20;

/// One granted store.
pub struct Store {
    /// The file, or `:memory:`.
    target: String,
    /// Most live key plus value bytes.
    limit: u64,
    /// Device and inode of the file, to refuse two grants of one file.
    id: Option<(u64, u64)>,
    /// Granted `:ro`: writes stop the run, and reads follow another process's writes.
    read_only: bool,
    inner: Mutex<Inner>,
}

struct Inner {
    map: BTreeMap<String, Vec<u8>>,
    /// Sum of live key and value lengths (not kept by a reader, which never writes).
    live: u64,
    /// The log; `None` in memory.
    log: Option<Log>,
}

/// The open log and its path. A writer holds the file's lock and `len` is the file's length; a
/// reader holds no lock, `len` is where the last whole record it applied ends, and `id` is the
/// file's device and inode, to notice a compaction's rename.
struct Log {
    file: File,
    len: u64,
    path: PathBuf,
    id: (u64, u64),
}

/// The path of a `--kv` spec and whether it ends in `:ro`.
pub fn split_spec(spec: &str) -> (&str, bool) {
    match spec.strip_suffix(":ro") {
        Some(path) if !path.is_empty() => (path, true),
        _ => (spec, false),
    }
}

/// The first write (`set`, `delete`, `swap`) an authority report's `main` line shows on `name`.
pub fn write_in_report(report: &str, name: &str) -> Option<&'static str> {
    let calls = crate::main_calls(report);
    ["set", "delete", "swap"]
        .into_iter()
        .find(|m| calls.contains(&(name.to_string(), m.to_string())))
}

/// Open every `(name, spec)` grant with `mb` megabytes each, or say which one cannot be and why.
pub fn open_all(specs: &[(String, String)], mb: u64) -> std::result::Result<Vec<(String, Arc<Store>)>, String> {
    let limit = mb.saturating_mul(1 << 20);
    let mut out: Vec<(String, Arc<Store>)> = Vec::new();
    for (name, spec) in specs {
        let flag = format!("--kv {name}={spec}");
        if out.iter().any(|(n, _)| n == name) {
            return Err(format!(
                "{flag}: `{name}` is granted twice\n  hint: give each store one name"
            ));
        }
        let (path, ro) = split_spec(spec);
        if path == ":memory:" && ro {
            return Err(format!(
                "{flag}: an in-memory store is never read-only\n  hint: drop `:ro`; only this process can write it"
            ));
        }
        if spec == ":memory:" {
            out.push((name.clone(), Arc::new(Store::memory(limit))));
            continue;
        }
        let id = std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
        if let Some((other, _)) = out.iter().find(|(_, s)| id.is_some() && s.id == id) {
            return Err(format!(
                "{flag}: the same file as --kv {other}\n  hint: grant one store once, or give each grant its own file"
            ));
        }
        let store = match ro {
            true => Store::open_read_only(Path::new(path), limit),
            false => Store::open(Path::new(path), limit),
        };
        out.push((name.clone(), Arc::new(store.map_err(|e| format!("{flag}: {e}"))?)));
    }
    Ok(out)
}

impl Store {
    /// A store lost when the process exits.
    pub fn memory(limit: u64) -> Self {
        Store::new(":memory:".into(), limit, BTreeMap::new(), None, false)
    }

    fn new(target: String, limit: u64, map: BTreeMap<String, Vec<u8>>, log: Option<Log>, read_only: bool) -> Self {
        let live = map.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum();
        Store {
            target,
            limit,
            id: log.as_ref().map(|l| l.id),
            read_only,
            inner: Mutex::new(Inner { map, live, log }),
        }
    }

    /// Open the existing log at `path` to read only: no lock, no write, no compaction, so another
    /// process may hold the store and write it. Errors name the file and say what to do.
    pub fn open_read_only(path: &Path, limit: u64) -> std::result::Result<Self, String> {
        let shown = path.display();
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{shown} is a symbolic link\n  hint: grant the file it points to"
            ));
        }
        let (log, map) = read_log(path)?;
        Ok(Store::new(shown.to_string(), limit, map, Some(log), true))
    }

    /// The grant as the audit header shows it: the file, with `:ro` if read-only, or `:memory:`.
    pub fn label(&self) -> String {
        match self.read_only {
            true => format!("{}:ro", self.target),
            false => self.target.clone(),
        }
    }

    /// Open (or create) the log at `path`, lock it, replay it, drop a truncated last record and
    /// compact it when it is over twice its live records. Errors name the file and say what to do.
    pub fn open(path: &Path, limit: u64) -> std::result::Result<Self, String> {
        let shown = path.display();
        let io = |e: io::Error| format!("{shown}: {e}");
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{shown} is a symbolic link\n  hint: grant the file it points to"
            ));
        }
        let Some(mut file) = lock(path).map_err(io)? else {
            return Err(format!(
                "{shown} is in use by another process (it holds the file's lock)\n  hint: stop the other kio, or grant another file"
            ));
        };
        let mut len = file.metadata().map_err(io)?.len();
        let mut head = Vec::new();
        (&file).take(8).read_to_end(&mut head).map_err(io)?;
        if head.len() < MAGIC.len() && MAGIC.starts_with(&head) {
            // New, or its header was cut short: nothing was ever stored.
            file.set_len(0).map_err(io)?;
            file.write_all_at(MAGIC, 0).map_err(io)?;
            len = MAGIC.len() as u64;
        } else if head != MAGIC {
            return Err(format!(
                "{shown} is not a kio store (no `kio-kv1` header)\n  hint: grant a new file; `--kv` creates it"
            ));
        }
        file.seek(SeekFrom::Start(MAGIC.len() as u64)).map_err(io)?;
        let mut map = BTreeMap::new();
        let (end, bad) = replay(&mut BufReader::new(&mut file), &mut map, MAGIC.len() as u64).map_err(io)?;
        // A tail of zeros is what a power loss leaves after a record whose length was written:
        // drop it like a record cut short.
        if bad && !zeros(&file, end, len).map_err(io)? {
            return Err(format!(
                "{shown} is corrupt at byte {end}\n  hint: restore it from a backup, or move it away to start empty"
            ));
        }
        let file = if end > 2 * compacted(&map) {
            rewrite(path, &map).map_err(io)?
        } else {
            if end < len {
                file.set_len(end).map_err(io)?; // the truncated tail
            }
            file
        };
        let m = file.metadata().map_err(io)?;
        let log = Log {
            file,
            len: m.len(),
            path: path.to_path_buf(),
            id: (m.dev(), m.ino()),
        };
        Ok(Store::new(shown.to_string(), limit, map, Some(log), false))
    }

    /// The file, or `:memory:`, for the audit log and errors.
    pub fn target(&self) -> &str {
        &self.target
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic never leaves `Inner` half-updated: the map changes only after the log write.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, key: &str) -> io::Result<Result<Option<Vec<u8>>, KvErr>> {
        if let Err(e) = check_key(key) {
            return Ok(Err(e));
        }
        let inner = self.current()?;
        Ok(Ok(inner.map.get(key).cloned()))
    }

    /// The store, with a read-only one first brought up to date with the file.
    fn current(&self) -> io::Result<std::sync::MutexGuard<'_, Inner>> {
        let mut inner = self.inner();
        if self.read_only {
            inner.follow()?;
        }
        Ok(inner)
    }

    fn set(&self, key: &str, value: Vec<u8>) -> io::Result<Result<(), KvErr>> {
        if let Err(e) = check_key(key) {
            return Ok(Err(e));
        }
        self.inner().put(key, Some(value), self.limit)
    }

    fn delete(&self, key: &str) -> io::Result<Result<(), KvErr>> {
        if let Err(e) = check_key(key) {
            return Ok(Err(e));
        }
        self.inner().put(key, None, self.limit)
    }

    fn keys(&self, prefix: &str) -> io::Result<Result<Vec<String>, KvErr>> {
        if prefix.len() > KEY_MAX {
            return Ok(Err(KvErr::TooLarge));
        }
        let inner = self.current()?;
        let found = || {
            inner
                .map
                .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
                .take_while(|(k, _)| k.starts_with(prefix))
        };
        // Counted before anything is copied.
        if found().take(KEYS_MAX + 1).count() > KEYS_MAX {
            return Ok(Err(KvErr::TooLarge));
        }
        Ok(Ok(found().map(|(k, _)| k.clone()).collect()))
    }

    /// Compare and set under one lock, so no other call sees or changes `key` in between.
    fn swap(&self, key: &str, old: Option<Vec<u8>>, new: Option<Vec<u8>>) -> io::Result<Result<bool, KvErr>> {
        if let Err(e) = check_key(key) {
            return Ok(Err(e));
        }
        let mut inner = self.inner();
        if inner.map.get(key) != old.as_ref() {
            return Ok(Ok(false));
        }
        Ok(inner.put(key, new, self.limit)?.map(|()| true))
    }
}

impl Inner {
    /// A read-only store: apply the whole records another process appended since the last call.
    /// If the path now names another file (a compaction renamed one over it) or the file is
    /// shorter than what was applied, replay the file at the path from the start. A record still
    /// being written, or a tail of zeros, waits for the next call. A bad record where the last one
    /// ended replays the whole file too (it may have been rewritten in place), and is an error
    /// only if that finds the file corrupt; the view is then kept and the next call tries again.
    fn follow(&mut self) -> io::Result<()> {
        let Some(log) = &mut self.log else { return Ok(()) };
        let shown = log.path.display();
        let at_path = match std::fs::symlink_metadata(&log.path) {
            // Removed: keep the view of the file still open.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            r => r?,
        };
        if at_path.file_type().is_symlink() {
            return Err(io::Error::other(format!("{shown} is a symbolic link")));
        }
        if !at_path.is_file() {
            return Err(io::Error::other(format!("{shown}: not a regular file")));
        }
        let len = log.file.metadata()?.len();
        // The old file stays open here, so the new one cannot have its inode.
        if (at_path.dev(), at_path.ino()) == log.id && len >= log.len {
            if len == log.len {
                return Ok(());
            }
            let mut file = &log.file;
            file.seek(SeekFrom::Start(log.len))?;
            let (end, bad) = replay(&mut BufReader::new(file.take(len - log.len)), &mut self.map, log.len)?;
            log.len = end;
            if !bad || zeros(&log.file, end, len)? {
                return Ok(());
            }
        }
        let (new, map) =
            read_log(&log.path).map_err(|e| io::Error::other(e.lines().next().unwrap_or_default().to_string()))?;
        *log = new;
        self.map = map;
        Ok(())
    }

    /// Set `key` to `value`, or delete it for `None`: log first, then change the map. Compacts the
    /// log once it is over twice its compacted size plus `COMPACT_SLACK`.
    fn put(&mut self, key: &str, value: Option<Vec<u8>>, limit: u64) -> io::Result<Result<(), KvErr>> {
        let old = self.map.get(key).map_or(0, |v| (key.len() + v.len()) as u64);
        let rec = match &value {
            Some(v) if v.len() > VALUE_MAX => return Ok(Err(KvErr::TooLarge)),
            Some(v) if self.live - old + (key.len() + v.len()) as u64 > limit => return Ok(Err(KvErr::Full)),
            Some(v) => record(SET, key, v),
            None if !self.map.contains_key(key) => return Ok(Ok(())),
            None => record(DELETE, key, &[]),
        };
        if let Some(log) = &mut self.log {
            if let Err(e) = log.file.write_all_at(&rec, log.len) {
                let _ = log.file.set_len(log.len); // a partial record would corrupt the next append
                return Err(e);
            }
            log.len += rec.len() as u64;
        }
        self.live -= old;
        match value {
            Some(v) => {
                self.live += (key.len() + v.len()) as u64;
                self.map.insert(key.to_string(), v);
            }
            None => {
                self.map.remove(key);
            }
        }
        let compact = MAGIC.len() as u64 + OVERHEAD * self.map.len() as u64 + self.live;
        if let Some(log) = &mut self.log {
            if log.len > 2 * compact + COMPACT_SLACK {
                // Still under the store's lock, and the old file stays locked until the new one,
                // locked, is in place. If this fails the log is still whole: retried next write.
                if let Ok(file) = rewrite(&log.path, &self.map) {
                    log.len = compact;
                    log.file = file;
                }
            }
        }
        Ok(Ok(()))
    }
}

fn check_key(key: &str) -> Result<(), KvErr> {
    if key.is_empty() || key.len() > KEY_MAX {
        return Err(KvErr::TooLarge);
    }
    Ok(())
}

/// Open `path` read-write (creating it, never through a symlink) and take its advisory lock
/// (`File::try_lock`, `flock` on Unix). `None` if another open file holds it. Retries if a
/// compaction renamed a new file into place between the open and the lock, so the lock is always
/// on the file at `path`.
fn lock(path: &Path) -> io::Result<Option<File>> {
    loop {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)?;
        let m = file.metadata()?;
        if !m.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        let id = (m.dev(), m.ino());
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(e)) => return Err(e),
        }
        let now = std::fs::symlink_metadata(path)?;
        if now.file_type().is_symlink() {
            return Err(io::Error::other("a symbolic link"));
        }
        if (now.dev(), now.ino()) == id {
            return Ok(Some(file));
        }
    }
}

/// Open the log at `path` read-only (never through a symlink, never waiting on a FIFO) and replay
/// it: the open log with `len` where its last whole record ends, and the live map. A record cut
/// short or a tail of zeros is left for a later `follow`; the file is never changed.
fn read_log(path: &Path) -> std::result::Result<(Log, BTreeMap<String, Vec<u8>>), String> {
    let shown = path.display();
    let io = |e: io::Error| format!("{shown}: {e}");
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let file = match OpenOptions::new().read(true).custom_flags(flags.bits() as i32).open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!(
                "{shown} does not exist\n  hint: a read-only store is written by another kio: run it once first, or drop `:ro`"
            ))
        }
        r => r.map_err(io)?,
    };
    let m = file.metadata().map_err(io)?;
    if !m.is_file() {
        return Err(format!("{shown}: not a regular file"));
    }
    let mut head = [0u8; 8];
    if file.read_at(&mut head, 0).map_err(io)? < head.len() || &head != MAGIC {
        return Err(format!(
            "{shown} is not a kio store (no `kio-kv1` header)\n  hint: grant a store a kio program writes, or drop `:ro`"
        ));
    }
    let mut map = BTreeMap::new();
    let mut r = &file;
    r.seek(SeekFrom::Start(MAGIC.len() as u64)).map_err(io)?;
    let (end, bad) = replay(
        &mut BufReader::new(r.take(m.len() - MAGIC.len() as u64)),
        &mut map,
        MAGIC.len() as u64,
    )
    .map_err(io)?;
    if bad && !zeros(&file, end, m.len()).map_err(io)? {
        return Err(format!(
            "{shown} is corrupt at byte {end}\n  hint: restore it from a backup, or move it away to start empty"
        ));
    }
    let log = Log {
        file,
        len: end,
        path: path.to_path_buf(),
        id: (m.dev(), m.ino()),
    };
    Ok((log, map))
}

/// Size of the log `rewrite` would write for `map`.
fn compacted(map: &BTreeMap<String, Vec<u8>>) -> u64 {
    let records: u64 = map.iter().map(|(k, v)| OVERHEAD + (k.len() + v.len()) as u64).sum();
    MAGIC.len() as u64 + records
}

/// Write `map` as a compacted log to a new `path.tmp`, locked and synced, and rename it over
/// `path`. The caller still holds the old file's lock, so no other process can take the store in
/// between. Any old `path.tmp` is removed first and the new one is created exclusively, so a
/// symlink planted there is never followed.
fn rewrite(path: &Path, map: &BTreeMap<String, Vec<u8>>) -> io::Result<File> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let file = OpenOptions::new().read(true).write(true).create_new(true).open(&tmp)?;
    let written = (|| {
        file.try_lock().map_err(io::Error::from)?;
        let mut w = BufWriter::new(&file);
        w.write_all(MAGIC)?;
        for (k, v) in map {
            w.write_all(&record(SET, k, v))?;
        }
        w.flush()?;
        drop(w);
        file.sync_all()?; // a rename that outlives the data would lose the whole store
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Make the rename itself durable; best effort, as not every platform syncs a directory.
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(file)
}

/// Apply the records in `r`, which starts at file offset `at`, to `map`, holding at most one
/// record in memory besides the map. Returns the offset where the last whole record ends, and
/// whether what follows is a bad record rather than nothing or a record cut short at the end.
fn replay(r: &mut impl Read, map: &mut BTreeMap<String, Vec<u8>>, mut at: u64) -> io::Result<(u64, bool)> {
    let mut head = [0u8; 8];
    let mut body = Vec::new();
    let u32_of = |b: &[u8]| u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
    loop {
        if fill(r, &mut head)? < head.len() {
            return Ok((at, false));
        }
        let len = u32_of(&head);
        if !(5..=BODY_MAX).contains(&len) {
            return Ok((at, true));
        }
        body.resize(len, 0);
        if fill(r, &mut body)? < len {
            return Ok((at, false));
        }
        let klen = u32_of(&body[1..]);
        if crc32(&body) as usize != u32_of(&head[4..]) || !(1..=KEY_MAX).contains(&klen) || 5 + klen > len {
            return Ok((at, true));
        }
        let Ok(key) = std::str::from_utf8(&body[5..5 + klen]) else {
            return Ok((at, true));
        };
        let value = &body[5 + klen..];
        match body[0] {
            SET if value.len() <= VALUE_MAX => map.insert(key.to_string(), value.to_vec()),
            DELETE if value.is_empty() => map.remove(key),
            _ => return Ok((at, true)),
        };
        at += 8 + len as u64;
    }
}

/// Read into `buf` until it is full or the input ends; how many bytes were read.
fn fill(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Whether every byte of `file` from `from` to `to` is zero.
fn zeros(file: &File, mut from: u64, to: u64) -> io::Result<bool> {
    let mut buf = vec![0u8; 64 << 10];
    while from < to {
        let n = file.read_at(&mut buf[..(to - from).min(64 << 10) as usize], from)?;
        if n == 0 {
            break;
        }
        if buf[..n].iter().any(|&b| b != 0) {
            return Ok(false);
        }
        from += n as u64;
    }
    Ok(true)
}

fn record(tag: u8, key: &str, value: &[u8]) -> Vec<u8> {
    let mut body = vec![tag];
    body.extend_from_slice(&(key.len() as u32).to_le_bytes());
    body.extend_from_slice(key.as_bytes());
    body.extend_from_slice(value);
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&crc32(&body).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// CRC-32 (IEEE) lookup table: entry `i` is the CRC of the byte `i`.
const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CRC-32 (IEEE), a byte at a time.
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = (c >> 8) ^ CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize];
    }
    !c
}

impl crate::kio::caps::kv::Host for State {}

impl HostKv for State {
    fn get(&mut self, h: Resource<Kv>, key: String) -> Result<Result<Option<Vec<u8>>, KvErr>> {
        let arg = key.clone();
        self.call(
            h.rep(),
            "get",
            &arg,
            |s| {
                let r = s.kv(&h, move |st| st.get(&key))?;
                s.spend_io(r.as_ref().map_or(0, |v| v.as_ref().map_or(0, Vec::len)))?;
                Ok(r)
            },
            outcome,
        )
    }

    fn set(&mut self, h: Resource<Kv>, key: String, value: Vec<u8>) -> Result<Result<(), KvErr>> {
        let arg = key.clone();
        self.call(
            h.rep(),
            "set",
            &arg,
            |s| {
                s.writable(&h, "set")?;
                // Charged first, so a call the quota stops changes nothing.
                s.spend_io(value.len())?;
                s.kv(&h, move |st| st.set(&key, value))
            },
            outcome,
        )
    }

    fn delete(&mut self, h: Resource<Kv>, key: String) -> Result<Result<(), KvErr>> {
        let arg = key.clone();
        self.call(
            h.rep(),
            "delete",
            &arg,
            |s| {
                s.writable(&h, "delete")?;
                s.kv(&h, move |st| st.delete(&key))
            },
            outcome,
        )
    }

    fn keys(&mut self, h: Resource<Kv>, prefix: String) -> Result<Result<Vec<String>, KvErr>> {
        let arg = prefix.clone();
        self.call(
            h.rep(),
            "keys",
            &arg,
            |s| {
                let r = s.kv(&h, move |st| st.keys(&prefix))?;
                // A refused listing still scanned up to `KEYS_MAX` keys: charged like `Dir.list`.
                s.spend_io(match &r {
                    Ok(ks) => ks.iter().map(String::len).sum(),
                    Err(KvErr::TooLarge) => KEYS_MAX,
                    Err(_) => 0,
                })?;
                Ok(r)
            },
            outcome,
        )
    }

    fn swap(
        &mut self,
        h: Resource<Kv>,
        key: String,
        old: Option<Vec<u8>>,
        new: Option<Vec<u8>>,
    ) -> Result<Result<bool, KvErr>> {
        let arg = key.clone();
        self.call(
            h.rep(),
            "swap",
            &arg,
            |s| {
                s.writable(&h, "swap")?;
                s.spend_io(old.as_ref().map_or(0, Vec::len) + new.as_ref().map_or(0, Vec::len))?;
                s.kv(&h, move |st| st.swap(&key, old, new))
            },
            |r| match r {
                Ok(false) => "stale".into(),
                r => outcome(r),
            },
        )
    }

    fn drop(&mut self, h: Resource<Kv>) -> Result<()> {
        let _ = self.table.delete(Resource::<Arc<Store>>::new_own(h.rep()));
        Ok(())
    }
}

impl State {
    /// Stop the run if the store behind `h` is read-only: the startup check refuses a program
    /// whose report shows a write on it, so this is defence in depth (a write on a read-only file
    /// would also fail in the OS, as the file is open for reading only).
    fn writable(&mut self, h: &Resource<Kv>, method: &str) -> Result<()> {
        if !self.table.get(&Resource::<Arc<Store>>::new_borrow(h.rep()))?.read_only {
            return Ok(());
        }
        let cap = self.handles.get(&h.rep()).map_or("?", |h| h.0.as_str());
        self.read_only_write = true;
        self.audit_msg = format!("kv: `{cap}` is granted read-only, so `{cap} {method}` stops the run");
        Err(wasmtime::Error::msg(self.audit_msg.clone()))
    }

    /// Run `op` on the store behind `h` within the run's deadline (H5). A full disk is `full`;
    /// any other failed write, or a read-only store that cannot follow its file, stops the run.
    fn kv<T: Send + 'static>(
        &mut self,
        h: &Resource<Kv>,
        op: impl FnOnce(&Store) -> io::Result<Result<T, KvErr>> + Send + 'static,
    ) -> Result<Result<T, KvErr>> {
        let store = self.table.get(&Resource::<Arc<Store>>::new_borrow(h.rep()))?.clone();
        let s = store.clone();
        match self.within(move || op(&s))? {
            Ok(r) => Ok(r),
            Err(e) if e.kind() == io::ErrorKind::StorageFull => Ok(Err(KvErr::Full)),
            // The reader's errors name the file.
            Err(e) if store.read_only => Err(wasmtime::Error::msg(format!("kv: {e}"))),
            Err(e) => Err(wasmtime::Error::msg(format!(
                "kv: cannot write {}: {e}",
                store.target()
            ))),
        }
    }
}

/// H12: a read-only store following a writer in the same process (the writer's lock does not
/// keep a reader out), including hostile changes to the file under it.
#[cfg(test)]
mod tests {
    use super::*;

    fn get(s: &Store, key: &str) -> io::Result<Option<String>> {
        Ok(s.get(key)?.unwrap().map(|v| String::from_utf8(v).unwrap()))
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    #[test]
    fn follows_appends_and_waits_for_a_partial_record() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("s.kv");
        let w = Store::open(&db, 1 << 20).unwrap();
        w.set("a", b"1".to_vec()).unwrap().unwrap();
        let r = Store::open_read_only(&db, 1 << 20).unwrap();
        assert_eq!(get(&r, "a").unwrap().as_deref(), Some("1"));
        w.set("a", b"2".to_vec()).unwrap().unwrap();
        w.set("b", b"3".to_vec()).unwrap().unwrap();
        assert_eq!(get(&r, "a").unwrap().as_deref(), Some("2"));
        assert_eq!(r.keys("").unwrap().unwrap(), ["a", "b"]);
        w.delete("b").unwrap().unwrap();
        assert_eq!(get(&r, "b").unwrap(), None);

        // A record written in two pieces: the first piece is not applied, nor an error.
        let rec = record(SET, "c", b"4");
        append(&db, &rec[..5]);
        assert_eq!(get(&r, "c").unwrap(), None);
        append(&db, &rec[5..rec.len() - 1]);
        assert_eq!(get(&r, "c").unwrap(), None);
        append(&db, &rec[rec.len() - 1..]);
        assert_eq!(get(&r, "c").unwrap().as_deref(), Some("4"));

        // A tail of zeros (a writer's power loss) waits too.
        append(&db, &[0; 16]);
        assert_eq!(get(&r, "c").unwrap().as_deref(), Some("4"));
    }

    #[test]
    fn a_compaction_or_a_shrunk_file_is_replayed() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("s.kv");
        let w = Store::open(&db, 1 << 20).unwrap();
        w.set("a", b"1".to_vec()).unwrap().unwrap();
        w.set("b", b"2".to_vec()).unwrap().unwrap();
        let r = Store::open_read_only(&db, 1 << 20).unwrap();
        assert_eq!(get(&r, "b").unwrap().as_deref(), Some("2"));
        // Rename a compacted copy over the path, as the writer's compaction does.
        let map = BTreeMap::from([("a".to_string(), b"9".to_vec())]);
        drop(rewrite(&db, &map).unwrap());
        assert_eq!(get(&r, "a").unwrap().as_deref(), Some("9"));
        assert_eq!(get(&r, "b").unwrap(), None);

        // Truncated under the reader to just the header: replayed, so the store is empty.
        append(&db, &record(SET, "c", b"3"));
        assert_eq!(get(&r, "c").unwrap().as_deref(), Some("3"));
        OpenOptions::new().write(true).open(&db).unwrap().set_len(8).unwrap();
        assert_eq!(get(&r, "a").unwrap(), None);
        assert_eq!(r.keys("").unwrap().unwrap(), Vec::<String>::new());

        // Rewritten in place, longer than before: the reader's offset lands inside a record,
        // which replays the whole file rather than failing or misreading.
        append(&db, &record(SET, "d", b"4"));
        assert_eq!(get(&r, "d").unwrap().as_deref(), Some("4"));
        OpenOptions::new().write(true).open(&db).unwrap().set_len(8).unwrap();
        append(&db, &record(SET, "e", b"a much longer value than before"));
        assert_eq!(
            get(&r, "e").unwrap().as_deref(),
            Some("a much longer value than before")
        );
        assert_eq!(get(&r, "d").unwrap(), None);
    }

    #[test]
    fn garbage_stops_reads_until_the_file_is_whole_again() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("s.kv");
        let w = Store::open(&db, 1 << 20).unwrap();
        w.set("a", b"1".to_vec()).unwrap().unwrap();
        let r = Store::open_read_only(&db, 1 << 20).unwrap();
        let mut bad = record(SET, "a", b"2");
        let n = bad.len();
        bad[n - 1] ^= 1; // the checksum no longer matches
        append(&db, &bad);
        let e = get(&r, "a").unwrap_err().to_string();
        assert_eq!(e, format!("{} is corrupt at byte 23", db.display()));
        // A huge length claims nothing beyond the record limit.
        let mut huge = (u32::MAX).to_le_bytes().to_vec();
        huge.extend_from_slice(&[1; 12]);
        append(&db, &huge);
        assert!(get(&r, "a").is_err());
        // Still stopped at the same place; a writer's compaction makes it whole again.
        let map = BTreeMap::from([("a".to_string(), b"3".to_vec())]);
        drop(rewrite(&db, &map).unwrap());
        assert_eq!(get(&r, "a").unwrap().as_deref(), Some("3"));
    }

    #[test]
    fn a_symlink_or_another_file_swapped_in() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("s.kv");
        let w = Store::open(&db, 1 << 20).unwrap();
        w.set("a", b"1".to_vec()).unwrap().unwrap();
        let r = Store::open_read_only(&db, 1 << 20).unwrap();
        assert_eq!(get(&r, "a").unwrap().as_deref(), Some("1"));
        let other = t.path().join("other.kv");
        std::fs::write(&other, MAGIC).unwrap();
        append(&other, &record(SET, "a", b"evil"));
        std::fs::rename(&db, t.path().join("moved.kv")).unwrap();
        std::os::unix::fs::symlink(&other, &db).unwrap();
        let e = get(&r, "a").unwrap_err().to_string();
        assert_eq!(e, format!("{} is a symbolic link", db.display()));
        // A file that is not a store, renamed in: an error, never a partial view.
        std::fs::remove_file(&db).unwrap();
        std::fs::write(&db, "name,amount\n").unwrap();
        assert!(get(&r, "a").unwrap_err().to_string().contains("is not a kio store"));
        // A FIFO is refused without opening it (which could wait for a writer).
        std::fs::remove_file(&db).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(&db)
            .status()
            .unwrap()
            .success());
        assert!(get(&r, "a").unwrap_err().to_string().ends_with("not a regular file"));
        // Removed: the reader keeps the file it has open.
        std::fs::remove_file(&db).unwrap();
        assert_eq!(get(&r, "a").unwrap().as_deref(), Some("1"));
    }

    /// A writer appending and compacting online while a reader polls: the reader never errors,
    /// never sees a value go backwards, and ends at the last one.
    #[test]
    fn a_reader_races_a_compacting_writer() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("s.kv");
        let w = Store::open(&db, 64 << 20).unwrap();
        w.set("n", b"0".to_vec()).unwrap().unwrap();
        let r = Store::open_read_only(&db, 64 << 20).unwrap();
        let n = |s: &str| s.split(' ').next().unwrap().parse::<u32>().unwrap();
        let reader = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let (mut last, mut reads) = (0, 0);
                while last < 1500 {
                    let now = n(&get(&r, "n").unwrap().unwrap());
                    assert!(now >= last, "{now} after {last}");
                    (last, reads) = (now, reads + 1);
                }
                reads
            });
            let pad = " ".repeat(8 << 10);
            for i in 1..=1500 {
                w.set("n", format!("{i}{pad}").into_bytes()).unwrap().unwrap();
            }
            reader.join().unwrap()
        });
        assert!(reader > 1);
        // 12 MB written, compacted online every 1 MB or so.
        assert!(std::fs::metadata(&db).unwrap().len() < 4 << 20);
    }
}
