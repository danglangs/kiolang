//! K1: the `Kv` capability.
use kio_host::kv::{open_all, Store};
use kio_host::{Kio, Limits};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// Shows a `Result<(), KvErr>`, a `Result<Bool, KvErr>` and an `Option<Bytes>` as text.
const SHOW: &str = r#"
fn show(r: Result<(), KvErr>) -> Str {
  match r { Ok(_) => "ok", Err(e) => if e == KvErr::TooLarge { "too-large" } else { "full" } }
}
fn swapped(r: Result<Bool, KvErr>) -> Str {
  match r { Ok(b) => if b { "true" } else { "false" }, Err(e) => show(Err(e)) }
}
fn val(r: Result<Option<Bytes>, KvErr>) -> Str {
  match r { Ok(v) => match v { Some(b) => b.to_str(), None => "none" }, Err(e) => show(Err(e)) }
}
fn big(n: Int) -> Str {
  let mut s = "x";
  for i in 0..n { s = s + s; }
  s
}
"#;

/// Run `main` (plus `SHOW`) with `store` granted `limit` bytes in memory; `main`'s value.
fn run(main: &str, limit: u64, limits: Limits) -> String {
    let mut kio = Kio::new(limits).unwrap();
    kio.grant_kvs(vec![("store".into(), Arc::new(Store::memory(limit)))]);
    let out = kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap();
    out.value
}

/// `kio` with `args`: exit code, stdout, stderr.
fn kio(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_kio")).args(args).output().unwrap();
    let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (out.status.code().unwrap(), text(out.stdout), text(out.stderr))
}

fn write(dir: &Path, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.to_str().unwrap().into()
}

/// Increments "n" and prints its old value.
const COUNTER: &str = r#"fn main(store: Kv) -> Result<Int, KvErr> {
  let n = match store.get("n")? { Some(b) => b.to_str().parse_int().unwrap_or(0), None => 0 };
  store.set("n", (n + 1).to_str().to_bytes())?;
  Ok(n)
}"#;

#[test]
fn each_method() {
    let main = r#"fn main(store: Kv) -> Str {
      let a = show(store.set("b/2", "two".to_bytes()));
      let b = show(store.set("a", "one".to_bytes()));
      let c = show(store.set("b/1", "uno".to_bytes()));
      let d = show(store.set("a", "ONE".to_bytes()));
      let ks = match store.keys("b/") { Ok(ks) => ks, Err(e) => return "keys failed" };
      let all = match store.keys("") { Ok(ks) => ks, Err(e) => return "keys failed" };
      let got = val(store.get("a"));
      let e = show(store.delete("a"));
      let f = show(store.delete("missing"));
      a + b + c + d + " " + got + " " + val(store.get("b/1")) + " " + ks[0] + "," + ks[1]
        + " " + all.len().to_str() + " " + e + f + " " + val(store.get("a")) + val(store.get("missing"))
    }"#;
    assert_eq!(
        run(main, 1 << 20, Limits::default()),
        "okokokok ONE uno b/1,b/2 3 okok nonenone"
    );
}

#[test]
fn swap_absent_matching_stale() {
    let main = r#"fn main(store: Kv) -> Str {
      let absent = swapped(store.swap("k", None, Some("a".to_bytes())));
      let stale_none = swapped(store.swap("k", None, Some("b".to_bytes())));
      let stale = swapped(store.swap("k", Some("x".to_bytes()), Some("b".to_bytes())));
      let after_stale = val(store.get("k"));
      let matching = swapped(store.swap("k", Some("a".to_bytes()), Some("b".to_bytes())));
      let after = val(store.get("k"));
      let delete = swapped(store.swap("k", Some("b".to_bytes()), None));
      absent + " " + stale_none + " " + stale + " " + after_stale + " " + matching + " " + after + " "
        + delete + " " + val(store.get("k")) + " " + swapped(store.swap("k", None, None))
    }"#;
    assert_eq!(
        run(main, 1 << 20, Limits::default()),
        "true false false a true b true none true"
    );
}

#[test]
fn key_and_value_limits() {
    // big(10) is 1,024 bytes, big(20) is 1 MB.
    let main = r#"fn main(store: Kv) -> Str {
      show(store.set("", "v".to_bytes())) + " " + show(store.set(big(10), "v".to_bytes())) + " "
        + show(store.set(big(10) + "x", "v".to_bytes())) + " " + val(store.get(big(10) + "x")) + " "
        + show(store.delete("")) + " " + swapped(store.swap(big(10) + "x", None, None)) + " "
        + show(store.set("v", big(20).to_bytes())) + " " + show(store.set("w", (big(20) + "x").to_bytes())) + " "
        + swapped(store.swap("w", None, Some((big(20) + "x").to_bytes())))
    }"#;
    assert_eq!(
        run(main, 64 << 20, Limits::default()),
        "too-large ok too-large too-large too-large too-large ok too-large too-large"
    );
}

