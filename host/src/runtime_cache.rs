//! The runtime's native code, compiled once when `kio` is built and embedded in the binary
//! (C18, speed-plan fix 3). host/build.rs compiles runtime.wasm with `config::compile_config()`
//! and records Cranelift's incremental cache: one entry per function, keyed by a SHA-256 of the
//! function's IR and the ISA, holding its native code before linking. A run then compiles the
//! program's component as before (`Component::new`), and Cranelift takes every runtime function
//! from the cache, compiling only the program's own functions.
//!
//! What this trusts, and how each requirement of C18 holds:
//! - Only bytes the build produced reach Cranelift: `SEED` is `include_bytes!` of the build
//!   script's output. No file, environment variable, cache directory or other source is read
//!   (host/tests/cache.rs checks this file's text), and nothing is ever written to the seed:
//!   `insert` never touches it. A `Shared` engine (T5, `kio test` and `kio review`) keeps what
//!   it compiles in a memory layer behind the seed, in this process only, as T5 did.
//!   open_questions #75 stands: no native code enters from outside the binary, and no program
//!   can plant any.
//! - The engine is fixed at build time and checked at load: the seed carries the build engine's
//!   `config::fingerprint` (Wasmtime's own compatibility hash of target, CPU features, compiler
//!   flags and tunables; `config::baseline` pins the ISA so it is the same on every machine of a
//!   release target that has the baseline's features). `engine` serves the seed only to an
//!   engine whose fingerprint is equal; any other engine compiles from source. Cranelift also
//!   refuses an entry from another Cranelift version (its version marker) and compiles the
//!   function instead. Both the seed and the binary come from one Cargo.lock, so one Wasmtime
//!   version.
//! - Program data patches no native bytes: a cached entry is a function compiled from an IR
//!   whose SHA-256 is the key, resolved against the program's module exactly as a function
//!   compiled this run would be (Cranelift's `apply_params`); the program supplies Wasm, and the
//!   host compiles whatever is not in the cache. A program function whose IR equals a runtime
//!   function's gets that IR's code, which compiling would also produce. With the host's
//!   `cache-checks` feature Cranelift compiles every hit as well and asserts the two equal.
//! - The module is one core module as before: memory, handles, fuel, epochs, traps and the
//!   instance per run are untouched (docs/codegen.md, "Pipeline"). A miss costs only the time
//!   to compile that function.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use wasmtime::{CacheStore, Config, Engine, Result};

/// host/build.rs's output; its format is described there.
const SEED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runtime.cache"));

/// An entry's fixed part: a 32-byte key and a 4-byte length.
const ENTRY_HEAD: usize = 36;

/// The seed, parsed: the build engine's fingerprint and the entries, sorted by key.
struct Seed {
    fingerprint: u64,
    entries: Vec<(&'static [u8], &'static [u8])>,
}

/// Parses `SEED` once. A malformed seed (never, from the build script) is no seed.
fn seed() -> Option<&'static Seed> {
    static PARSED: OnceLock<Option<Seed>> = OnceLock::new();
    PARSED.get_or_init(|| parse(SEED)).as_ref()
}

fn parse(bytes: &'static [u8]) -> Option<Seed> {
    let rest = bytes.strip_prefix(b"kio-rtc1")?;
    let (fingerprint, rest) = rest.split_first_chunk::<8>()?;
    let (count, mut rest) = rest.split_first_chunk::<4>()?;
    let count = u32::from_le_bytes(*count) as usize;
    // Each entry takes at least its head, so a count the bytes cannot hold is refused before
    // anything is allocated for it.
    if count > rest.len() / ENTRY_HEAD {
        return None;
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let (key, r) = rest.split_at_checked(32)?;
        let (len, r) = r.split_first_chunk::<4>()?;
        let (code, r) = r.split_at_checked(u32::from_le_bytes(*len) as usize)?;
        if entries.last().is_some_and(|(k, _)| *k >= key) {
            return None;
        }
        entries.push((key, code));
        rest = r;
    }
    rest.is_empty().then_some(Seed {
        fingerprint: u64::from_le_bytes(*fingerprint),
        entries,
    })
}

