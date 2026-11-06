/// The engine settings that affect compiled code. `build.rs` precompiles the guest with them and
/// the runtime loads it with them; Wasmtime refuses the precompiled guest if they differ.
pub fn compile_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    config
}

/// The largest `--stack`, in bytes; the guest's in-memory stack is linked at this size.
pub const MAX_STACK: usize = 8 << 20;
