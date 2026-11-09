//! `kio run`'s cache of compiled programs: reused, refused when it is not private, and never
//! trusted when an entry does not match its hash.
use kio_host::{Kio, Limits};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn run(dir: &Path, src: &str) -> String {
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.cache(dir.to_path_buf());
    kio.run(src, vec![]).unwrap().unwrap().value
}

fn entries(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    v.sort();
    v
}

#[test]
fn a_program_is_compiled_once_and_reused() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("kio");
    assert_eq!(run(&dir, "fn main() -> Int { 40 + 2 }"), "42");
    let first = entries(&dir);
    assert_eq!(first.len(), 1);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    let written = std::fs::metadata(&first[0]).unwrap().modified().unwrap();
    assert_eq!(run(&dir, "fn main() -> Int { 40 + 2 }"), "42");
    assert_eq!(entries(&dir), first);
    assert_eq!(
        std::fs::metadata(&first[0]).unwrap().modified().unwrap(),
        written,
        "reused, not rewritten"
    );
    assert_eq!(run(&dir, "fn main() -> Int { 1 }"), "1");
    assert_eq!(entries(&dir).len(), 2);
}

#[test]
fn a_damaged_entry_is_recompiled() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("kio");
    run(&dir, "fn main() -> Int { 7 }");
    let entry = entries(&dir).remove(0);
    let mut bytes = std::fs::read(&entry).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 1;
    std::fs::write(&entry, &bytes).unwrap();
    assert_eq!(run(&dir, "fn main() -> Int { 7 }"), "7");
    assert_ne!(std::fs::read(&entry).unwrap(), bytes, "replaced");
    std::fs::write(&entry, b"short").unwrap();
    assert_eq!(run(&dir, "fn main() -> Int { 7 }"), "7");
}

#[test]
fn a_directory_others_can_write_is_not_used() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("kio");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(run(&dir, "fn main() -> Int { 3 }"), "3");
    assert!(entries(&dir).is_empty());
    // A symlink to a private directory is not the cache either.
    let real = t.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = t.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(run(&link, "fn main() -> Int { 3 }"), "3");
    assert!(entries(&real).is_empty());
}
