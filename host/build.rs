use std::{env, path::PathBuf, process::Command};

#[path = "src/config.rs"]
mod config;

fn main() {
    for p in ["../guest", "../lang/src", "../caps.wit", "src/config.rs", "build.rs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let cargo = env::var("CARGO").unwrap();
    let ok = Command::new(cargo)
        .current_dir(&root)
        .args(["build", "-p", "guest", "--target", "wasm32-wasip2", "--release"])
        .args(["--target-dir", "target/guest"])
        // The guest's own Rust stack lives in its linear memory and is fixed here, at link time.
        // Make it at least `MAX_STACK`, so `--stack` (Wasmtime's stack) is always the limit that
        // trips first and a deep program stops with "stack overflow", never a memory fault.
        // (The outer cargo sets `CARGO_ENCODED_RUSTFLAGS` for this script, which would override a
        // target-specific variable, so set it directly. Its separator is 0x1f.)
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            format!("-C\x1flink-arg=-zstack-size={}", config::MAX_STACK),
        )
        .status()
        .unwrap()
        .success();
    assert!(ok, "building the guest failed");
    // Compile the guest to native code once here, not on every run.
    let wasm = std::fs::read(root.join("target/guest/wasm32-wasip2/release/guest.wasm")).unwrap();
    let engine = wasmtime::Engine::new(&config::compile_config()).unwrap();
    let native = engine.precompile_component(&wasm).unwrap();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("guest.cwasm"), native).unwrap();
    // `Server` pre-initializes the guest for its program, which needs the Wasm itself.
    std::fs::write(out.join("guest.wasm"), wasm).unwrap();
}
