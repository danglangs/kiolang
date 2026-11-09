//! `kio run`'s cache of compiled programs: reused, refused when it is not private, never trusted
//! when an entry does not match its hash, and not used by a run granted a `--dir` over it.
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

/// `kio run` with `$XDG_CACHE_HOME` at `xdg`: exit code, stdout, stderr.
fn kio_run(xdg: &Path, args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kio"))
        .arg("run")
        .args(args)
        .env("XDG_CACHE_HOME", xdg)
        .output()
        .unwrap();
    let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (out.status.code().unwrap(), text(out.stdout), text(out.stderr))
}

/// Writes the bytes in `src/bytes` to the path in `src/name` under `d`; returns 1.
const PLANT: &str = r#"fn main(d: Dir, src: Dir) -> Result<Int, IoErr> {
  let name = src.read("name")?.to_str();
  d.write(name, src.read("bytes")?)?;
  Ok(1)
}"#;
/// Same grants, returns 2: its native code under `PLANT`'s key would make `PLANT` return 2.
const OTHER: &str = "fn main(d: Dir, src: Dir) -> Result<Int, IoErr> { Ok(2) }";

/// Adversarial review (C5): a program granted a `--dir` over the cache could write an entry that a
/// later run loads as native code. Here it plants another program's real native code under its
/// own key; were the cache used, its second run would run that code and print 2.
#[test]
fn a_run_granted_a_dir_over_the_cache_does_not_use_it() {
    let t = tempfile::tempdir().unwrap();
    let w = |name: &str, src: &str| {
        let p = t.path().join(name);
        std::fs::write(&p, src).unwrap();
        p.to_str().unwrap().to_string()
    };
    let (plant, other) = (w("plant.kio", PLANT), w("other.kio", OTHER));
    let src = t.path().join("src");
    std::fs::create_dir(&src).unwrap();
    let src_grant = format!("src={}", src.display());

    // `PLANT`'s key and `OTHER`'s native code, from a cache no program is granted.
    let pre = t.path().join("pre");
    let empty = t.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let d_empty = format!("d={}", empty.display());
    assert_eq!(
        kio_run(&pre, &[&plant, "--dir", &d_empty, "--dir", &src_grant]).0,
        1,
        "no name file yet: Err"
    );
    let key = entries(&pre.join("kio")).remove(0);
    assert_eq!(
        kio_run(&pre, &[&other, "--dir", &d_empty, "--dir", &src_grant]).1,
        "Ok(2)\n"
    );
    let native = (entries(&pre.join("kio")).into_iter()).find(|e| *e != key).unwrap();
    std::fs::copy(native, src.join("bytes")).unwrap();
    let key = key.file_name().unwrap().to_str().unwrap();

    // The grant is the cache, an ancestor of it, or inside it.
    let xdg = t.path().join("xdg");
    let cache = xdg.join("kio");
    std::fs::create_dir_all(cache.join("sub")).unwrap();
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    let report = "main : d write, src read\n";
    for (root, name) in [(cache.clone(), key.to_string()), (xdg.clone(), format!("kio/{key}"))] {
        std::fs::write(src.join("name"), &name).unwrap();
        let d = format!("d={}", root.display());
        for _ in 0..2 {
            let (code, out, err) = kio_run(&xdg, &[&plant, "--dir", &d, "--dir", &src_grant]);
            assert_eq!(
                (code, out.as_str(), err.as_str()),
                (0, "Ok(1)\n", report),
                "{}",
                root.display()
            );
        }
        assert!(cache.join(key).is_file(), "planted, and not loaded");
        std::fs::remove_file(cache.join(key)).unwrap();
    }
    // A grant inside the cache cannot reach an entry's path, but is kept apart all the same.
    std::fs::write(src.join("name"), key).unwrap();
    let d = format!("d={}", cache.join("sub").display());
    let (code, out, _) = kio_run(&xdg, &[&plant, "--dir", &d, "--dir", &src_grant]);
    assert_eq!((code, out.as_str()), (0, "Ok(1)\n"));
    assert!(!cache.join(key).exists(), "not cached");
    // Granted no overlapping dir, a run uses the cache.
    let d_empty = format!("d={}", empty.display());
    assert_eq!(
        kio_run(&xdg, &[&plant, "--dir", &d_empty, "--dir", &src_grant]).1,
        "Ok(1)\n"
    );
    assert!(cache.join(key).is_file(), "used without an overlapping grant");
}

/// A store inside the cache could become an entry: refused at startup.
#[test]
fn a_store_inside_the_cache_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let prog = t.path().join("p.kio");
    std::fs::write(
        &prog,
        r#"fn main(s: Kv) -> Result<(), KvErr> { s.set("a", "1".to_bytes()) }"#,
    )
    .unwrap();
    let prog = prog.to_str().unwrap();
    let xdg = t.path().join("xdg");
    for db in [xdg.join("kio/s.kv"), xdg.join("kio/new/s.kv")] {
        let (code, out, err) = kio_run(&xdg, &[prog, "--kv", &format!("s={}", db.display())]);
        assert_eq!((code, out.as_str()), (2, ""), "{err}");
        assert_eq!(
            err,
            "kio: `--kv s` is inside kio's compile cache\n  hint: keep stores outside `$XDG_CACHE_HOME/kio` (default `~/.cache/kio`)\n"
        );
        assert!(!db.exists());
    }
    std::fs::create_dir(&xdg).unwrap();
    let (code, _, err) = kio_run(&xdg, &[prog, "--kv", &format!("s={}", xdg.join("s.kv").display())]);
    assert_eq!(code, 0, "{err}");
}

/// What the overlap rule guards: the host's own checks (owner, regular file, trailing SHA-256)
/// pass on any file this user wrote, so Wasmtime's `deserialize`, which trusts the native code it
/// is given, is the only gate on an entry. A forged entry with a bad header is refused there and
/// replaced; one with a valid header would run as host code.
#[test]
fn host_side_checks_pass_on_a_forged_entry_wasmtime_is_the_only_gate() {
    use sha2::{Digest, Sha256};
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("kio");
    assert_eq!(run(&dir, "fn main() -> Int { 9 }"), "9");
    let entry = entries(&dir).remove(0);
    let mut forged = b"not real native code".to_vec();
    let sha = Sha256::digest(&forged);
    forged.extend_from_slice(&sha);
    std::fs::write(&entry, &forged).unwrap();
    assert_eq!(run(&dir, "fn main() -> Int { 9 }"), "9");
    assert_ne!(
        std::fs::read(&entry).unwrap(),
        forged,
        "refused by Wasmtime and replaced"
    );
}