#[test]
fn full_store_refuses_growth_not_deletes() {
    // 100 bytes: "a" + 89 bytes fits; another key does not, nor does growing "a"; deleting frees it.
    let main = r#"fn main(store: Kv) -> Str {
      let v = big(6) + big(4) + big(3) + "x";
      show(store.set("a", v.to_bytes())) + " " + show(store.set("b", "0123456789x".to_bytes())) + " "
        + swapped(store.swap("b", None, Some("0123456789x".to_bytes()))) + " "
        + show(store.set("a", (v + "yyyyyyyyyyy").to_bytes())) + " "
        + show(store.set("a", "small".to_bytes())) + " " + show(store.set("b", "0123456789x".to_bytes())) + " "
        + show(store.delete("a")) + " " + show(store.delete("b"))
    }"#;
    assert_eq!(run(main, 100, Limits::default()), "ok full full full ok ok ok ok");
}

#[test]
fn keys_over_ten_thousand_is_too_large() {
    let main = r#"fn main(store: Kv) -> Str {
      for i in 0..10000 { let r = store.set("k" + i.to_str(), "".to_bytes()); }
      let at = match store.keys("k") { Ok(ks) => ks.len().to_str(), Err(e) => "too-large" };
      let r = store.set("k-extra", "".to_bytes());
      let over = match store.keys("k") { Ok(ks) => ks.len().to_str(), Err(e) => "too-large" };
      let narrow = match store.keys("k-") { Ok(ks) => ks.len().to_str(), Err(e) => "too-large" };
      at + " " + over + " " + narrow
    }"#;
    let limits = Limits {
        calls: 20_000,
        timeout_secs: 120, // thousands of host calls: slow under load
        ..Limits::default()
    };
    assert_eq!(run(main, 64 << 20, limits), "10000 too-large 1");
}

#[test]
fn io_quota_stops_a_write_before_it_lands() {
    // A write past `--io` stops the run before the store changes.
    let store = Arc::new(Store::memory(1 << 20));
    let limits = Limits {
        io: 4,
        ..Limits::default()
    };
    let mut kio = Kio::new(limits).unwrap();
    kio.grant_kvs(vec![("store".into(), store.clone())]);
    let e = kio
        .run(
            "fn main(store: Kv) -> Result<(), KvErr> { store.set(\"k\", \"12345\".to_bytes()) }",
            vec![],
        )
        .unwrap_err();
    assert_eq!(
        kio.explain(&e).unwrap(),
        "too much I/O (limit 0 MB; raise it with --io)"
    );
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.grant_kvs(vec![("store".into(), store)]);
    let out = kio.run(
        &format!("{SHOW}fn main(store: Kv) -> Str {{ val(store.get(\"k\")) }}"),
        vec![],
    );
    assert_eq!(out.unwrap().unwrap().value, "none");
}

#[test]
fn memory_is_lost_and_file_persists_across_runs() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "count.kio", COUNTER);
    let db = t.path().join("s.kv");
    let file = format!("store={}", db.display());
    for want in ["Ok(0)\n", "Ok(1)\n", "Ok(2)\n"] {
        assert_eq!(kio(&["run", &prog, "--kv", &file]).1, want);
    }
    for _ in 0..2 {
        assert_eq!(kio(&["run", &prog, "--kv", "store=:memory:"]).1, "Ok(0)\n");
    }
}

/// Sets `a`, then `b`, through two runs of `prog`; returns the file.
fn two_records(t: &Path) -> std::path::PathBuf {
    let set = write(
        t,
        "set.kio",
        r#"fn main(store: Kv) -> Result<(), KvErr> { store.set("a", "1".to_bytes())?; store.set("b", "2".to_bytes()) }"#,
    );
    let db = t.join("s.kv");
    assert_eq!(kio(&["run", &set, "--kv", &format!("store={}", db.display())]).0, 0);
    db
}

fn read_ab(t: &Path, db: &Path) -> (i32, String, String) {
    let prog = write(
        t,
        "read.kio",
        &format!(
            "{SHOW}fn main(store: Kv) -> Str {{ val(store.get(\"a\")) + val(store.get(\"b\")) + show(store.set(\"c\", \"3\".to_bytes())) }}"
        ),
    );
    kio(&["run", &prog, "--kv", &format!("store={}", db.display())])
}

#[test]
fn replay_drops_a_truncated_tail() {
    let t = tempfile::tempdir().unwrap();
    let db = two_records(t.path());
    let len = std::fs::metadata(&db).unwrap().len();
    // Each record is 8 + 5 + 1 + 1 = 15 bytes; cut 3 off the second.
    assert_eq!(len, 8 + 2 * 15);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&db)
        .unwrap()
        .set_len(len - 3)
        .unwrap();
    assert_eq!(read_ab(t.path(), &db).1, "1noneok\n");
    // The tail was cut off before the new record was appended, so the file replays cleanly.
    assert_eq!(std::fs::metadata(&db).unwrap().len(), 8 + 2 * 15);
    assert_eq!(read_ab(t.path(), &db).1, "1noneok\n");
}

#[test]
fn corruption_refuses_to_start() {
    let t = tempfile::tempdir().unwrap();
    let db = two_records(t.path());
    let mut data = std::fs::read(&db).unwrap();
    data[8 + 15 + 14] ^= 1; // the second record's value
    std::fs::write(&db, &data).unwrap();
    let (code, _, err) = read_ab(t.path(), &db);
    assert_eq!(code, 2);
    assert!(
        err.contains(&format!("{} is corrupt at byte 23", db.display())),
        "{err}"
    );
    assert!(
        err.contains("hint: restore it from a backup, or move it away to start empty"),
        "{err}"
    );

    std::fs::write(&db, "name,amount\n").unwrap();
    let (code, _, err) = read_ab(t.path(), &db);
    assert_eq!(code, 2);
    assert!(
        err.contains("is not a kio store (no `kio-kv1` header)\n  hint: grant a new file"),
        "{err}"
    );
}

