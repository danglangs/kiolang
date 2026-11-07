use std::{env, path::PathBuf, process::Command};

/// Builds runtime.wasm, the complete core module codegen appends each program to.
fn main() {
    for p in ["../runtime", "../caps.wit", "build.rs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let ok = Command::new(env::var("CARGO").unwrap())
        .current_dir(&root)
        .args([
            "build",
            "-p",
            "runtime",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .args(["--target-dir", "target/runtime"])
        // The outer cargo's flags are for the host, not the runtime.
        .env("CARGO_ENCODED_RUSTFLAGS", "")
        .status()
        .unwrap()
        .success();
    assert!(ok, "building the runtime failed");
    let wasm = root.join("target/runtime/wasm32-unknown-unknown/release/runtime.wasm");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::copy(wasm, out.join("runtime.wasm")).unwrap();
}
