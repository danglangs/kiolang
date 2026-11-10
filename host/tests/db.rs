//! H17: the `Db` capability (SQLite). Containment first: a program reaches nothing but the
//! granted database, cannot run code, and cannot hold the host's CPU or memory past the run's
//! limits.
use kio_host::db::{open_all, Db};
use kio_host::{Interp, Kio, Limits, Sources};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Shows results as text: `show` a count or error, `rows` a result set (`;` between rows, `,`
/// between columns, text quoted, blobs in hex).
const SHOW: &str = r#"
fn err(e: DbErr) -> Str {
  match e {
    DbErr::Invalid(m) => "invalid: " + m,
    DbErr::Denied => "denied",
    DbErr::Busy => "busy",
    DbErr::TooLarge => "too-large",
    DbErr::Constraint(m) => "constraint: " + m,
  }
}
fn show(r: Result<Int, DbErr>) -> Str { match r { Ok(n) => n.to_str(), Err(e) => err(e) } }
fn cell(v: SqlValue) -> Str {
  match v {
    SqlValue::Null => "null",
    SqlValue::Int(n) => n.to_str(),
    SqlValue::Float(x) => x.to_str(),
    SqlValue::Text(s) => "'" + s + "'",
    SqlValue::Blob(b) => "x" + b.to_hex(),
  }
}
fn rows(r: Result<List<List<SqlValue>>, DbErr>) -> Str {
  match r { Ok(rs) => rs.map(|row| row.map(cell).join(",")).join(";"), Err(e) => err(e) }
}
"#;

fn memory() -> Sources {
    Sources {
        dbs: vec![("db".into(), Arc::new(Db::memory().unwrap()))],
        ..Sources::default()
    }
}

/// Run `main` (plus `SHOW`) compiled with an in-memory `db` granted; `main`'s value.
fn run(main: &str, limits: Limits) -> String {
    let mut kio = Kio::new(limits).unwrap();
    kio.grant_sources(memory());
    kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap().value
}

/// `run` both ways, interpreted and compiled, which must agree.
fn both(main: &str) -> String {
    let mut interp = Interp::new(Limits::default()).unwrap();
    interp.grant_sources(memory());
    let interpreted = interp.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap().value;
    let compiled = run(main, Limits::default());
    assert_eq!(compiled, interpreted);
    compiled
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

#[test]
fn each_method_and_every_type_round_trips() {
    let main = r#"fn main(db: Db) -> Str {
      let a = show(db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, i INTEGER, f REAL, s TEXT, b BLOB)", []));
      let b = show(db.exec("INSERT INTO t (i, f, s, b) VALUES (?, ?, ?, ?)",
        [SqlValue::Int(-9223372036854775807 - 1), SqlValue::Float(-0.5), SqlValue::Text("héllo \"q\""), SqlValue::Blob("\0\tÿ".to_bytes())]));
      let c = show(db.transaction(
        ["INSERT INTO t (i, f, s, b) VALUES (?, ?, ?, ?)", "INSERT INTO t (i) VALUES (?)", "UPDATE t SET f = f * 2 WHERE i IS NOT NULL"],
        [[SqlValue::Int(9223372036854775807), SqlValue::Float(0.25), SqlValue::Text(""), SqlValue::Blob("".to_bytes())], [SqlValue::Null], []]));
      let d = rows(db.query("SELECT i, f, s, b FROM t ORDER BY id", []));
      let e = rows(db.query("SELECT ?, ?, typeof(?), ?", [SqlValue::Int(3), SqlValue::Text("x"), SqlValue::Float(0.0 / 0.0), SqlValue::Null]));
      let f = rows(db.query("SELECT count(*) FROM t WHERE s = ?", [SqlValue::Text("héllo \"q\"")]));
      let g = show(db.exec("DELETE FROM t WHERE i IS NULL", []));
      let h = rows(db.query("SELECT 1 WHERE 0", []));
      a + " " + b + " " + c + " | " + d + " | " + e + " | " + f + " " + g + " [" + h + "]"
    }"#;
    assert_eq!(
        both(main),
        "0 1 4 | -9223372036854775808,-1,'héllo \"q\"',x0009c3bf;9223372036854775807,0.5,'',x;null,null,null,null | 3,'x','null',null | 1 1 []"
    );
}

#[test]
fn errors_teach_and_agree_both_ways() {
    let main = r#"fn main(db: Db) -> Str {
      let a = show(db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, n TEXT UNIQUE NOT NULL)", []));
      let b = show(db.exec("INSERT INTO t (n) VALUES (?)", [SqlValue::Text("a")]));
      let dup = show(db.exec("INSERT INTO t (n) VALUES (?)", [SqlValue::Text("a")]));
      let null = show(db.exec("INSERT INTO t (n) VALUES (NULL)", []));
      let syntax = rows(db.query("SELEC 1", []));
      let two = show(db.exec("INSERT INTO t (n) VALUES ('b'); INSERT INTO t (n) VALUES ('c')", []));
      let few = show(db.exec("DELETE FROM t WHERE n = ? AND n = ?", [SqlValue::Text("d")]));
      let many = show(db.exec("DELETE FROM t WHERE n = ?", [SqlValue::Text("d"), SqlValue::Text("e")]));
      let rows_in_exec = show(db.exec("SELECT n FROM t", []));
      let returning = show(db.exec("INSERT INTO t (n) VALUES ('r') RETURNING id", []));
      let blank = rows(db.query("  -- nothing", []));
      let nul = show(db.exec("SELECT 1 \0", []));
      let dqs = rows(db.query("SELECT \"not a string\"", []));
      let comment = rows(db.query("SELECT count(*) FROM t; -- a trailing comment is fine", []));
      a + " " + b + " [" + dup + "] [" + null + "] [" + syntax + "] [" + two + "] [" + few + "] [" + many + "] ["
        + rows_in_exec + "] [" + returning + "] [" + blank + "] [" + nul + "] [" + dqs + "] " + comment
    }"#;
    assert_eq!(
        both(main),
        "0 1 [constraint: UNIQUE constraint failed: t.n] [constraint: NOT NULL constraint failed: t.n] \
         [invalid: near \"SELEC\": syntax error at byte 0] [invalid: one statement per call] \
         [invalid: the statement takes 2 parameters, 1 were given] [invalid: the statement takes 1 parameters, 2 were given] \
         [invalid: the statement returns rows: run it with `query`] [invalid: the statement returns rows: run it with `query`] \
         [invalid: no statement] [invalid: the SQL holds a NUL byte] [invalid: no such column: \"not a string\" - should this be a string literal in single-quotes? at byte 7] 1"
    );
}