#[test]
fn open_compacts_a_log_over_twice_its_live_data() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(
        t.path(),
        "churn.kio",
        &format!("{SHOW}fn main(store: Kv) -> Str {{ for i in 0..50 {{ let r = store.set(\"k\", (big(6) + i.to_str()).to_bytes()); }} \"\" }}"),
    );
    let db = t.path().join("s.kv");
    let flag = format!("store={}", db.display());
    assert_eq!(kio(&["run", &prog, "--kv", &flag]).0, 0);
    let big = std::fs::metadata(&db).unwrap().len();
    // 50 records of 8 + 5 + 1 + 64..66 bytes.
    assert!(big > 50 * 78, "{big}");
    let get = write(
        t.path(),
        "get.kio",
        &format!("{SHOW}fn main(store: Kv) -> Str {{ val(store.get(\"k\")) }}"),
    );
    let (code, out, _) = kio(&["run", &get, "--kv", &flag]);
    assert_eq!((code, out), (0, format!("{}49\n", "x".repeat(64))));
    assert_eq!(std::fs::metadata(&db).unwrap().len(), 8 + 8 + 5 + 1 + 66);
    assert!(!t.path().join("s.kv.tmp").exists());
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn audit_lines_name_keys_and_sizes_never_values() {
    let main = r#"fn main(store: Kv) -> Str {
      let a = show(store.set("user/1", "secret-value".to_bytes()));
      let b = val(store.get("user/1"));
      let c = match store.keys("user/") { Ok(ks) => ks.len(), Err(e) => 0 };
      let d = swapped(store.swap("user/1", Some("nope".to_bytes()), None));
      let e = show(store.delete("user/1"));
      ""
    }"#;
    let log = Log::default();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(log.clone(), vec![]);
    kio.grant_kvs(vec![("store".into(), Arc::new(Store::memory(1 << 20)))]);
    kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap();
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(!text.contains("secret"), "{text}");
    let lines: Vec<String> = text
        .lines()
        .map(|l| match l.find(r#","ms":"#) {
            Some(i) => format!("{}}}", &l[..i]),
            None => l.to_string(),
        })
        .collect();
    assert!(lines[0].contains(r#""grants":[{"name":"store","kind":"Kv","target":":memory:"}]"#));
    let call = |seq: u32, method: &str, arg: &str, bytes: u32, result: &str| {
        format!(
            r#"{{"event":"call","seq":{seq},"cap":"store","kind":"Kv","method":"{method}","arg":"{arg}","bytes":{bytes},"result":"{result}"}}"#
        )
    };
    assert_eq!(lines[1], call(1, "set", "user/1", 12, "ok"));
    assert_eq!(lines[2], call(2, "get", "user/1", 12, "ok"));
    assert_eq!(lines[3], call(3, "keys", "user/", 6, "ok"));
    assert_eq!(lines[4], call(4, "swap", "user/1", 4, "stale"));
    assert_eq!(lines[5], call(5, "delete", "user/1", 0, "ok"));
    assert!(lines[6].starts_with(r#"{"event":"end","outcome":"ok","calls":5,"io":34,"#));
}

#[test]
fn report_lists_kv_methods_and_the_host_checks_them() {
    let t = tempfile::tempdir().unwrap();
    let src = r#"fn main(store: Kv) -> Result<(), KvErr> {
      store.set("a", "1".to_bytes())?;
      let v = store.get("a")?;
      let ks = store.keys("")?;
      let s = store.swap("a", v, None)?;
      store.delete("a")
    }"#;
    let prog = write(t.path(), "all.kio", src);
    assert_eq!(kio(&["check", &prog]).1, "main : store delete get keys set swap\n");

    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.grant_kvs(vec![("store".into(), Arc::new(Store::memory(1 << 20)))]);
    kio.hide_from_report("store", "swap");
    let e = kio.run(src, vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&e).unwrap(),
        "audit: call `store.swap` not in the authority report (a checker bug)"
    );
}

#[test]
fn grant_errors_have_hints() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(t.path(), "count.kio", COUNTER);
    let db = t.path().join("s.kv");
    let file = format!("store={}", db.display());
    let link = t.path().join("link.kv");

    let (code, _, err) = kio(&["run", &prog]);
    assert_eq!(code, 1);
    assert!(
        err.contains("error: no grant for `store: Kv`\n  hint: grant it: `--kv store=path` or `--kv store=:memory:`"),
        "{err}"
    );

    let (code, _, err) = kio(&["run", &prog, "--kv", &file, "--kv", "store=:memory:"]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: --kv store=:memory:: `store` is granted twice\n  hint: give each store one name\n"
        )
    );

    let (code, _, err) = kio(&["run", &prog, "--kv", &file, "--kv", "other=:memory:"]);
    assert_eq!(code, 1);
    assert!(
        err.contains(
            "error: grant `other` is not a parameter of `main`\n  hint: grant only `main`'s parameters: store"
        ),
        "{err}"
    );

    std::os::unix::fs::symlink(&db, &link).unwrap();
    let (code, _, err) = kio(&[
        "run",
        &prog,
        "--kv",
        &file,
        "--kv",
        &format!("other={}", link.display()),
    ]);
    assert_eq!(code, 2);
    assert!(
        err.ends_with(": the same file as --kv store\n  hint: grant one store once, or give each grant its own file\n"),
        "{err}"
    );

    let (code, _, err) = kio(&["run", &prog, "--kv", "store"]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: --kv store: expected name=path, name=path:ro or name=:memory:\n"
        )
    );
    let (code, _, err) = kio(&["run", &prog, "--kv-mb", "0"]);
    assert_eq!(
        (code, err.as_str()),
        (2, "kio: --kv-mb 0: expected a positive integer\n")
    );
    let (code, _, err) = kio(&["run", &prog, "--kv", &format!("store={}", t.path().display())]);
    assert_eq!(code, 2);
    assert!(
        err.starts_with(&format!("kio: --kv store={0}: {0}: ", t.path().display())),
        "{err}"
    );
}

