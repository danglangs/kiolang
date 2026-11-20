//! Engine settings, shared with host/build.rs (`#[path]`): the runtime's native code is compiled
//! there with this configuration and embedded in the binary (runtime_cache.rs, C18).

/// The engine settings every run uses: fuel and epoch interruption bound a run's work and time.
pub fn compile_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    config
}

/// What decides an engine's native code: Wasmtime's own compatibility hash of its target, CPU
/// features, compiler flags and tunables (what it checks before using precompiled code), as one
/// number.
/// host/build.rs records it beside the runtime's native code; `runtime_cache::engine` serves that
/// code only to an engine with the same number.
pub fn fingerprint(engine: &wasmtime::Engine) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut h);
    h.finish()
}

/// The largest `--stack`, in bytes.
pub const MAX_STACK: usize = 8 << 20;

/// The native stack of a thread that checks or runs a program. Compiled code runs on the
/// calling thread's stack, up to `--stack`, with host functions below it; on a smaller thread a
/// deep program would overflow the host's stack (an abort) before Wasmtime's limit (a trap).
pub const RUN_STACK: usize = 4 * MAX_STACK;
