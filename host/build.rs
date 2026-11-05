use std::{env, path::PathBuf, process::Command};

#[path = "src/config.rs"]
mod config;

fn main() {
    for p in ["../guest", "../lang/src", "../caps.wit", "src/config.rs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let cargo = env::var("CARGO").unwrap();
    let ok = Command::new(cargo)
        .current_dir(&root)
        .args(["build", "-p", "guest", "--target", "wasm32-wasip2", "--release"])
        .args(["--target-dir", "target/guest"])
        .status()
        .unwrap()
        .success();
    assert!(ok, "building the guest failed");
    // Compile the guest to native code once here, not on every run.
    let wasm = std::fs::read(root.join("target/guest/wasm32-wasip2/release/guest.wasm")).unwrap();
    let engine = wasmtime::Engine::new(&config::compile_config()).unwrap();
    let native = engine.precompile_component(&wasm).unwrap();
    std::fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("guest.cwasm"), native).unwrap();
}