/// A `kio run --listen` child; killed on drop.
struct Server(Child, u16);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `kio run app --listen 127.0.0.1:0 extra..` and read the port from its stderr.
fn listen(app: &str, extra: &[&str]) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kio"))
        .args(["run", app, "--listen", "127.0.0.1:0"])
        .args(extra)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("listening on http://") {
        line.clear();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "kio exited before listening");
    }
    let port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    Server(child, port)
}

/// GET `/` on a fresh connection: status and body.
fn get(port: u16) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    (head.split(' ').nth(1).unwrap().parse().unwrap(), body.into())
}

/// Each request adds one to "n" with a `swap` loop and answers the value it stored.
const INCREMENT: &str = r#"fn main(req: Incoming, res: Outgoing, store: Kv) -> Result<(), WebErr> {
  let mut n = 0;
  let mut done = false;
  while !done {
    let old = match store.get("n") { Ok(v) => v, Err(e) => return res.send(500, [], [], "".to_bytes()) };
    n = match old { Some(b) => b.to_str().parse_int().unwrap_or(0), None => 0 } + 1;
    done = match store.swap("n", old, Some(n.to_str().to_bytes())) { Ok(b) => b, Err(e) => return res.send(500, [], [], "".to_bytes()) };
  }
  res.send(200, [], [], n.to_str().to_bytes())
}"#;

#[test]
fn fifty_concurrent_swap_increments_end_at_fifty() {
    let t = tempfile::tempdir().unwrap();
    let app = write(t.path(), "inc.kio", INCREMENT);
    let db = t.path().join("n.kv");
    let flag = format!("store={}", db.display());
    let server = listen(&app, &["--kv", &flag]);
    let port = server.1;
    let threads: Vec<_> = (0..50)
        .map(|_| {
            std::thread::spawn(move || loop {
                // A request told 503 (all workers busy) never ran: retry it.
                match get(port) {
                    (200, body) => return body.parse::<u32>().unwrap(),
                    (status, _) => assert_eq!(status, 503),
                }
            })
        })
        .collect();
    let mut seen: Vec<u32> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    seen.sort();
    assert_eq!(seen, (1..=50).collect::<Vec<_>>());

    // The server holds the file's lock: a second process cannot open the store.
    let count = write(t.path(), "count.kio", COUNTER);
    let (code, _, err) = kio(&["run", &count, "--kv", &flag]);
    assert_eq!(code, 2);
    assert!(
        err.contains(&format!("{} is in use by another process (it holds the file's lock)\n  hint: stop the other kio, or grant another file", db.display())),
        "{err}"
    );
    drop(server);
    assert_eq!(kio(&["run", &count, "--kv", &flag]).1, "Ok(50)\n");
}

#[test]
fn open_all_refuses_a_locked_file_in_process() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("s.kv").display().to_string();
    let first = open_all(&[("a".into(), db.clone())], 1).unwrap();
    let Err(e) = open_all(&[("b".into(), db.clone())], 1) else {
        panic!("opened a locked store")
    };
    assert!(e.contains("is in use by another process"), "{e}");
    drop(first);
    // Other tests fork children, which hold the open file until they exec: retry briefly.
    let reopened = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        open_all(&[("b".into(), db.clone())], 1).is_ok()
    });
    assert!(reopened);
}

/// Adversarial review: a key with quotes, control characters and JSON punctuation is one escaped
/// JSON string in the audit line, never a second line or field.
#[test]
fn audit_escapes_a_hostile_key() {
    let main = r#"fn main(store: Kv) -> Str {
      let k = "a\"b\nc\\d\0e\te}{,\"result\":\"ok\"";
      show(store.set(k, "v".to_bytes()))
    }"#;
    let log = Log::default();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(log.clone(), vec![]);
    kio.grant_kvs(vec![("store".into(), Arc::new(Store::memory(1 << 20)))]);
    kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap();
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text.lines().count(), 3, "{text}");
    assert!(
        text.lines()
            .nth(1)
            .unwrap()
            .contains(r#""arg":"a\"b\u000ac\\d\u0000e\u0009e}{,\"result\":\"ok\"","bytes":1,"result":"ok""#),
        "{text}"
    );
}

