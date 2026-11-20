use std::{env, path::PathBuf, process::Command};

/// Builds runtime.wasm, the complete core module codegen appends each program to.
/// The runtime is built under `OUT_DIR`, so builds with different target directories never
/// share (or race on) it. `KIO_TEST_HEAP=1` builds it with the `test-heap` feature.
fn main() {
    // lang's files the runtime includes by `#[path]` too.
    let shared = [
        "../lang/src/hmap.rs",
        "../lang/src/digest.rs",
        "../lang/src/sort.rs",
        "../lang/src/conv.rs",
    ];
    for p in ["../runtime", "../caps.wit", "build.rs"].into_iter().chain(shared) {
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
        // The outer cargo's flags are for the host, not the runtime. 128 KB of free heap in the
        // initial memory, which dlmalloc takes first: a small request never grows memory (C17).
        // `--growable-table`: the table has no maximum, so Wasmtime bounds every `call_indirect`
        // against the table's size at run time rather than a constant, and a runtime function's
        // native code is the same whatever the program appends to the table (host/build.rs, C18).
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            "-Clink-arg=--initial-heap=131072\u{1f}-Clink-arg=--growable-table",
        );
    if env::var_os("KIO_TEST_HEAP").is_some_and(|v| !v.is_empty() && v != "0") {
        cargo.args(["--features", "test-heap"]);
    }
    assert!(cargo.status().unwrap().success(), "building the runtime failed");
    let wasm = target.join("wasm32-unknown-unknown/release/runtime.wasm");
    let runtime = out.join("runtime.wasm");
    std::fs::copy(wasm, &runtime).unwrap();
    println!("cargo:runtime_wasm={}", runtime.display());
}
