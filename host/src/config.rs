//! Engine settings, shared with host/build.rs (`#[path]`): the runtime's native code is compiled
//! there with this configuration and embedded in the binary (runtime_cache.rs, C18).

/// The engine settings every run uses: fuel and epoch interruption bound a run's work and time.
/// The ISA is the `baseline` of this binary's own target when the host has its features, so the
/// native code the build embedded fits every machine the release targets run on; a host that
/// lacks them gets Cranelift's inferred features and compiles from source.
pub fn compile_config() -> wasmtime::Config {
    compile_config_with(cfg!(feature = "cache-checks"))
}

/// `compile_config`, with Cranelift's incremental-cache checks on when `checks` (the host's
/// `cache-checks` feature): every cache hit is also compiled and must equal the cached code, or
/// Cranelift panics. The target is the one this binary was compiled for (`KIO_TARGET`, set by
/// host/build.rs from cargo's `TARGET`), never the machine it runs on.
pub fn compile_config_with(checks: bool) -> wasmtime::Config {
    let pin = baseline(option_env!("KIO_TARGET").unwrap_or("")).filter(|(_, flags)| host_has(flags));
    configured(pin, checks)
}

/// The settings host/build.rs compiles the seed with: `target`'s baseline, pinned whatever the
/// build machine has (the seed is for the machines the binary will run on, and a build host
/// lacking a feature must not produce an unpinned seed), with checks as `compile_config_with`
/// (the build script reads `CARGO_FEATURE_CACHE_CHECKS`).
#[cfg_attr(not(test), allow(dead_code))] // the build script's and the tests'
pub fn compile_config_for(target: &str, checks: bool) -> wasmtime::Config {
    configured(baseline(target), checks)
}

fn configured(pin: Option<(&'static str, &'static [&'static str])>, checks: bool) -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    if let Some((triple, flags)) = pin {
        config.target(triple).expect("a release target's triple");
        for flag in flags {
            // SAFETY (Wasmtime marks the method unsafe because code built for a feature the host
            // lacks would fault): the binary enables a flag only after `host_has` reported the
            // feature; the build script's engine only compiles, never runs, what it builds.
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

/// x86-64 level v2: SSE3 to SSE4.2 and POPCNT, which every x86-64 CPU since about 2009 has, and
/// which Cranelift wants for `f64` rounding (`roundsd`) and bit counts without a library call.
const X86_64_V2: &[&str] = &["has_sse3", "has_ssse3", "has_sse41", "has_sse42", "has_popcnt"];

/// The ISA baseline for `target`, one of the release targets' families: the triple to compile
/// for (an explicit target turns Cranelift's host feature inference off) and the Cranelift flags
/// to enable. aarch64 is Armv8.0 as Cranelift assumes it (no LSE, no pointer authentication:
/// compiled kio has no atomics); x86-64 is `X86_64_V2`. Any other target is `None`: the engine
/// infers the host's features and the embedded code is not used. The triple is chosen from the
/// target's name alone, so host/build.rs (a host program) and the binary it builds agree on it
/// even when one is built for another machine (`x86_64-unknown-linux-musl` on a gnu runner).
pub fn baseline(target: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match target {
        "aarch64-apple-darwin" => ("aarch64-apple-darwin", &[]),
        "aarch64-unknown-linux-gnu" => ("aarch64-unknown-linux-gnu", &[]),
        "aarch64-unknown-linux-musl" => ("aarch64-unknown-linux-musl", &[]),
        "x86_64-apple-darwin" => ("x86_64-apple-darwin", X86_64_V2),
        "x86_64-unknown-linux-gnu" => ("x86_64-unknown-linux-gnu", X86_64_V2),
        "x86_64-unknown-linux-musl" => ("x86_64-unknown-linux-musl", X86_64_V2),
        _ => return None,
    })
}

/// Whether this machine has every feature in `flags` (the baseline's; `X86_64_V2` or none).
fn host_has(flags: &[&str]) -> bool {
    flags.iter().all(|flag| match *flag {
        #[cfg(target_arch = "x86_64")]
        "has_sse3" => std::arch::is_x86_feature_detected!("sse3"),
        #[cfg(target_arch = "x86_64")]
        "has_ssse3" => std::arch::is_x86_feature_detected!("ssse3"),
        #[cfg(target_arch = "x86_64")]
        "has_sse41" => std::arch::is_x86_feature_detected!("sse4.1"),
        #[cfg(target_arch = "x86_64")]
        "has_sse42" => std::arch::is_x86_feature_detected!("sse4.2"),
        #[cfg(target_arch = "x86_64")]
        "has_popcnt" => std::arch::is_x86_feature_detected!("popcnt"),
        _ => false,
    })
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