/// Adversarial review: crafted record headers and bodies are refused as corruption at their
/// offset, never a panic or a wrong replay; only a record cut short at the end is dropped.
#[test]
fn replay_refuses_crafted_records_without_panicking() {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
            }
        }
        !c
    }
    fn body(tag: u8, klen: u32, key: &[u8], value: &[u8]) -> Vec<u8> {
        let mut b = vec![tag];
        b.extend_from_slice(&klen.to_le_bytes());
        b.extend_from_slice(key);
        b.extend_from_slice(value);
        b
    }
    fn framed(len: u32, body: &[u8]) -> Vec<u8> {
        let mut r = len.to_le_bytes().to_vec();
        r.extend_from_slice(&crc32(body).to_le_bytes());
        r.extend_from_slice(body);
        r
    }
    let good = body(1, 1, b"a", b"1");
    let crafted: Vec<(&str, Vec<u8>)> = vec![
        ("len 0", framed(0, &good)),
        ("len u32::MAX", framed(u32::MAX, &good)),
        ("len over BODY_MAX", framed((1 + 4 + 1024 + (1 << 20)) + 1, &good)),
        ("tag 3", framed(7, &body(3, 1, b"a", b"1"))),
        ("klen 0", framed(7, &body(1, 0, b"a", b"1"))),
        ("klen u32::MAX", framed(7, &body(1, u32::MAX, b"a", b"1"))),
        ("klen past body", framed(7, &body(1, 3, b"a", b"1"))),
        ("klen 1025", framed(5 + 1025, &body(1, 1025, &[b'k'; 1025], b""))),
        ("delete with value", framed(7, &body(2, 1, b"a", b"1"))),
        ("invalid utf-8 key", framed(7, &body(1, 1, b"\xff", b"1"))),
        ("bad crc", {
            let mut r = framed(7, &good);
            r[4] ^= 1;
            r
        }),
    ];
    let t = tempfile::tempdir().unwrap();
    for (i, (what, rec)) in crafted.iter().enumerate() {
        let db = t.path().join(format!("c{i}.kv"));
        let mut data = b"kio-kv1\n".to_vec();
        data.extend_from_slice(rec);
        std::fs::write(&db, &data).unwrap();
        let err = open_all(&[("s".into(), db.to_str().unwrap().into())], 1)
            .err()
            .unwrap_or_default();
        assert!(err.contains("is corrupt at byte 8"), "{what}: {err}");
    }

    // A value holding what looks like a record is just a value, and a record cut short is dropped.
    let db = t.path().join("value.kv");
    let mut data = b"kio-kv1\n".to_vec();
    let inner = framed(7, &body(1, 1, b"z", b"9"));
    let outer = body(1, 1, b"a", &inner);
    data.extend_from_slice(&framed(outer.len() as u32, &outer));
    data.extend_from_slice(&framed(7, &body(1, 1, b"b", b"2"))[..12]);
    std::fs::write(&db, &data).unwrap();
    let store = Store::open(&db, 1 << 20).unwrap();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.grant_kvs(vec![("store".into(), Arc::new(store))]);
    let main = r#"fn main(store: Kv) -> Str {
      let n = match store.get("a") { Ok(v) => match v { Some(b) => b.len(), None => 0 }, Err(e) => 0 };
      n.to_str() + " " + val(store.get("z")) + " " + val(store.get("b"))
    }"#;
    let out = kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap();
    assert_eq!(out.value, "15 none none");
    assert_eq!(std::fs::metadata(&db).unwrap().len(), 8 + 8 + 21);
}

/// Adversarial review: a server-style loop of 1 MB writes keeps the log near its live size, the
/// compacted store reads back after reopening, and the file stays locked across compactions.
#[test]
fn writes_compact_the_log_online() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("s.kv");
    let store = Arc::new(Store::open(&db, 64 << 20).unwrap());
    let main = r#"fn main(store: Kv) -> Str {
      let v = big(20).to_bytes();
      for i in 0..40 { let a = store.set("k", v); let b = store.set("k", "".to_bytes()); }
      show(store.set("k", "last".to_bytes()))
    }"#;
    let mut kio = Kio::new(Limits {
        timeout_secs: 120,
        ..Limits::default()
    })
    .unwrap();
    kio.grant_kvs(vec![("store".into(), store.clone())]);
    assert_eq!(kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap().value, "ok");
    let len = std::fs::metadata(&db).unwrap().len();
    assert!(len < 4 << 20, "{len}");
    assert!(!t.path().join("s.kv.tmp").exists());
    let Err(e) = open_all(&[("b".into(), db.display().to_string())], 1) else {
        panic!("opened a store another grant holds")
    };
    assert!(e.contains("is in use by another process"), "{e}");
    drop(kio);
    drop(store);
    let mut kio = Kio::new(Limits {
        timeout_secs: 120,
        ..Limits::default()
    })
    .unwrap();
    kio.grant_kvs(vec![("store".into(), Arc::new(Store::open(&db, 1 << 20).unwrap()))]);
    let out = kio.run(
        &format!("{SHOW}fn main(store: Kv) -> Str {{ val(store.get(\"k\")) }}"),
        vec![],
    );
    assert_eq!(out.unwrap().unwrap().value, "last");
}

