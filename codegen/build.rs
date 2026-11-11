use std::{env, path::PathBuf, process::Command};

/// Builds runtime.wasm, the complete core module codegen appends each program to.
/// The runtime is built under `OUT_DIR`, so builds with different target directories never
/// share (or race on) it. `KIO_TEST_HEAP=1` builds it with the `test-heap` feature.
fn main() {
    for p in ["../runtime", "../lang/src/hmap.rs", "../caps.wit", "build.rs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    println!("cargo:rerun-if-env-changed=KIO_TEST_HEAP");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = out.join("runtime-target");
    let mut cargo = Command::new(env::var("CARGO").unwrap());
    cargo
        .current_dir(&root)
        .args([
            "build",
            "-p",
            "runtime",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .arg("--target-dir")
        .arg(&target)
        // The outer cargo's flags are for the host, not the runtime.
        .env("CARGO_ENCODED_RUSTFLAGS", "");
    if env::var_os("KIO_TEST_HEAP").is_some_and(|v| !v.is_empty() && v != "0") {
        cargo.args(["--features", "test-heap"]);
    }
    assert!(cargo.status().unwrap().success(), "building the runtime failed");
    let wasm = target.join("wasm32-unknown-unknown/release/runtime.wasm");
    std::fs::copy(wasm, out.join("runtime.wasm")).unwrap();
}
