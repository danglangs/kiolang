use std::{env, path::PathBuf, process::Command};

fn main() {
    for p in ["../guest", "../lang/src", "../caps.wit"] {
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
}
