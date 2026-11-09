/// The engine settings every run uses: fuel and epoch interruption bound a run's work and time.
pub fn compile_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    config
}

/// The largest `--stack`, in bytes.
pub const MAX_STACK: usize = 8 << 20;

/// The native stack of a thread that checks or runs a program. Compiled code runs on the
/// calling thread's stack, up to `--stack`, with host functions below it; on a smaller thread a
/// deep program would overflow the host's stack (an abort) before Wasmtime's limit (a trap).
pub const RUN_STACK: usize = 4 * MAX_STACK;
