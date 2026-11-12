// KIO_COMMIT: the short commit `kio --version` names, or "unknown" outside a git checkout.
fn main() {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=KIO_COMMIT={commit}");
    // A new commit moves HEAD or the branch it names.
    for p in ["../.git/HEAD", "../.git/refs/heads", "../.git/packed-refs"] {
        println!("cargo:rerun-if-changed={p}");
    }
}
