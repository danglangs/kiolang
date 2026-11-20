//! kio keeps no compiled code on disk (open question 75): every run compiles from the Wasm it
//! just generated, and a `.cwasm` an earlier kio cached, or a program planted, is never loaded
//! and never touched. The runtime's native code is built into the binary (C18); no file is read
//! for it either.
use kio_host::{Kio, Limits};
use sha2::{Digest, Sha256};
use std::path::Path;

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
/// Same grants, returns 2: its native code loaded under `PLANT`'s name would make `PLANT` return 2.
const OTHER: &str = "fn main(d: Dir, src: Dir) -> Result<Int, IoErr> { Ok(2) }";

/// The case the old cache failed. A run granted a `--dir` over the old cache plants `OTHER`'s
/// real native code, with a valid trailer, under `PLANT`'s name in the old cache format; later
/// runs of `PLANT` with an ordinary directory return `PLANT`'s value and leave the entry as it
/// was, byte for byte.
#[test]
fn a_planted_entry_is_never_loaded_or_touched() {
    let t = tempfile::tempdir().unwrap();
    // The old format: named by the SHA-256 of kio's version, a 0 byte and the program's Wasm;
    // holding the native code, then its SHA-256.
    let kio = Kio::new(Limits::default()).unwrap();
    let prog = t.path().join("plant.kio");
    let prog = prog.to_str().unwrap();
    let files = |src: &str| [(prog.to_string(), src.to_string())];
    let (wasm, _) = kio.wasm_and_native(&files(PLANT)).unwrap();
    let mut sha = Sha256::new();
    sha.update(env!("CARGO_PKG_VERSION"));
    sha.update([0]);
    sha.update(&wasm);
    let name: String = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let name = format!("{name}.cwasm");
    let (_, mut native) = kio.wasm_and_native(&files(OTHER)).unwrap();
    let sum = Sha256::digest(&native);
    native.extend_from_slice(&sum);
    let w = |p: &str, bytes: &[u8]| std::fs::write(t.path().join(p), bytes).unwrap();
    w("plant.kio", PLANT.as_bytes());
    for d in ["d", "src"] {
        std::fs::create_dir(t.path().join(d)).unwrap();
    }
    let src = format!("src={}", t.path().join("src").display());

    // Planted through a grant of the cache's parent.
    let xdg = t.path().join("xdg");
    let entry = xdg.join("kio").join(&name);
    std::fs::create_dir_all(xdg.join("kio")).unwrap();
    w("src/name", format!("kio/{name}").as_bytes());
    w("src/bytes", &native);
    let over = format!("d={}", xdg.display());
    let (code, out, err) = kio_run(&xdg, &[prog, "--dir", &over, "--dir", &src]);
    assert_eq!((code, out.as_str()), (0, "Ok(1)\n"), "{err}");
    assert_eq!(std::fs::read(&entry).unwrap(), native, "planted");
    let modified = std::fs::metadata(&entry).unwrap().modified().unwrap();

    w("src/name", b"out");
    let ordinary = format!("d={}", t.path().join("d").display());
    for _ in 0..2 {
        let (code, out, err) = kio_run(&xdg, &[prog, "--dir", &ordinary, "--dir", &src]);
        assert_eq!((code, out.as_str()), (0, "Ok(1)\n"), "{err}");
    }
    assert_eq!(std::fs::read(&entry).unwrap(), native, "left as it was");
    assert_eq!(std::fs::metadata(&entry).unwrap().modified().unwrap(), modified);
    let names: Vec<_> = std::fs::read_dir(xdg.join("kio"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [std::ffi::OsString::from(&name)], "nothing written beside it");
}

/// Every run compiles from the Wasm it just generated: `kio run` creates nothing under
/// `$XDG_CACHE_HOME`, and the library, which has no cache to configure, runs from source alone.
#[test]
fn every_run_compiles_and_writes_no_cache() {
    let t = tempfile::tempdir().unwrap();
    let prog = t.path().join("p.kio");
    std::fs::write(&prog, "fn main() -> Int { 3 }").unwrap();
    let xdg = t.path().join("xdg");
    for _ in 0..2 {
        assert_eq!(kio_run(&xdg, &[prog.to_str().unwrap()]).1, "3\n");
    }
    assert!(!xdg.exists(), "no cache directory");
    let mut kio = Kio::new(Limits::default()).unwrap();
    for _ in 0..2 {
        assert_eq!(kio.run("fn main() -> Int { 3 }", vec![]).unwrap().unwrap().value, "3");
    }
}

/// No path in the host loads native code from bytes: `Component::deserialize` and
/// `deserialize_file` trust what they are given. Components are made with `Component::new` from
/// the Wasm codegen just produced.
#[test]
fn the_host_never_deserializes_native_code() {
    let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains("deserialize"), "{}", path.display());
        }
    }
}

/// The runtime's native code comes from the binary (host/src/runtime_cache.rs): compiling a
/// program looks up every function in the embedded cache and finds the runtime's there, so only
/// the program's own functions, the entry and the trampolines are compiled. Cranelift 0.137 keys
/// a function partly on state a reused compile context carries over (`DataFlowGraph::clear` does
/// not clear `exception_tables`), so a parallel compile misses some runtime functions; the
/// floor here allows for it.
#[test]
fn the_runtime_is_served_from_the_binary() {
    let (hits0, misses0) = Kio::runtime_cache_stats();
    let mut kio = Kio::new(Limits::default()).unwrap();
    assert_eq!(kio.run("fn main() -> Int { 3 }", vec![]).unwrap().unwrap().value, "3");
    let (hits, misses) = Kio::runtime_cache_stats();
    let (hits, misses) = (hits - hits0, misses - misses0);
    println!("runtime cache: {hits} hits, {misses} misses");
    assert!(hits > 500, "hits {hits}, misses {misses}");
    assert!(hits * 100 >= (hits + misses) * 75, "hits {hits}, misses {misses}");
}

/// A `--listen` server's engine (the pooling allocator, one compile thread) builds the same
/// native code as `kio run`'s, so it is served from the binary too.
#[test]
fn a_server_is_served_from_the_binary() {
    let (hits0, _) = Kio::runtime_cache_stats();
    let src = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
        res.send(200, [], [], "ok".to_bytes())
    }"#;
    kio_host::Server::new(vec![("main.kio".into(), src.into())], Limits::web())
        .unwrap()
        .unwrap();
    let (hits, _) = Kio::runtime_cache_stats();
    assert!(hits - hits0 > 500, "hits {}", hits - hits0);
}