/// Containment: every statement that could reach another file, change the connection, run code
/// or escape `transaction` is `denied`, and leaves nothing behind. The allowed read-only pragmas
/// still work.
#[test]
fn denied_statements_reach_nothing() {
    let t = tempfile::tempdir().unwrap();
    let other = t.path().join("other.db");
    let main = format!(
        r#"fn main(db: Db) -> Str {{
      let setup = show(db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, n TEXT)", []));
      let denied = [
        db.exec("ATTACH DATABASE '{other}' AS o", []),
        db.exec("ATTACH DATABASE ':memory:' AS m", []),
        db.exec("ATTACH DATABASE 'file:{other}?mode=rwc' AS u", []),
        db.exec("DETACH DATABASE main", []),
        db.exec("VACUUM INTO '{other}'", []),
        db.exec("VACUUM", []),
        db.exec("PRAGMA journal_mode = DELETE", []),
        db.exec("PRAGMA temp_store_directory = '{dir}'", []),
        db.exec("PRAGMA temp_store = FILE", []),
        db.exec("PRAGMA writable_schema = ON", []),
        db.exec("PRAGMA foreign_keys = OFF", []),
        db.exec("PRAGMA schema_version = 9", []),
        db.exec("PRAGMA user_version = 9", []),
        db.exec("PRAGMA page_size = 1024", []),
        db.exec("PRAGMA data_version = 1", []),
        db.exec("BEGIN", []),
        db.exec("COMMIT", []),
        db.exec("ROLLBACK", []),
        db.exec("SAVEPOINT s", []),
      ];
      let defensive = show(db.exec("UPDATE sqlite_master SET sql = 'x'", []));
      let denied_queries = [
        db.query("SELECT load_extension('{other}')", []),
        db.query("SELECT load_extension('{other}', 'init')", []),
        db.query("SELECT fts3_tokenizer('simple')", []),
        db.query("PRAGMA journal_mode", []),
        db.query("PRAGMA database_list", []),
        db.query("PRAGMA compile_options", []),
        db.query("SELECT * FROM pragma_database_list", []),
      ];
      let allowed = rows(db.query("PRAGMA table_info(t)", [])) + " | " + rows(db.query("PRAGMA foreign_keys", []))
        + " | " + rows(db.query("SELECT name FROM pragma_table_info('t')", [])) + " | " + rows(db.query("PRAGMA index_list(t)", []))
        + " | " + rows(db.query("PRAGMA page_size", []));
      // FTS5 and R*Tree run their own PRAGMA reads through the authorizer; creating one reports
      // the row its module wrote to a shadow table.
      let vtabs = show(db.exec("CREATE VIRTUAL TABLE f USING fts5(body)", [])) + show(db.exec("INSERT INTO f VALUES ('hello world')", []))
        + rows(db.query("SELECT rowid FROM f WHERE f MATCH 'hello'", [])) + show(db.exec("CREATE VIRTUAL TABLE r USING rtree(id, x0, x1)", []))
        + show(db.exec("INSERT INTO r VALUES (1, 0.0, 1.0)", [])) + rows(db.query("SELECT id FROM r WHERE x0 < 0.5", []));
      setup + " " + denied.map(show).join(",") + " " + denied_queries.map(rows).join(",") + " [" + defensive + "] " + allowed + " " + vtabs
    }}"#,
        other = other.display(),
        dir = t.path().display()
    );
    let out = both(&main);
    let denied = ["denied"; 19].join(",");
    let denied_queries = ["denied"; 7].join(",");
    assert_eq!(
        out,
        format!(
            "0 {denied} {denied_queries} [invalid: table sqlite_master may not be modified] 0,'id','INTEGER',0,null,1;1,'n','TEXT',0,null,0 | 1 | 'id';'n' |  | 4096 111111"
        )
    );
    assert!(!other.exists(), "VACUUM INTO or ATTACH created a file");
    assert_eq!(
        std::fs::read_dir(t.path()).unwrap().count(),
        0,
        "a statement left a file"
    );
}