/// Adversarial review: a `--kv` file inside a `--dir` root could be replaced or crafted through
/// the `Dir`; the run is refused before any code runs.
#[test]
fn a_store_inside_a_granted_dir_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let prog = write(
        t.path(),
        "overlap.kio",
        r#"fn main(d: Dir, store: Kv) -> Result<(), KvErr> { store.set("a", "1".to_bytes()) }"#,
    );
    let data = t.path().join("data");
    std::fs::create_dir_all(data.join("sub")).unwrap();
    let dir = format!("d={}", data.display());
    for db in [data.join("db.log"), data.join("sub/db.log"), data.join("sub/../db.log")] {
        let (code, out, err) = kio(&["run", &prog, "--dir", &dir, "--kv", &format!("store={}", db.display())]);
        assert_eq!((code, out.as_str()), (2, ""), "{err}");
        assert_eq!(
            err,
            "kio: `--kv store` is inside `--dir d`\n  hint: keep stores outside granted directories\n"
        );
        assert!(!db.exists());
    }
    let db = t.path().join("db.log");
    let (code, _, err) = kio(&["run", &prog, "--dir", &dir, "--kv", &format!("store={}", db.display())]);
    assert_eq!(code, 0, "{err}");
}

/// Adversarial review: a symlink planted at `<store>.tmp` is removed, not followed, when the log
/// is compacted; a symlink at the store path itself is refused, even a dangling one.
#[test]
fn compaction_never_follows_a_symlink() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("s.kv");
    let store = Store::open(&db, 1 << 20).unwrap();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.grant_kvs(vec![("store".into(), Arc::new(store))]);
    let churn = r#"fn main(store: Kv) -> Str { for i in 0..50 { let r = store.set("k", i.to_str().to_bytes()); } "" }"#;
    kio.run(churn, vec![]).unwrap().unwrap();
    drop(kio);
    let victim = t.path().join("victim.txt");
    std::fs::write(&victim, "keep").unwrap();
    std::os::unix::fs::symlink(&victim, t.path().join("s.kv.tmp")).unwrap();
    drop(Store::open(&db, 1 << 20).unwrap()); // compacts: 50 records, one live
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    assert!(std::fs::symlink_metadata(&db).unwrap().is_file());
    assert_eq!(std::fs::metadata(&db).unwrap().len(), 8 + 8 + 5 + 1 + 2);
    assert!(std::fs::symlink_metadata(t.path().join("s.kv.tmp")).is_err());

    let link = t.path().join("link.kv");
    let target = t.path().join("missing.kv");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let Err(e) = Store::open(&link, 1 << 20) else {
        panic!("opened a symlink")
    };
    assert_eq!(
        e,
        format!(
            "{} is a symbolic link\n  hint: grant the file it points to",
            link.display()
        )
    );
    assert!(!target.exists());
}

