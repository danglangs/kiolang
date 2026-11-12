use std::process::Command;

#[test]
fn version_names_the_package_version_and_commit() {
    let out = Command::new(env!("CARGO_BIN_EXE_kio"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let line = String::from_utf8(out.stdout).unwrap();
    let want = format!("kio {} (", env!("CARGO_PKG_VERSION"));
    assert!(line.starts_with(&want) && line.trim_end().ends_with(')'), "{line}");
}

#[test]
fn usage_lists_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8(out.stderr).unwrap().contains("kio --version"));
}