/// A `:memory:` grant has no file; a file grant writes only its own file and SQLite's `-wal`
/// and `-shm` beside it, and a temp table lives in memory.
#[test]
fn a_file_grant_touches_only_its_own_files() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("a.db");
    let main = write(
        t.path(),
        "m.kio",
        r#"fn main(db: Db) -> Result<Int, DbErr> {
      db.exec("CREATE TABLE IF NOT EXISTS t (n INTEGER)", [])?;
      db.exec("CREATE TEMP TABLE tmp AS SELECT 1 AS x", [])?;
      db.exec("INSERT INTO t SELECT x FROM tmp", [])
    }"#,
    );
    let (code, out, err) = kio(&["run", &main, "--db", &format!("db={}", db.display())]);
    assert_eq!((code, out.as_str()), (0, "Ok(1)\n"), "{err}");
    let mut names: Vec<String> = std::fs::read_dir(t.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    // The WAL is checkpointed and removed when the last connection closes.
    assert_eq!(names, ["a.db", "m.kio"]);
}

#[test]
fn a_transaction_that_fails_partway_rolls_back() {
    let main = r#"fn main(db: Db) -> Str {
      let a = show(db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, n TEXT UNIQUE)", []));
      let b = show(db.transaction(
        ["INSERT INTO t (n) VALUES (?)", "INSERT INTO t (n) VALUES (?)", "INSERT INTO t (n) VALUES (?)"],
        [[SqlValue::Text("a")], [SqlValue::Text("b")], [SqlValue::Text("a")]]));
      let after_failure = rows(db.query("SELECT count(*) FROM t", []));
      let c = show(db.transaction(["INSERT INTO t (n) VALUES (?)", "SELECT 1"], [[SqlValue::Text("c")], []]));
      let d = show(db.transaction(["INSERT INTO t (n) VALUES (?)"], [[SqlValue::Text("c")], []]));
      let e = show(db.transaction([], []));
      let f = show(db.transaction(["INSERT INTO t (n) VALUES (?)", "DELETE FROM t"], [[SqlValue::Text("z")], []]));
      let again = show(db.exec("INSERT INTO t (n) VALUES (?)", [SqlValue::Text("d")]));
      a + " [" + b + "] " + after_failure + " [" + c + "] [" + d + "] " + e + " " + f + " " + again + " " + rows(db.query("SELECT count(*) FROM t", []))
    }"#;
    assert_eq!(
        both(main),
        "0 [constraint: UNIQUE constraint failed: t.n] 0 [invalid: the statement returns rows: run it with `query`] \
         [invalid: 1 statements but 2 parameter lists: give each statement its own list] 0 2 1 1"
    );
}