static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// The lookups so far, `(hits, misses)`, over every engine of this process (`--json`'s
/// `runtime_cache`, `kio test --verbose`); whether an engine is served is the engine's own
/// (`Kio::runtime_cache_served`).
pub fn stats() -> (u64, u64) {
    (HITS.load(Relaxed), MISSES.load(Relaxed))
}

/// The most bytes of compiled functions the memory layer keeps (T5).
const MEMORY_MAX: usize = 32 << 20;

/// Cranelift's cache: the embedded seed, read-only and answering nothing until `armed`, and
/// behind it, for a `Shared` engine (T5), what this process compiled, by key, as Cranelift
/// serialized it. Only the memory layer takes inserts, and only of what the seed lacks.
struct Store {
    seed: &'static Seed,
    armed: AtomicBool,
    memory: Option<Mutex<Memory>>,
}

#[derive(Default)]
struct Memory {
    by_key: HashMap<Vec<u8>, Arc<[u8]>>,
    bytes: usize,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "the runtime's embedded native code ({} functions){}",
            self.seed.entries.len(),
            if self.memory.is_some() {
                ", and this process's compiles"
            } else {
                ""
            }
        )
    }
}

impl CacheStore for Store {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        if self.armed.load(Relaxed) {
            if let Ok(i) = self.seed.entries.binary_search_by(|(k, _)| (*k).cmp(key)) {
                HITS.fetch_add(1, Relaxed);
                return Some(Cow::Borrowed(self.seed.entries[i].1));
            }
        }
        // `get` may borrow only from `self`, and an entry can go when the memory empties, so the
        // bytes are copied; outside the lock, which holds only while the `Arc` is cloned.
        let kept = self.memory.as_ref().and_then(|m| {
            m.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .by_key
                .get(key)
                .cloned()
        });
        match kept {
            Some(code) => Some(Cow::Owned(code.to_vec())),
            None => {
                MISSES.fetch_add(1, Relaxed);
                None
            }
        }
    }

    /// The seed never grows. A `Shared` engine keeps what it compiles in memory, up to
    /// `MEMORY_MAX`; full, it starts over (most entries are cases' own functions, never met
    /// again, and the functions every run shares come back on the next compile).
    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        let Some(memory) = &self.memory else {
            return false;
        };
        let bytes = key.len() + value.len();
        let mut memory = memory.lock().unwrap_or_else(PoisonError::into_inner);
        if bytes > MEMORY_MAX || memory.by_key.contains_key(key) {
            return false;
        }
        if memory.bytes + bytes > MEMORY_MAX {
            *memory = Memory::default();
        }
        memory.bytes += bytes;
        memory.by_key.insert(key.to_vec(), value.into());
        true
    }
}

