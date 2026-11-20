//! KIO_COMMIT: the short commit `kio --version` names, or "unknown" outside a git checkout.
//! runtime.cache: the runtime's functions compiled to native code with the engine settings of
//! src/config.rs, as Cranelift's incremental cache (C18). The binary embeds it
//! (src/runtime_cache.rs), so a run compiles only the program's own functions.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::{env, fs, path::PathBuf};

#[path = "src/config.rs"]
#[allow(dead_code)]
mod config;

fn main() {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=KIO_COMMIT={commit}");
    // A new commit moves HEAD or the branch it names.
    for p in ["../.git/HEAD", "../.git/refs/heads", "../.git/packed-refs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    seed_runtime_cache();
}

/// Records what Cranelift compiles.
#[derive(Debug, Default)]
struct Recorder(Mutex<BTreeMap<Vec<u8>, Vec<u8>>>);

impl wasmtime::CacheStore for Recorder {
    fn get(&self, key: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
        self.0.lock().unwrap().get(key).cloned().map(Into::into)
    }
    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        self.0.lock().unwrap().insert(key.to_vec(), value);
        true
    }
}

/// Compiles runtime.wasm (codegen's build, `DEP_KIO_RUNTIME_RUNTIME_WASM`) wrapped as a
/// component, with `config::compile_config_for` the binary's target, recording every function's native code under
/// Cranelift's key for it (a SHA-256 of the function's IR and the ISA), and writes
/// `OUT_DIR/runtime.cache`: `kio-rtc1`, the engine's `config::fingerprint` (u64), the entry
/// count (u32), then the entries sorted by key (32-byte key, u32 length, bytes); integers
/// little-endian. The build dependency has no `parallel-compilation` feature, so one thread
/// compiles, in module order, and the file is the same from one build to the next.
fn seed_runtime_cache() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/config.rs");
    println!("cargo:rerun-if-env-changed=KIO_TEST_HEAP");
    let runtime = env::var("DEP_KIO_RUNTIME_RUNTIME_WASM").expect("codegen/build.rs names runtime.wasm");
    println!("cargo:rerun-if-changed={runtime}");
    let wasm = fs::read(&runtime).expect("runtime.wasm");
    let component = wit_component::ComponentEncoder::default()
        .module(&wasm)
        .expect("runtime.wasm is a core module")
        .validate(true)
        .encode()
        .expect("wrapping the runtime as a component");
    let recorder = std::sync::Arc::new(Recorder::default());
    // The binary's target, not the build machine's: `x86_64-unknown-linux-musl` is built on a gnu
    // runner. The binary reads it back as `KIO_TARGET` (config.rs) so both pick the same baseline.
    let target = env::var("TARGET").expect("cargo sets TARGET");
    println!("cargo:rustc-env=KIO_TARGET={target}");
    let checks = env::var_os("CARGO_FEATURE_CACHE_CHECKS").is_some();
    let mut config = config::compile_config_for(&target, checks);
    config
        .enable_incremental_compilation(recorder.clone())
        .expect("Cranelift's incremental cache");
    let engine = wasmtime::Engine::new(&config).expect("engine");
    engine.precompile_component(&component).expect("compiling the runtime");
    let entries = recorder.0.lock().unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(b"kio-rtc1");
    out.extend_from_slice(&config::fingerprint(&engine).to_le_bytes());
    out.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
    for (key, code) in entries.iter() {
        assert_eq!(key.len(), 32, "Cranelift's cache key is a SHA-256");
        out.extend_from_slice(key);
        out.extend_from_slice(&u32::try_from(code.len()).unwrap().to_le_bytes());
        out.extend_from_slice(code);
    }
    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("runtime.cache");
    fs::write(path, out).unwrap();
}
