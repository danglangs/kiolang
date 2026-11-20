//! The runtime's native code, compiled once when `kio` is built and embedded in the binary
//! (C18, speed-plan fix 3). host/build.rs compiles runtime.wasm with `config::compile_config()`
//! and records Cranelift's incremental cache: one entry per function, keyed by a SHA-256 of the
//! function's IR and the ISA, holding its native code before linking. A run then compiles the
//! program's component as before (`Component::new`), and Cranelift takes every runtime function
//! from the cache, compiling only the program's own functions.
//!
//! What this trusts, and how each requirement of C18 holds:
//! - Only bytes the build produced reach Cranelift: `SEED` is `include_bytes!` of the build
//!   script's output. No file, environment variable, cache directory or other source is read,
//!   and nothing is ever written: `insert` keeps nothing and returns false, so a run's compiled
//!   functions never enter the cache. open_questions #75 stands: no native code is loaded from
//!   disk, and no program can plant any.
//! - The engine is fixed at build time and checked at load: the seed carries the build engine's
//!   `config::fingerprint` (Wasmtime's own compatibility hash of target, CPU features, compiler
//!   flags and tunables). `engine` serves the seed only to an engine whose fingerprint is equal;
//!   any other engine compiles from source. Cranelift also refuses an entry from another
//!   Cranelift version (its version marker) and compiles the function instead. Both the seed
//!   and the binary come from one Cargo.lock, so one Wasmtime version.
//! - Program data patches no native bytes: a cached entry is a function compiled from an IR
//!   whose SHA-256 is the key, resolved against the program's module exactly as a function
//!   compiled this run would be (Cranelift's `apply_params`); the program supplies Wasm, and the
//!   host compiles whatever is not in the cache. A program function whose IR equals a runtime
//!   function's gets that IR's code, which compiling would also produce.
//! - The module is one core module as before: memory, handles, fuel, epochs, traps and the
//!   instance per run are untouched (docs/codegen.md, "Pipeline"). A miss costs only the time
//!   to compile that function.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};
use wasmtime::{CacheStore, Config, Engine, Result};

/// host/build.rs's output; its format is described there.
const SEED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runtime.cache"));

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

/// Cache lookups so far: `(hits, misses)`, over every engine of this process (a test hook).
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
pub fn stats() -> (u64, u64) {
    (HITS.load(Relaxed), MISSES.load(Relaxed))
}

/// The embedded seed as Cranelift's cache: read-only.
struct Embedded(&'static Seed);

impl std::fmt::Debug for Embedded {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "the runtime's embedded native code ({} functions)",
            self.0.entries.len()
        )
    }
}

impl CacheStore for Embedded {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let found = self.0.entries.binary_search_by(|(k, _)| (*k).cmp(key)).ok();
        match found {
            Some(i) => {
                HITS.fetch_add(1, Relaxed);
                Some(Cow::Borrowed(self.0.entries[i].1))
            }
            None => {
                MISSES.fetch_add(1, Relaxed);
                None
            }
        }
    }

    /// Nothing a run compiles is kept: the cache is the build's and never grows.
    fn insert(&self, _key: &[u8], _value: Vec<u8>) -> bool {
        false
    }
}

/// An engine for `config`, serving the runtime's embedded native code when `config` builds the
/// engine the seed was made for (same fingerprint), and compiling everything from source
/// otherwise. The second value says which.
pub fn engine(mut config: Config) -> Result<(Engine, bool)> {
    let engine = Engine::new(&config)?;
    let Some(seed) = seed() else {
        return Ok((engine, false));
    };
    if crate::config::fingerprint(&engine) != seed.fingerprint {
        return Ok((engine, false));
    }
    config.enable_incremental_compilation(Arc::new(Embedded(seed)))?;
    Ok((Engine::new(&config)?, true))
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

    #[test]
    fn the_default_engine_is_served_and_another_is_not() {
        let (_, served) = engine(crate::config::compile_config()).unwrap();
        assert!(served);
        let mut other = crate::config::compile_config();
        other.consume_fuel(false);
        let (_, served) = engine(other).unwrap();
        assert!(!served, "an engine with other settings compiles from source");
    }

    #[test]
    fn a_truncated_or_unsorted_seed_is_no_seed() {
        assert!(parse(&SEED[..SEED.len() - 1]).is_none());
        // The first key made the largest possible: no longer sorted, so refused.
        let mut edited = SEED.to_vec();
        let first_key = 8 + 8 + 4;
        edited[first_key..first_key + 32].fill(0xff);
        assert!(parse(Box::leak(edited.into_boxed_slice())).is_none());
    }
}
