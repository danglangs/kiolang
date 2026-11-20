//! Engine settings, shared with host/build.rs (`#[path]`): the runtime's native code is compiled
//! there with this configuration and embedded in the binary (runtime_cache.rs, C18).

/// The engine settings every run uses: fuel and epoch interruption bound a run's work and time.
/// The ISA is `baseline()` when the host supports it, so the native code the build embedded
/// fits every machine the release targets run on; a host that does not gets Cranelift's
/// inferred features and compiles from source.
pub fn compile_config() -> wasmtime::Config {
    compile_config_with(cfg!(feature = "cache-checks"))
}

/// `compile_config`, with Cranelift's incremental-cache checks on when `checks` (the host's
/// `cache-checks` feature; host/build.rs reads `CARGO_FEATURE_CACHE_CHECKS`): every cache hit is
/// also compiled and must equal the cached code, or Cranelift panics.
pub fn compile_config_with(checks: bool) -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    if let Some((triple, flags)) = baseline() {
        config.target(triple).expect("the host's own triple");
        for flag in flags {
            // SAFETY (Wasmtime marks the method unsafe because code built for a feature the host
            // lacks would fault): `baseline` returns a flag only after the host reported the
            // feature.
            unsafe { config.cranelift_flag_enable(flag) };
        }
    }
    if checks {
        // SAFETY: the setting adds checks and changes no generated code (Wasmtime leaves it out
        // of its compatibility hash for that reason).
        unsafe { config.cranelift_flag_enable("enable_incremental_compilation_cache_checks") };
    }
    config
}

/// The ISA baseline of the release targets, if this host runs one and has its features: the
/// triple to compile for (an explicit target turns Cranelift's host feature inference off) and
/// the Cranelift flags to enable. aarch64 is Armv8.0 as Cranelift assumes it (no LSE, no
/// pointer authentication: compiled kio has no atomics). x86-64 is level v2: SSE3 to SSE4.2 and
/// POPCNT, which every x86-64 CPU since about 2009 has, and which Cranelift wants for `f64`
/// rounding (`roundsd`) and bit counts without a library call. Anything else is `None`: the
/// engine infers the host's features and the embedded code is not used.
pub fn baseline() -> Option<(&'static str, &'static [&'static str])> {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    return Some(("aarch64-apple-darwin", &[]));
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    return Some(("aarch64-unknown-linux-gnu", &[]));
    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    {
        let supported = std::arch::is_x86_feature_detected!("sse3")
            && std::arch::is_x86_feature_detected!("ssse3")
            && std::arch::is_x86_feature_detected!("sse4.1")
            && std::arch::is_x86_feature_detected!("sse4.2")
            && std::arch::is_x86_feature_detected!("popcnt");
        let triple = match (cfg!(target_os = "macos"), cfg!(target_env = "musl")) {
            (true, _) => "x86_64-apple-darwin",
            (false, true) => "x86_64-unknown-linux-musl",
            (false, false) => "x86_64-unknown-linux-gnu",
        };
        return supported.then_some((
            triple,
            &["has_sse3", "has_ssse3", "has_sse41", "has_sse42", "has_popcnt"],
        ));
    }
    #[allow(unreachable_code)]
    None
}

/// What decides an engine's native code: Wasmtime's own compatibility hash of its target, CPU
/// features, compiler flags and tunables (what it checks before using precompiled code), as one
/// number. host/build.rs records it beside the runtime's native code; `runtime_cache::engine`
/// serves that code only to an engine with the same number.
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