/// The result limits (10,000 rows, 8 MB), SQLite's lowered limits (8 MB values, 1 MB of SQL,
/// 4,096 parameters, no attached databases) and the statement cap of a transaction.
#[test]
fn limits_are_too_large() {
    let main = r#"fn main(db: Db) -> Str {
      let count = |r: Result<List<List<SqlValue>>, DbErr>| match r { Ok(rs) => rs.len().to_str(), Err(e) => err(e) };
      let at = count(db.query("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 10000) SELECT x FROM c", []));
      let over = count(db.query("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 10001) SELECT x FROM c", []));
      let two = count(db.query("SELECT zeroblob(4194304) UNION ALL SELECT zeroblob(4194304)", []));
      let three = count(db.query("SELECT zeroblob(4194304) UNION ALL SELECT zeroblob(4194304) UNION ALL SELECT zeroblob(1)", []));
      let value = count(db.query("SELECT zeroblob(8388609)", []));
      let mut sql = "SELECT 1 -- ";
      for i in 0..20 { sql = sql + sql; }
      let long = count(db.query(sql, []));
      let mut qs: List<Str> = [];
      let mut ps: List<SqlValue> = [];
      for i in 0..4097 { qs.push("?"); ps.push(SqlValue::Int(i)); }
      let params = count(db.query("SELECT 1 WHERE 1 IN (" + qs.join(", ") + ")", ps));
      qs.pop();
      ps.pop();
      let fits = count(db.query("SELECT 1 WHERE 1 IN (" + qs.join(", ") + ")", ps));
      let mut stmts: List<Str> = [];
      let mut lists: List<List<SqlValue>> = [];
      for i in 0..1001 { stmts.push("SELECT 1 WHERE 0"); lists.push([]); }
      let many = show(db.transaction(stmts, lists));
      at + " " + over + " " + two + " " + three + " " + value + " " + long + " [" + params + "] " + fits + " " + many
    }"#;
    let limits = Limits {
        io: 1 << 30,
        ..Limits::default()
    };
    assert_eq!(
        run(main, limits),
        "10000 too-large 2 too-large too-large too-large [invalid: too many SQL variables at byte 12309] 1 too-large"
    );
}

/// SQLite runs on the host, outside fuel: a statement still running at the run's deadline is
/// interrupted and the run stops as a timeout, promptly.
#[test]
fn a_long_query_is_stopped_at_the_deadline() {
    let main = r#"fn main(db: Db) -> Str {
      rows(db.query("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 4000000000) SELECT count(*) FROM c", []))
    }"#;
    let limits = Limits {
        timeout_secs: 1,
        ..Limits::default()
    };
    let mut kio = Kio::new(limits).unwrap();
    kio.grant_sources(memory());
    let start = Instant::now();
    let e = kio.run(&format!("{SHOW}{main}"), vec![]).unwrap_err();
    assert_eq!(
        kio.explain(&e).unwrap(),
        "timed out after 1 s (raise it with --timeout)"
    );
    assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
}