/// An engine for `config`, serving the runtime's embedded native code when `config` builds the
/// engine the seed was made for (same fingerprint), and compiling everything from source
/// otherwise. The second value says which. With `keep`, the engine also keeps what it compiles
/// in memory (a `Shared` engine, T5), whether or not the seed fits it. The store is attached
/// before the engine exists and the seed armed only once the engine's fingerprint is known to
/// match; nothing compiles in between, and an unarmed seed answers no lookup. An engine the seed
/// does not fit and that keeps nothing is made again without the store, so Cranelift does not
/// serialize what it compiles for it.
pub fn engine(mut config: Config, keep: bool) -> Result<(Engine, bool)> {
    let plain = config.clone();
    let Some(seed) = seed() else {
        return Ok((Engine::new(&plain)?, false));
    };
    let store = Arc::new(Store {
        seed,
        armed: AtomicBool::new(false),
        memory: keep.then(Mutex::default),
    });
    config.enable_incremental_compilation(store.clone())?;
    let engine = Engine::new(&config)?;
    if crate::config::fingerprint(&engine) != seed.fingerprint {
        return Ok(if keep {
            (engine, false)
        } else {
            (Engine::new(&plain)?, false)
        });
    }
    store.armed.store(true, Relaxed);
    Ok((engine, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_parses_and_holds_the_runtime() {
        let seed = seed().expect("the build script wrote a well-formed seed");
        assert!(seed.entries.len() > 500, "{} entries", seed.entries.len());
        assert!(seed.entries.windows(2).all(|w| w[0].0 < w[1].0));
    }

    /// The seed was compiled for this binary's target (host/build.rs, cargo's `TARGET`), with the
    /// baseline chosen from the target's name alone, so the build script and the binary agree on
    /// it however the build machine differs from the target.
    #[test]
    fn the_seed_is_for_this_binary_target() {
        let target = env!("KIO_TARGET");
        let checks = cfg!(feature = "cache-checks");
        let for_target = Engine::new(&crate::config::compile_config_for(target, checks)).unwrap();
        assert_eq!(crate::config::fingerprint(&for_target), seed().unwrap().fingerprint);
        for (name, flags) in [
            ("x86_64-unknown-linux-musl", 5),
            ("x86_64-unknown-linux-gnu", 5),
            ("x86_64-apple-darwin", 5),
            ("aarch64-apple-darwin", 0),
            ("aarch64-unknown-linux-gnu", 0),
        ] {
            let (triple, baseline) = crate::config::baseline(name).unwrap();
            assert_eq!((triple, baseline.len()), (name, flags));
        }
        assert!(crate::config::baseline("x86_64-pc-windows-msvc").is_none());
        assert!(crate::config::baseline("").is_none());
    }

    #[test]
    fn the_default_engine_is_served_and_another_is_not() {
        let (_, served) = engine(crate::config::compile_config(), false).unwrap();
        assert!(served);
        let mut other = crate::config::compile_config();
        other.consume_fuel(false);
        let (_, served) = engine(other, false).unwrap();
        assert!(!served, "an engine with other settings compiles from source");
    }

    /// The seed answers first and never changes; the memory layer, when there is one, keeps what
    /// the seed lacks; without one, nothing is kept.
    #[test]
    fn the_memory_layer_sits_behind_the_seed() {
        let seed = seed().unwrap();
        let (seeded_key, seeded_code) = seed.entries[0];
        let unknown = [7u8; 32];
        for keep in [false, true] {
            let store = Store {
                seed,
                armed: AtomicBool::new(true),
                memory: keep.then(Mutex::default),
            };
            assert_eq!(store.get(seeded_key).as_deref(), Some(seeded_code));
            assert!(store.get(&unknown).is_none());
            assert_eq!(store.insert(&unknown, vec![1, 2, 3]), keep);
            assert_eq!(store.get(&unknown).as_deref(), keep.then_some(&[1u8, 2, 3][..]));
            assert_eq!(store.get(seeded_key).as_deref(), Some(seeded_code));
            // Unarmed (the fingerprint did not match), the seed answers nothing; memory still does.
            store.armed.store(false, Relaxed);
            assert!(store.get(seeded_key).is_none());
            assert_eq!(store.get(&unknown).as_deref(), keep.then_some(&[1u8, 2, 3][..]));
        }
    }

    #[test]
    fn a_truncated_unsorted_or_overcounted_seed_is_no_seed() {
        assert!(parse(&SEED[..SEED.len() - 1]).is_none());
        // The first key made the largest possible: no longer sorted, so refused.
        let mut edited = SEED.to_vec();
        let first_key = 8 + 8 + 4;
        edited[first_key..first_key + 32].fill(0xff);
        assert!(parse(Box::leak(edited.into_boxed_slice())).is_none());
        // A count the bytes cannot hold is refused before any entry is read.
        let mut counted = SEED.to_vec();
        counted[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(Box::leak(counted.into_boxed_slice())).is_none());
    }
}