/// Adversarial review: a refused `keys` is charged to `--io` like a refused `Dir.list`, and an
/// over-long prefix is refused before any work.
#[test]
fn too_large_keys_is_charged() {
    let main = r#"fn main(store: Kv) -> Str {
      for i in 0..10001 { let r = store.set("k" + i.to_str(), "".to_bytes()); }
      let over = match store.keys("k") { Ok(ks) => ks.len().to_str(), Err(e) => "too-large" };
      let long = match store.keys(big(10) + "x") { Ok(ks) => ks.len().to_str(), Err(e) => "too-large" };
      over + " " + long
    }"#;
    let log = Log::default();
    let mut kio = Kio::new(Limits {
        calls: 20_000,
        timeout_secs: 120, // thousands of host calls: slow under load
        ..Limits::default()
    })
    .unwrap();
    kio.audit(log.clone(), vec![]);
    kio.grant_kvs(vec![("store".into(), Arc::new(Store::memory(64 << 20)))]);
    let out = kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap();
    assert_eq!(out.value, "too-large too-large");
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let keys: Vec<&str> = text.lines().filter(|l| l.contains(r#""method":"keys""#)).collect();
    assert_eq!(keys.len(), 2);
    for l in keys {
        assert!(l.contains(r#""bytes":10000,"result":"too-large""#), "{l}");
    }
}

/// Adversarial review: a tail of zeros, as a power loss leaves, is dropped like a record cut
/// short; anything non-zero after the last record is still corruption.
#[test]
fn replay_drops_a_zero_filled_tail() {
    let t = tempfile::tempdir().unwrap();
    let db = two_records(t.path());
    let mut data = std::fs::read(&db).unwrap();
    data.extend_from_slice(&[0; 12]);
    std::fs::write(&db, &data).unwrap();
    assert_eq!(read_ab(t.path(), &db).1, "12ok\n");
    assert_eq!(std::fs::metadata(&db).unwrap().len(), 8 + 3 * 15);

    let mut data = std::fs::read(&db).unwrap();
    data.extend_from_slice(&[0; 12]);
    data.push(1);
    std::fs::write(&db, &data).unwrap();
    let (code, _, err) = read_ab(t.path(), &db);
    assert_eq!(code, 2);
    assert!(err.contains("is corrupt at byte 53"), "{err}");
}

/// Answers the value of "token", `none`, or `err`.
const READ_TOKEN: &str = r#"fn main(req: Incoming, res: Outgoing, store: Kv) -> Result<(), WebErr> {
  let v = match store.get("token") { Ok(o) => match o { Some(b) => b.to_str(), None => "none" }, Err(e) => "err" };
  res.send(200, [], [], v.to_bytes())
}"#;

/// A program that sets "token" to `value`.
fn set_token(t: &Path, value: &str) -> String {
    let src = format!(r#"fn main(store: Kv) -> Result<(), KvErr> {{ store.set("token", "{value}".to_bytes()) }}"#);
    write(t, &format!("set-{value}.kio"), &src)
}

/// H12: each refusal of a read-only grant, at startup with exit 2 and a hint.
#[test]
fn read_only_grant_refusals() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("s.kv");
    let ro = format!("store={}:ro", db.display());
    let counter = write(t.path(), "count.kio", COUNTER);

    // Missing: nothing to read yet, and nothing is created.
    let reader = write(
        t.path(),
        "read.kio",
        &format!("{SHOW}fn main(store: Kv) -> Str {{ val(store.get(\"token\")) }}"),
    );
    let (code, _, err) = kio(&["run", &reader, "--kv", &ro]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        format!(
            "kio: --kv {ro}: {} does not exist\n  hint: a read-only store is written by another kio: run it once first, or drop `:ro`\n",
            db.display()
        )
    );
    assert!(!db.exists());

    // A program that can write the store, before anything is opened.
    let (code, _, err) = kio(&["run", &counter, "--kv", &ro]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: `store` is granted read-only but `main` can call `store set`\n  hint: drop `:ro` or remove the writes\n"
        )
    );
    for (method, call) in [
        ("delete", r#"store.delete("k")"#),
        ("swap", r#"store.swap("k", None, None)"#),
    ] {
        let src = format!("fn main(store: Kv) -> Str {{ let r = {call}; \"\" }}");
        let prog = write(t.path(), &format!("{method}.kio"), &src);
        let (code, _, err) = kio(&["run", &prog, "--kv", &ro]);
        assert_eq!(code, 2);
        assert!(
            err.contains(&format!("`main` can call `store {method}`\n  hint: drop `:ro`")),
            "{err}"
        );
    }

    let (code, _, err) = kio(&["run", &reader, "--kv", "store=:memory::ro"]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: --kv store=:memory::ro: an in-memory store is never read-only\n  hint: drop `:ro`; only this process can write it\n"
        )
    );

    // The file now exists and reads; a symlink to it and a copy inside a `--dir` are refused.
    let rw = format!("store={}", db.display());
    assert_eq!(kio(&["run", &set_token(t.path(), "a"), "--kv", &rw]).0, 0);
    assert_eq!(kio(&["run", &reader, "--kv", &ro]).1, "a\n");
    let link = t.path().join("link.kv");
    std::os::unix::fs::symlink(&db, &link).unwrap();
    let (code, _, err) = kio(&["run", &reader, "--kv", &format!("store={}:ro", link.display())]);
    assert_eq!(code, 2);
    assert!(
        err.contains("is a symbolic link\n  hint: grant the file it points to"),
        "{err}"
    );
    let data = t.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::copy(&db, data.join("s.kv")).unwrap();
    let both = write(
        t.path(),
        "both.kio",
        "fn main(d: Dir, store: Kv) -> Result<Option<Bytes>, KvErr> { store.get(\"token\") }",
    );
    let (code, _, err) = kio(&[
        "run",
        &both,
        "--dir",
        &format!("d={}", data.display()),
        "--kv",
        &format!("store={}:ro", data.join("s.kv").display()),
    ]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: `--kv store` is inside `--dir d`\n  hint: keep stores outside granted directories\n"
        )
    );
}

/// H12: a write that reaches a read-only store (here through the library, which has no startup
/// check) stops the run and is audited; the file is unchanged.
#[test]
fn a_write_to_a_read_only_store_stops_the_run() {
    let t = tempfile::tempdir().unwrap();
    let db = two_records(t.path());
    let before = std::fs::read(&db).unwrap();
    for (method, call) in [
        ("set", r#"store.set("a", "x".to_bytes())"#),
        ("delete", r#"store.delete("a")"#),
        ("swap", r#"store.swap("a", None, None)"#),
    ] {
        let kvs = open_all(&[("store".into(), format!("{}:ro", db.display()))], 1).unwrap();
        let log = Log::default();
        let mut kio = Kio::new(Limits::default()).unwrap();
        kio.audit(log.clone(), vec![]);
        kio.grant_kvs(kvs);
        let main = format!("fn main(store: Kv) -> Str {{ let r = {call}; \"done\" }}");
        let e = kio.run(&main, vec![]).unwrap_err();
        assert_eq!(
            kio.explain(&e).unwrap(),
            format!("kv: `store` is granted read-only, so `store {method}` stops the run")
        );
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].contains(&format!(r#""kind":"Kv","target":"{}:ro""#, db.display())),
            "{text}"
        );
        assert!(
            lines[1].contains(&format!(
                r#""method":"{method}","arg":"a","bytes":0,"result":"stopped:read-only""#
            )),
            "{text}"
        );
        assert!(lines[2].starts_with(r#"{"event":"end","outcome":"error""#), "{text}");
    }
    assert_eq!(std::fs::read(&db).unwrap(), before);
}

/// H12: a `--listen` reader of `path:ro` sees what a separate writer process stores, without
/// either holding the other off, including after the writer compacts the file (a new inode).
#[test]
fn a_reader_follows_a_writer_process_through_compaction() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("s.kv");
    let rw = format!("store={}", db.display());
    assert_eq!(kio(&["run", &set_token(t.path(), "v1"), "--kv", &rw]).0, 0);
    let app = write(t.path(), "read.kio", READ_TOKEN);
    let server = listen(&app, &["--kv", &format!("store={}:ro", db.display())]);
    assert_eq!(get(server.1), (200, "v1".into()));
    let ino = |p: &Path| std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(p).unwrap());
    let first = ino(&db);
    for v in ["v2", "v3"] {
        let (code, _, err) = kio(&["run", &set_token(t.path(), v), "--kv", &rw]);
        assert_eq!(code, 0, "{err}");
        assert_eq!(get(server.1), (200, v.into()));
    }
    assert_eq!(ino(&db), first, "appended in place");
    // Three records of one key: the next writer compacts the log on open, then appends.
    assert_eq!(kio(&["run", &set_token(t.path(), "v4"), "--kv", &rw]).0, 0);
    assert_ne!(ino(&db), first, "compacted");
    assert_eq!(get(server.1), (200, "v4".into()));
    assert_eq!(kio(&["run", &set_token(t.path(), "v5"), "--kv", &rw]).0, 0);
    assert_eq!(get(server.1), (200, "v5".into()));
}

/// H12 review: the startup refusal reads `main`'s report line, which charges writes made through
/// helpers, generic functions, module functions and recursion to the parameter `main` passes.
#[test]
fn read_only_refusal_sees_writes_through_helpers() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("s.kv");
    let rw = format!("store={}", db.display());
    let ro = format!("store={}:ro", db.display());
    assert_eq!(kio(&["run", &set_token(t.path(), "a"), "--kv", &rw]).0, 0);
    std::fs::write(
        t.path().join("util.kio"),
        r#"pub fn w(k: Kv) -> Result<(), KvErr> { k.set("token", "x".to_bytes()) }"#,
    )
    .unwrap();
    let shapes = [
        ("helper", "fn w(k: Kv) -> Result<(), KvErr> { k.set(\"token\", \"x\".to_bytes()) }\nfn main(store: Kv) -> Result<(), KvErr> { w(store) }", "set"),
        ("generic", "fn w<T>(x: T, k: Kv) -> Result<(), KvErr> { k.delete(\"token\") }\nfn main(store: Kv) -> Result<(), KvErr> { w::<Int>(1, store) }", "delete"),
        ("recursion", "fn a(k: Kv, n: Int) -> Result<Bool, KvErr> { if n == 0 { k.swap(\"token\", None, None) } else { b(k, n - 1) } }\nfn b(k: Kv, n: Int) -> Result<Bool, KvErr> { a(k, n) }\nfn main(store: Kv) -> Result<Bool, KvErr> { b(store, 3) }", "swap"),
        ("module", "mod util;\nfn main(store: Kv) -> Result<(), KvErr> { util::w(store) }", "set"),
        ("renamed", "fn w(store: Kv) -> Result<(), KvErr> { store.set(\"token\", \"x\".to_bytes()) }\nfn main(auth: Kv) -> Result<(), KvErr> { let r = w(auth); Ok(()) }", "set"),
    ];
    for (name, src, method) in shapes {
        let prog = write(t.path(), &format!("{name}.kio"), src);
        let grant = if name == "renamed" {
            ro.replace("store=", "auth=")
        } else {
            ro.clone()
        };
        let param = if name == "renamed" { "auth" } else { "store" };
        let (code, _, err) = kio(&["run", &prog, "--kv", &grant]);
        assert_eq!(code, 2, "{name}: {err}");
        assert_eq!(
            err,
            format!("kio: `{param}` is granted read-only but `main` can call `{param} {method}`\n  hint: drop `:ro` or remove the writes\n"),
            "{name}"
        );
    }
    // The same file as a reader and a writer, in either order, and twice read-only: one grant.
    let two = write(
        t.path(),
        "two.kio",
        "fn main(a: Kv, b: Kv) -> Str { let x = a.get(\"token\"); let y = b.get(\"token\"); \"ok\" }",
    );
    let a_ro = format!("a={}:ro", db.display());
    for (x, y) in [
        (a_ro.as_str(), rw.replace("store=", "b=").as_str()),
        (rw.replace("store=", "a=").as_str(), ro.replace("store=", "b=").as_str()),
        (a_ro.as_str(), ro.replace("store=", "b=").as_str()),
    ] {
        let (code, _, err) = kio(&["run", &two, "--kv", x, "--kv", y]);
        assert_eq!(code, 2, "{x} {y}: {err}");
        assert!(err.contains("the same file as --kv a"), "{x} {y}: {err}");
    }
    assert_eq!(
        std::fs::read(&db).unwrap().len(),
        8 + 8 + 1 + 4 + 5 + 1,
        "nothing was written"
    );
}