/// SQLite's memory is capped (its temp store lives there): a statement that would need more
/// fails as `too-large` instead of growing the host. Run in a child process, since the cap is
/// per process and a failed allocation could reach other tests' statements.
#[test]
fn a_runaway_temp_table_is_too_large() {
    let t = tempfile::tempdir().unwrap();
    let main = write(
        t.path(),
        "m.kio",
        &format!(
            "{SHOW}fn main(db: Db) -> Str {{
      let big = show(db.exec(\"CREATE TEMP TABLE big AS WITH RECURSIVE c(x) AS (SELECT zeroblob(1000000) UNION ALL SELECT x FROM c LIMIT 400) SELECT x FROM c\", []));
      big + \" \" + rows(db.query(\"SELECT 1\", []))
    }}"
        ),
    );
    let (code, out, err) = kio(&["run", &main, "--db", "db=:memory:"]);
    assert_eq!((code, out.as_str()), (0, "too-large 1\n"), "{err}");
}

/// Under `--listen`, one connection per grant behind a mutex: concurrent requests each insert
/// one row and read the count; none is lost, and the file is in WAL mode while served.
#[test]
fn fifty_concurrent_requests_insert_fifty_rows() {
    let t = tempfile::tempdir().unwrap();
    let app = write(
        t.path(),
        "app.kio",
        r#"fn main(req: Incoming, res: Outgoing, db: Db) -> Result<(), WebErr> {
      let ok = db.exec("CREATE TABLE IF NOT EXISTS hits (id INTEGER PRIMARY KEY, who TEXT)", []);
      let n = match db.transaction(["INSERT INTO hits (who) VALUES (?)"], [[SqlValue::Text(req.target())]]) {
        Ok(n) => n, Err(e) => return res.send(500, [], [], "".to_bytes()) };
      match db.query("SELECT count(*) FROM hits", []) {
        Ok(rows) => match rows[0][0] { SqlValue::Int(c) => res.send(200, [], [], c.to_str().to_bytes()), _ => res.send(500, [], [], "".to_bytes()) },
        Err(e) => res.send(500, [], [], "".to_bytes()),
      }
    }"#,
    );
    let db = t.path().join("hits.db");
    let flag = format!("db={}", db.display());
    let server = listen(&app, &["--db", &flag]);
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
    assert!(seen.iter().zip(1..).all(|(&c, i)| c >= i) && seen[49] == 50, "{seen:?}");
    assert!(db.with_extension("db-wal").exists(), "WAL mode while served");
    // Another process reads the same file meanwhile (SQLite's own locking; no kio lock).
    let count = write(
        t.path(),
        "count.kio",
        "fn main(db: Db) -> Result<List<List<SqlValue>>, DbErr> { db.query(\"SELECT count(*) FROM hits\", []) }",
    );
    let (code, out, err) = kio(&["run", &count, "--db", &format!("{}:ro", flag)]);
    assert_eq!((code, out.as_str()), (0, "Ok([[SqlValue::Int(50)]])\n"), "{err}");
    drop(server);
    assert_eq!(kio(&["run", &count, "--db", &flag]).1, "Ok([[SqlValue::Int(50)]])\n");
}

/// Another connection holding the write lock makes a write `busy` after the timeout, not a hang.
#[test]
fn busy_when_another_connection_holds_the_lock() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("b.db");
    let dbs = open_all(&[("db".into(), path.display().to_string())]).unwrap();
    let other = rusqlite::Connection::open(&path).unwrap();
    other
        .execute_batch("CREATE TABLE t (n); BEGIN IMMEDIATE; INSERT INTO t VALUES (1);")
        .unwrap();
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.grant_sources(Sources {
        dbs,
        ..Sources::default()
    });
    let main = r#"fn main(db: Db) -> Str { show(db.exec("INSERT INTO t VALUES (2)", [])) + " " + rows(db.query("SELECT count(*) FROM t", [])) }"#;
    let start = Instant::now();
    let out = kio.run(&format!("{SHOW}{main}"), vec![]).unwrap().unwrap().value;
    // WAL: a reader is never blocked by the writer, and sees the committed state (no rows).
    assert_eq!(out, "busy 0");
    let waited = start.elapsed();
    assert!(
        waited >= Duration::from_secs(2) && waited < Duration::from_secs(6),
        "{waited:?}"
    );
    other.execute_batch("COMMIT").unwrap();
}

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

/// Audit lines carry the SQL (cut at 256 characters), the parameter count, rows or rows changed
/// and bytes, never a parameter value.
#[test]
fn audit_lines_carry_sql_and_counts_never_values() {
    let main = r#"fn main(db: Db) -> Str {
      let a = show(db.exec("CREATE TABLE t (n TEXT)", []));
      let b = show(db.exec("INSERT INTO t VALUES (?)", [SqlValue::Text("secret-value")]));
      let c = rows(db.query("SELECT n FROM t WHERE n = ?", [SqlValue::Text("secret-value")]));
      let d = show(db.transaction(["INSERT INTO t VALUES (?)", "DELETE FROM t"], [[SqlValue::Text("secret-two")], []]));
      let mut long = "SELECT 1 -- ";
      for i in 0..8 { long = long + long; }
      let e = rows(db.query(long, []));
      let f = show(db.exec("ATTACH 'x' AS x", []));
      ""
    }"#;
    let log = Log(Arc::default());
    let mut kio = Kio::new(Limits::default()).unwrap();
    kio.audit(Log(log.0.clone()), vec![]);
    kio.grant_sources(memory());
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
    assert!(lines[0].contains(r#""grants":[{"name":"db","kind":"Db","target":":memory:"}]"#));
    let call = |seq: u32, method: &str, arg: &str, bytes: u32, result: &str, extra: &str| {
        format!(
            r#"{{"event":"call","seq":{seq},"cap":"db","kind":"Db","method":"{method}","arg":"{arg}","bytes":{bytes},"result":"{result}"{extra}}}"#
        )
    };
    assert_eq!(
        lines[1],
        call(
            1,
            "exec",
            "CREATE TABLE t (n TEXT)",
            0,
            "ok",
            r#","params":0,"changes":0"#
        )
    );
    assert_eq!(
        lines[2],
        call(
            2,
            "exec",
            "INSERT INTO t VALUES (?)",
            12,
            "ok",
            r#","params":1,"changes":1"#
        )
    );
    assert_eq!(
        lines[3],
        call(
            3,
            "query",
            "SELECT n FROM t WHERE n = ?",
            24,
            "ok",
            r#","params":1,"rows":1"#
        )
    );
    assert_eq!(
        lines[4],
        call(
            4,
            "transaction",
            "INSERT INTO t VALUES (?); DELETE FROM t",
            10,
            "ok",
            r#","statements":2,"params":1,"changes":3"#
        )
    );
    let cut = &"SELECT 1 -- ".repeat(256)[..256];
    assert_eq!(lines[5], call(5, "query", cut, 8, "ok", r#","params":0,"rows":1"#));
    assert_eq!(
        lines[6],
        call(6, "exec", "ATTACH 'x' AS x", 0, "denied", r#","params":0"#)
    );
    assert!(
        lines[7].starts_with(r#"{"event":"end","outcome":"ok","calls":6,"io":54,"#),
        "{}",
        lines[7]
    );
}

/// Each refusal of a read-only grant at startup, exit 2 with a hint, as `--kv`'s (H12); and a
/// `query` that writes a read-only database is `denied` at the call.
#[test]
fn read_only_grant_refusals() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("r.db");
    let ro = format!("db={}:ro", db.display());
    let rw = format!("db={}", db.display());
    let reader = write(
        t.path(),
        "read.kio",
        &format!("{SHOW}fn main(db: Db) -> Str {{ rows(db.query(\"SELECT n FROM t\", [])) }}"),
    );
    // Missing: nothing to read yet, and nothing is created.
    let (code, _, err) = kio(&["run", &reader, "--db", &ro]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        format!(
            "kio: --db {ro}: {} does not exist\n  hint: a read-only database is written by another kio: run it once first, or drop `:ro`\n",
            db.display()
        )
    );
    assert!(!db.exists());
    // A program that can write it, before anything is opened: through a helper too.
    for (method, call) in [
        ("exec", r#"db.exec("DELETE FROM t", [])"#),
        ("transaction", r#"db.transaction(["DELETE FROM t"], [[]])"#),
        ("exec", r#"helper(db)"#),
    ] {
        let src = format!(
            "fn helper(d: Db) -> Result<Int, DbErr> {{ d.exec(\"DELETE FROM t\", []) }}\nfn main(db: Db) -> Str {{ let r = {call}; \"\" }}"
        );
        let prog = write(t.path(), "w.kio", &src);
        let (code, _, err) = kio(&["run", &prog, "--db", &ro]);
        assert_eq!(
            (code, err.as_str()),
            (
                2,
                format!("kio: `db` is granted read-only but `main` can call `db {method}`\n  hint: drop `:ro` or remove the writes\n").as_str()
            )
        );
    }
    let (code, _, err) = kio(&["run", &reader, "--db", "db=:memory::ro"]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: --db db=:memory::ro: an in-memory database is never read-only\n  hint: drop `:ro`; only this process can write it\n"
        )
    );
    // Written once, read-only reads it; a `query` that writes is denied and changes nothing.
    let writer = write(
        t.path(),
        "write.kio",
        r#"fn main(db: Db) -> Result<Int, DbErr> {
      db.exec("CREATE TABLE t (n TEXT)", [])?;
      db.exec("INSERT INTO t VALUES ('v1')", [])
    }"#,
    );
    assert_eq!(kio(&["run", &writer, "--db", &rw]).1, "Ok(1)\n");
    assert_eq!(kio(&["run", &reader, "--db", &ro]).1, "'v1'\n");
    let sneaky = write(
        t.path(),
        "sneak.kio",
        &format!(
            "{SHOW}fn main(db: Db) -> Str {{
      rows(db.query(\"INSERT INTO t VALUES ('v2') RETURNING n\", [])) + \" \" + rows(db.query(\"DELETE FROM t RETURNING n\", []))
        + \" \" + rows(db.query(\"CREATE TABLE u (x)\", [])) + \" \" + rows(db.query(\"SELECT n FROM t\", []))
    }}"
        ),
    );
    assert_eq!(kio(&["run", &sneaky, "--db", &ro]).1, "denied denied denied 'v1'\n");
    // A symlink to it is refused.
    let link = t.path().join("link.db");
    std::os::unix::fs::symlink(&db, &link).unwrap();
    let (code, _, err) = kio(&["run", &reader, "--db", &format!("db={}:ro", link.display())]);
    assert_eq!(code, 2);
    assert!(
        err.contains("is a symbolic link\n  hint: grant the file it points to"),
        "{err}"
    );
}

/// A write that reaches a read-only database through the library (no startup check there) stops
/// the run and is audited `stopped:read-only`; the file is unchanged.
#[test]
fn a_write_to_a_read_only_db_stops_the_run() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("r.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t (n); INSERT INTO t VALUES (1);")
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    for (method, call) in [
        ("exec", r#"db.exec("DELETE FROM t", [])"#),
        ("transaction", r#"db.transaction(["DELETE FROM t"], [[]])"#),
    ] {
        let dbs = open_all(&[("db".into(), format!("{}:ro", path.display()))]).unwrap();
        let log = Log(Arc::default());
        let mut kio = Kio::new(Limits::default()).unwrap();
        kio.audit(Log(log.0.clone()), vec![]);
        kio.grant_sources(Sources {
            dbs,
            ..Sources::default()
        });
        let main = format!("fn main(db: Db) -> Str {{ let r = {call}; \"done\" }}");
        let e = kio.run(&main, vec![]).unwrap_err();
        assert_eq!(
            kio.explain(&e).unwrap(),
            format!("db: `db` is granted read-only, so `db {method}` stops the run")
        );
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].contains(&format!(r#""kind":"Db","target":"{}:ro""#, path.display())),
            "{text}"
        );
        assert!(
            lines[1].contains(&format!(
                r#""method":"{method}","arg":"DELETE FROM t","bytes":0,"result":"stopped:read-only""#
            )),
            "{text}"
        );
        assert!(lines[2].starts_with(r#"{"event":"end","outcome":"error""#), "{text}");
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// The path rules of a `Kv` file apply: not inside a granted `--dir` or the cache, not a `--kv`
/// store's file, not the audit log's, each name once, each file once; and the grant hint.
#[test]
fn path_rules_and_grant_errors() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let prog = write(
        t.path(),
        "p.kio",
        "fn main(d: Dir, db: Db) -> Result<Int, DbErr> { db.exec(\"CREATE TABLE IF NOT EXISTS t (n)\", []) }",
    );
    let dir = format!("d={}", data.display());
    let inside = format!("db={}", data.join("a.db").display());
    let (code, _, err) = kio(&["run", &prog, "--dir", &dir, "--db", &inside]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: `--db db` is inside `--dir d`\n  hint: keep stores outside granted directories\n"
        )
    );
    let outside = format!("db={}", t.path().join("a.db").display());
    let (code, _, err) = kio(&["run", &prog, "--dir", &dir, "--db", &outside, "--db", &outside]);
    assert_eq!(code, 2);
    assert!(err.contains("`db` is granted twice"), "{err}");
    let (code, _, err) = kio(&["run", &prog, "--dir", &dir]);
    assert_eq!(code, 1);
    assert!(
        err.contains("no grant for `db: Db`\n  hint: grant it: `--db db=path` (a SQLite file), `--db db=path:ro` or `--db db=:memory:`"),
        "{err}"
    );
    let (code, _, err) = kio(&["run", &prog, "--dir", &dir, "--db", "db"]);
    assert_eq!(
        (code, err.as_str()),
        (2, "kio: --db db: expected name=path, name=path:ro or name=:memory:\n")
    );
    // Same file as a store, as the audit log (also through SQLite's `-wal`), or as another database.
    let both = write(
        t.path(),
        "both.kio",
        "fn main(store: Kv, db: Db) -> Result<Int, DbErr> { db.exec(\"SELECT 1 WHERE 0\", []) }",
    );
    let shared = t.path().join("shared.file");
    let (code, _, err) = kio(&[
        "run",
        &both,
        "--kv",
        &format!("store={}", shared.display()),
        "--db",
        &format!("db={}", shared.display()),
    ]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "kio: `--db db` is the same file as `--kv store`\n  hint: give each its own file\n"
        )
    );
    let (code, _, err) = kio(&[
        "run",
        &prog,
        "--dir",
        &dir,
        "--db",
        &outside,
        "--audit",
        &format!("{}-wal", t.path().join("a.db").display()),
    ]);
    assert_eq!(code, 2);
    assert!(
        err.starts_with(&format!(
            "kio: `--audit {}-wal` is the same file as `--db db`\n  hint: give the audit log its own file",
            t.path().join("a.db").display()
        )),
        "{err}"
    );
    let two = write(
        t.path(),
        "two.kio",
        "fn main(a: Db, b: Db) -> Result<Int, DbErr> { b.exec(\"SELECT 1 WHERE 0\", []) }",
    );
    let (code, _, err) = kio(&[
        "run",
        &two,
        "--db",
        &format!("a={}", t.path().join("x.db").display()),
        "--db",
        &format!("b={}", t.path().join("x.db").display()),
    ]);
    assert_eq!(code, 2);
    assert!(err.contains("the same file as --db a"), "{err}");
    // A good run, and a `:memory:` one.
    let (code, out, err) = kio(&["run", &prog, "--dir", &dir, "--db", &outside]);
    assert_eq!((code, out.as_str()), (0, "Ok(0)\n"), "{err}");
    assert_eq!(kio(&["run", &prog, "--dir", &dir, "--db", "db=:memory:"]).1, "Ok(0)\n");
}

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
