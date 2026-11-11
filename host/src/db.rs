//! The `Db` capability (H17): one SQLite database per grant (`--db name=path`, `path:ro` or
//! `:memory:`), shared by every run of the process behind a mutex. SQL text is the program's,
//! and SQLite runs on the host outside Wasm fuel, so the connection is fenced: an authorizer
//! refuses every statement that could reach another file, change the connection or run code
//! (ATTACH, DETACH, PRAGMA but a read-only few, transactions outside `transaction`,
//! `load_extension`); `SQLITE_DBCONFIG_DEFENSIVE` and a limit of zero attached databases close
//! the back doors (`VACUUM INTO`, `writable_schema`); SQLite's own limits are lowered; a progress
//! handler interrupts a statement once the run's deadline passes, and host versions of `instr`,
//! `replace`, `like` and `glob` (which scan inside one virtual-machine step) check it as they
//! scan; a process-wide hard heap limit bounds SQLite's memory, where its temp store lives, and
//! `max_page_count` the database's size.
use crate::kio::caps::db::{Db as Handle, DbErr, HostDb, SqlValue};
use crate::State;
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{ffi, Connection, ErrorCode, OpenFlags};
use std::ffi::{c_int, c_void, CStr};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, Once, TryLockError};
use std::time::{Duration, Instant};
use wasmtime::component::Resource;
use wasmtime::Result;

/// The longest statement, in bytes (`SQLITE_LIMIT_SQL_LENGTH`).
const SQL_MAX: i32 = 1 << 20;
/// The largest text or blob, as a parameter, a column or inside SQLite (`SQLITE_LIMIT_LENGTH`),
/// and the most bytes one `query` returns.
const BYTES_MAX: usize = 8 << 20;
/// The most rows one `query` returns.
pub const ROWS_MAX: usize = 10_000;
/// The most statements in one `transaction`.
const STATEMENTS_MAX: usize = 1_000;
/// How long a call waits for another process's lock, or another request's statement on the
/// shared connection, before `busy`; the run's deadline cuts it.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
/// How often a call waiting for the connection looks again.
const LOCK_POLL: Duration = Duration::from_micros(100);
/// Virtual-machine steps between checks of the run's deadline.
const PROGRESS_OPS: i32 = 1_000;
/// The most memory SQLite may hold in this process, all connections together: temp tables and
/// sorts live there (`temp_store=MEMORY`), so a runaway statement fails with `too-large`.
const HEAP_MAX: i64 = 256 << 20;
/// The largest database, in MB, without `--db-mb`: the `--kv-mb` default.
pub const DB_MB: u64 = 64;
/// Bytes a host string function scans between checks of the run's deadline.
const SCAN_CHECK: usize = 64 << 10;

/// The pragmas a program may run; everything else is `denied`. `SCHEMA_PRAGMAS` describe a
/// table or index named in their argument; `READ_PRAGMAS` are read only without a value (FTS5
/// reads `data_version` and R*Tree `page_size` themselves).
const SCHEMA_PRAGMAS: [&str; 5] = [
    "table_info",
    "table_xinfo",
    "index_list",
    "index_info",
    "foreign_key_list",
];
const READ_PRAGMAS: [&str; 3] = ["foreign_keys", "data_version", "page_size"];

/// Functions refused whatever their arguments: `load_extension` runs code; `fts3_tokenizer`
/// gives out (and, if ever enabled, takes) a raw pointer.
const DENIED_FUNCTIONS: [&str; 2] = ["load_extension", "fts3_tokenizer"];

/// One granted database. `host` is set while the connection runs the host's own statements
/// (`BEGIN`, `COMMIT`, `ROLLBACK`), which the authorizer refuses to the program.
pub struct Db {
    conn: Mutex<Connection>,
    /// The path as granted, or `:memory:`.
    target: String,
    pub read_only: bool,
    host: Arc<AtomicBool>,
    /// Device and inode of the file, to refuse a second grant of it.
    id: Option<(u64, u64)>,
    /// The current call's deadline, for the host string functions.
    deadline: Deadline,
}

/// The path of a `--db` spec and whether it ends in `:ro`.
pub fn split_spec(spec: &str) -> (&str, bool) {
    crate::kv::split_spec(spec)
}

/// The first write (`exec`, `transaction`) an authority report's `main` line shows on `name`.
pub fn write_in_report(report: &str, name: &str) -> Option<&'static str> {
    let calls = crate::main_calls(report);
    ["exec", "transaction"]
        .into_iter()
        .find(|m| calls.contains(&(name.to_string(), m.to_string())))
}

/// Open every `(name, spec)` grant, each at most `mb` MB, or say which one cannot be and why.
pub fn open_all(specs: &[(String, String)], mb: u64) -> std::result::Result<Vec<(String, Arc<Db>)>, String> {
    let mut out: Vec<(String, Arc<Db>)> = Vec::new();
    for (name, spec) in specs {
        let flag = format!("--db {name}={spec}");
        if out.iter().any(|(n, _)| n == name) {
            return Err(format!(
                "{flag}: `{name}` is granted twice\n  hint: give each database one name"
            ));
        }
        let (path, ro) = split_spec(spec);
        if path == ":memory:" && ro {
            return Err(format!(
                "{flag}: an in-memory database is never read-only\n  hint: drop `:ro`; only this process can write it"
            ));
        }
        let db = match spec.as_str() {
            ":memory:" => Db::memory(mb),
            _ => Db::open(Path::new(path), ro, mb),
        };
        let db = db.map_err(|e| format!("{flag}: {e}"))?;
        if let Some((other, _)) = out.iter().find(|(_, d)| db.id.is_some() && d.id == db.id) {
            return Err(format!(
                "{flag}: the same file as --db {other}\n  hint: grant one database once, or give each grant its own file"
            ));
        }
        out.push((name.clone(), Arc::new(db)));
    }
    Ok(out)
}

/// Once per process: cap SQLite's heap. SQLite keeps memory statistics (the bundled build does
/// not turn them off), which the limit needs.
fn heap_limit() {
    static ONCE: Once = Once::new();
    // SAFETY: a plain C call with no pointers.
    ONCE.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_hard_heap_limit64(HEAP_MAX);
    });
}

impl Db {
    /// A database lost when the process exits, of at most `mb` MB.
    pub fn memory(mb: u64) -> std::result::Result<Self, String> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_in_memory_with_flags(flags).map_err(|e| e.to_string())?;
        Db::fence(conn, ":memory:".into(), false, None, mb)
    }

    /// Open (or create) the SQLite file at `path`, to grow to at most `mb` MB; read-only with
    /// `ro`, which needs the file to exist. Never through a symlink. Errors name the file and say
    /// what to do.
    pub fn open(path: &Path, ro: bool, mb: u64) -> std::result::Result<Self, String> {
        let shown = path.display();
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{shown} is a symbolic link\n  hint: grant the file it points to"
            ));
        }
        let mut flags = OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        if ro {
            if !path.exists() {
                return Err(format!(
                    "{shown} does not exist\n  hint: a read-only database is written by another kio: run it once first, or drop `:ro`"
                ));
            }
            flags |= OpenFlags::SQLITE_OPEN_READ_ONLY;
        } else {
            flags |= OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        }
        // The path with its directories' symlinks resolved, so `NOFOLLOW` sees none (macOS's
        // `/var` is one); absolute, so SQLite reads none of it as a `file:` URI.
        let real = crate::resolve(path).ok_or_else(|| {
            format!(
                "{shown} has `.` or `..` past the directories that exist\n  hint: name the database by a plain path"
            )
        })?;
        let conn = Connection::open_with_flags(&real, flags).map_err(|e| format!("{shown}: {}", message(&e)))?;
        let id = std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
        Db::fence(conn, shown.to_string(), ro, id, mb).map_err(|e| format!("{shown}: {e}"))
    }

    /// Harden `conn` and install the authorizer: the one place every connection is set up.
    fn fence(
        conn: Connection,
        target: String,
        read_only: bool,
        id: Option<(u64, u64)>,
        mb: u64,
    ) -> std::result::Result<Self, String> {
        heap_limit();
        let mut conn = conn;
        let set = |conn: &Connection| -> rusqlite::Result<()> {
            for (c, on) in [
                (DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
                (DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
                (DbConfig::SQLITE_DBCONFIG_DQS_DML, false),
                (DbConfig::SQLITE_DBCONFIG_DQS_DDL, false),
                (DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_CREATE, false),
                (DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_WRITE, false),
            ] {
                conn.set_db_config(c, on)?;
            }
            for (l, n) in [
                (Limit::SQLITE_LIMIT_LENGTH, BYTES_MAX as i32),
                (Limit::SQLITE_LIMIT_SQL_LENGTH, SQL_MAX),
                (Limit::SQLITE_LIMIT_COLUMN, 1_000),
                (Limit::SQLITE_LIMIT_EXPR_DEPTH, 256),
                (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 50),
                (Limit::SQLITE_LIMIT_VDBE_OP, 1_000_000),
                (Limit::SQLITE_LIMIT_FUNCTION_ARG, 32),
                (Limit::SQLITE_LIMIT_ATTACHED, 0),
                (Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, 1_024),
                (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 4_096),
                (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 32),
                (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
            ] {
                conn.set_limit(l, n)?;
            }
            // Before the authorizer, which refuses every PRAGMA with a value.
            conn.execute_batch("PRAGMA temp_store = MEMORY; PRAGMA foreign_keys = ON;")?;
            if !read_only && target != ":memory:" {
                conn.execute_batch("PRAGMA journal_mode = WAL;")?;
            }
            // SQLite keeps the larger of this and the pages the file already has.
            let page: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            let pages = (mb.saturating_mul(1 << 20) / page.max(512) as u64).clamp(1, 0xFFFF_FFFE);
            conn.execute_batch(&format!("PRAGMA max_page_count = {pages};"))?;
            Ok(())
        };
        set(&conn).map_err(|e| message(&e))?;
        let deadline = Arc::new(Mutex::new(Instant::now()));
        register_scans(&conn, &deadline).map_err(|e| message(&e))?;
        let host = Arc::new(AtomicBool::new(false));
        let flag = host.clone();
        conn.authorizer(Some(move |ctx: AuthContext| {
            authorize(&ctx, flag.load(Relaxed), read_only)
        }))
        .map_err(|e| message(&e))?;
        Ok(Db {
            conn: Mutex::new(conn),
            target,
            read_only,
            host,
            id,
            deadline,
        })
    }

    /// The grant as the audit header shows it: the file, with `:ro` if read-only, or `:memory:`.
    pub fn label(&self) -> String {
        match self.read_only {
            true => format!("{}:ro", self.target),
            false => self.target.clone(),
        }
    }

    /// The path, or `:memory:`.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The connection, or `None` once another call has kept it past `until`. A statement runs
    /// until its own run's deadline, which may be far later than this one's, so a wait is
    /// bounded: the other requests of a server get `busy` instead of losing their whole deadline,
    /// and no worker lingers on the lock.
    fn lock(&self, until: Instant) -> Option<MutexGuard<'_, Connection>> {
        loop {
            match self.conn.try_lock() {
                Ok(conn) => return Some(conn),
                Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() >= until => return None,
                Err(TryLockError::WouldBlock) => std::thread::sleep(LOCK_POLL),
            }
        }
    }

    /// Run `sqls` with `params` (one list per statement) under the connection's lock: one
    /// statement for `query` and `exec`, all of them in one transaction otherwise. Waits for
    /// the connection, then for another process's lock, at most `BUSY_TIMEOUT` each, or until
    /// `deadline`; a statement still running at `deadline` is interrupted.
    fn run(
        &self,
        mode: Mode,
        sqls: &[String],
        params: &[Vec<SqlValue>],
        deadline: Instant,
    ) -> std::result::Result<Out, Fail> {
        let until = (Instant::now() + BUSY_TIMEOUT).min(deadline);
        let Some(mut conn) = self.lock(until) else {
            return Err(Fail::Db(DbErr::Busy));
        };
        conn.progress_handler(PROGRESS_OPS, Some(move || Instant::now() >= deadline))?;
        *self.deadline.lock().unwrap_or_else(|p| p.into_inner()) = deadline;
        let left = deadline.saturating_duration_since(Instant::now());
        conn.busy_timeout(left.min(BUSY_TIMEOUT))?;
        // The host's own statements, which the authorizer allows while `host` is set: reset on a
        // panic too, or every later statement would run unfenced.
        let trusted = |sql: &str| {
            struct Reset<'a>(&'a AtomicBool);
            impl Drop for Reset<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Relaxed);
                }
            }
            self.host.store(true, Relaxed);
            let _reset = Reset(&self.host);
            conn.execute_batch(sql)
        };
        // Only a panic inside an earlier `transaction` leaves one open: end it, or every later
        // write would land in it and `BEGIN` would fail.
        if !conn.is_autocommit() {
            trusted("ROLLBACK")?;
        }
        if mode != Mode::Transaction {
            return statement(&conn, &sqls[0], &params[0], mode == Mode::Query);
        }
        if sqls.len() != params.len() {
            return Err(invalid(format!(
                "{} statements but {} parameter lists: give each statement its own list",
                sqls.len(),
                params.len()
            )));
        }
        if sqls.len() > STATEMENTS_MAX {
            return Err(Fail::Db(DbErr::TooLarge));
        }
        trusted("BEGIN")?;
        let run = || -> std::result::Result<i64, Fail> {
            let mut changes = 0;
            for (sql, p) in sqls.iter().zip(params) {
                changes += statement(&conn, sql, p, false)?.changes;
            }
            trusted("COMMIT")?;
            Ok(changes)
        };
        match run() {
            Ok(changes) => Ok(Out {
                rows: vec![],
                changes,
                bytes: 0,
            }),
            Err(e) => {
                // An interrupted write has already rolled the transaction back.
                if !conn.is_autocommit() {
                    let _ = trusted("ROLLBACK");
                }
                Err(e)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Query,
    Exec,
    Transaction,
}

/// What a call produced: the rows (`query` only), the rows changed and the bytes of the rows.
#[derive(Default)]
struct Out {
    rows: Vec<Vec<SqlValue>>,
    changes: i64,
    bytes: usize,
}

/// A refused or failed statement: the error to give the program, or SQLite's, classified later
/// (an interrupt is the run's deadline, which stops the run).
enum Fail {
    Db(DbErr),
    Sqlite(rusqlite::Error),
}

impl From<rusqlite::Error> for Fail {
    fn from(e: rusqlite::Error) -> Self {
        Fail::Sqlite(e)
    }
}

fn invalid(msg: String) -> Fail {
    Fail::Db(DbErr::Invalid(msg))
}

/// Whether the program may do `ctx`'s action: the host's own statements always; the program's
/// never an ATTACH, DETACH, transaction or savepoint, a PRAGMA outside `SCHEMA_PRAGMAS` and
/// `READ_PRAGMAS`, or a `DENIED_FUNCTIONS` call; and on a read-only database no write.
fn authorize(ctx: &AuthContext, host: bool, read_only: bool) -> Authorization {
    use AuthAction::*;
    if host {
        return Authorization::Allow;
    }
    let allowed = match ctx.action {
        Attach { .. } | Detach { .. } | Transaction { .. } | Savepoint { .. } | Unknown { .. } => false,
        Pragma {
            pragma_name,
            pragma_value,
        } => {
            let named = |list: &[&str]| list.iter().any(|p| p.eq_ignore_ascii_case(pragma_name));
            named(&SCHEMA_PRAGMAS) || (named(&READ_PRAGMAS) && pragma_value.is_none())
        }
        Function { function_name } => !DENIED_FUNCTIONS.iter().any(|f| f.eq_ignore_ascii_case(function_name)),
        Read { .. } | Select | Recursive => true,
        // Everything else writes the database or its schema.
        _ => !read_only,
    };
    if allowed {
        Authorization::Allow
    } else {
        Authorization::Deny
    }
}

/// Prepare `sql`, bind `params` and step it to the end: its rows if `want_rows`, else an error
/// at the first row. Rows past `ROWS_MAX` or `BYTES_MAX` are `too-large`.
fn statement(conn: &Connection, sql: &str, params: &[SqlValue], want_rows: bool) -> std::result::Result<Out, Fail> {
    if sql.contains('\0') {
        return Err(invalid("the SQL holds a NUL byte".into()));
    }
    let mut stmt = conn.prepare(sql)?;
    let n = stmt.parameter_count();
    if n != params.len() {
        return Err(invalid(format!(
            "the statement takes {n} parameters, {} were given",
            params.len()
        )));
    }
    let cols = stmt.column_count();
    // A blank or comment-only `sql` prepares to no statement at all.
    if n == 0 && cols == 0 && stmt.expanded_sql().is_none() {
        return Err(invalid("no statement".into()));
    }
    // Before anything runs, so an `INSERT .. RETURNING` under `exec` inserts nothing.
    if cols > 0 && !want_rows {
        return Err(invalid("the statement returns rows: run it with `query`".into()));
    }
    for (i, p) in params.iter().enumerate() {
        match p {
            SqlValue::Null => stmt.raw_bind_parameter(i + 1, rusqlite::types::Null)?,
            SqlValue::Int(n) => stmt.raw_bind_parameter(i + 1, n)?,
            SqlValue::Float(x) => stmt.raw_bind_parameter(i + 1, x)?,
            SqlValue::Text(s) => stmt.raw_bind_parameter(i + 1, s.as_str())?,
            SqlValue::Blob(b) => stmt.raw_bind_parameter(i + 1, b.as_slice())?,
        }
    }
    let before = conn.total_changes();
    let mut out = Out::default();
    let mut rows = stmt.raw_query();
    while let Some(row) = rows.next()? {
        if out.rows.len() == ROWS_MAX {
            return Err(Fail::Db(DbErr::TooLarge));
        }
        let mut r = Vec::with_capacity(cols);
        for i in 0..cols {
            let v = match row.get_ref(i)? {
                ValueRef::Null => SqlValue::Null,
                ValueRef::Integer(n) => SqlValue::Int(n),
                ValueRef::Real(x) => SqlValue::Float(x),
                // Text another writer stored that is not UTF-8 crosses lossily: a WIT string is UTF-8.
                ValueRef::Text(b) => SqlValue::Text(String::from_utf8_lossy(b).into_owned()),
                ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
            };
            out.bytes += size(&v);
            r.push(v);
        }
        if out.bytes > BYTES_MAX {
            return Err(Fail::Db(DbErr::TooLarge));
        }
        out.rows.push(r);
    }
    drop(rows);
    // SQLite's `changes()`: the rows this statement changed directly. It is stale after a
    // statement that changed nothing (a `CREATE TABLE` after an `INSERT`), which the total tells.
    if conn.total_changes() > before {
        out.changes = conn.changes() as i64;
    }
    Ok(out)
}

/// The bytes a value is charged as: 8 for a scalar, its length for text or a blob.
fn size(v: &SqlValue) -> usize {
    match v {
        SqlValue::Text(s) => s.len(),
        SqlValue::Blob(b) => b.len(),
        _ => 8,
    }
}

/// SQLite's message for `e`.
fn message(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(_, Some(m)) => m.clone(),
        rusqlite::Error::SqlInputError { msg, offset, .. } if *offset >= 0 => format!("{msg} at byte {offset}"),
        rusqlite::Error::SqlInputError { msg, .. } => msg.clone(),
        rusqlite::Error::MultipleStatement => "one statement per call".into(),
        e => e.to_string(),
    }
}

/// The run's deadline passed while a statement ran.
struct Interrupted;

/// `e` as the program sees it, or `Interrupted`.
fn classify(e: Fail) -> std::result::Result<DbErr, Interrupted> {
    let e = match e {
        Fail::Db(e) => return Ok(e),
        Fail::Sqlite(e) => e,
    };
    let code = match &e {
        rusqlite::Error::SqliteFailure(f, _) => Some(f.code),
        rusqlite::Error::SqlInputError { error, .. } => Some(error.code),
        _ => None,
    };
    Ok(match code {
        Some(ErrorCode::OperationInterrupted) => return Err(Interrupted),
        Some(ErrorCode::ConstraintViolation) => DbErr::Constraint(message(&e)),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => DbErr::Busy,
        Some(ErrorCode::AuthorizationForStatementDenied | ErrorCode::ReadOnly) => DbErr::Denied,
        // `DiskFull` is `max_page_count` (`--db-mb`) as well as a full disk.
        Some(ErrorCode::TooBig | ErrorCode::OutOfMemory | ErrorCode::DiskFull) => DbErr::TooLarge,
        // A function the authorizer refused is a plain error, "not authorized to use function".
        _ if message(&e).starts_with("not authorized") => DbErr::Denied,
        _ => DbErr::Invalid(message(&e)),
    })
}

/// `ok`, or the error's name: never its message, which could be long.
fn db_outcome<T>(r: &std::result::Result<T, DbErr>) -> String {
    match r {
        Ok(_) => "ok",
        Err(DbErr::Invalid(_)) => "invalid",
        Err(DbErr::Denied) => "denied",
        Err(DbErr::Busy) => "busy",
        Err(DbErr::TooLarge) => "too-large",
        Err(DbErr::Constraint(_)) => "constraint",
    }
    .into()
}

impl crate::kio::caps::db::Host for State {}

impl HostDb for State {
    fn query(
        &mut self,
        h: Resource<Handle>,
        sql: String,
        params: Vec<SqlValue>,
    ) -> Result<Result<Vec<Vec<SqlValue>>, DbErr>> {
        let arg = sql.clone();
        self.call(
            h.rep(),
            "query",
            &arg,
            |s| {
                let r = s.db_run(&h, Mode::Query, vec![sql], vec![params])?;
                Ok(r.map(|o| o.rows))
            },
            db_outcome,
        )
    }

    fn exec(&mut self, h: Resource<Handle>, sql: String, params: Vec<SqlValue>) -> Result<Result<i64, DbErr>> {
        let arg = sql.clone();
        self.call(
            h.rep(),
            "exec",
            &arg,
            |s| {
                s.writable_db(&h, "exec")?;
                let r = s.db_run(&h, Mode::Exec, vec![sql], vec![params])?;
                Ok(r.map(|o| o.changes))
            },
            db_outcome,
        )
    }

    fn transaction(
        &mut self,
        h: Resource<Handle>,
        sqls: Vec<String>,
        params: Vec<Vec<SqlValue>>,
    ) -> Result<Result<i64, DbErr>> {
        let arg = sqls.join("; ");
        self.call(
            h.rep(),
            "transaction",
            &arg,
            |s| {
                s.writable_db(&h, "transaction")?;
                s.audit_extra = format!(r#","statements":{}"#, sqls.len());
                let r = s.db_run(&h, Mode::Transaction, sqls, params)?;
                Ok(r.map(|o| o.changes))
            },
            db_outcome,
        )
    }

    fn drop(&mut self, h: Resource<Handle>) -> Result<()> {
        let _ = self.table.delete(Resource::<Arc<Db>>::new_own(h.rep()));
        Ok(())
    }
}

impl State {
    /// Stop the run if the database behind `h` is read-only: the startup check refuses a program
    /// whose report shows `exec` or `transaction` on it, so this is defence in depth (the
    /// authorizer and the read-only open would refuse the write too).
    fn writable_db(&mut self, h: &Resource<Handle>, method: &str) -> Result<()> {
        if !self.table.get(&Resource::<Arc<Db>>::new_borrow(h.rep()))?.read_only {
            return Ok(());
        }
        let cap = self.handles.get(&h.rep()).map_or("?", |h| h.0.as_str());
        self.read_only_write = true;
        self.audit_msg = format!("db: `{cap}` is granted read-only, so `{cap} {method}` stops the run");
        Err(wasmtime::Error::msg(self.audit_msg.clone()))
    }

    /// Run `sqls` with `params` on the database behind `h`, on a worker within the run's
    /// deadline (H5): parameter bytes are charged to `--io` first, result bytes after; the
    /// parameter count and the rows or rows changed go in the audit line. A statement the
    /// deadline interrupts stops the run as a timeout.
    fn db_run(
        &mut self,
        h: &Resource<Handle>,
        mode: Mode,
        sqls: Vec<String>,
        params: Vec<Vec<SqlValue>>,
    ) -> Result<Result<Out, DbErr>> {
        let db = self.table.get(&Resource::<Arc<Db>>::new_borrow(h.rep()))?.clone();
        let count: usize = params.iter().map(Vec::len).sum();
        self.audit_extra += &format!(r#","params":{count}"#);
        self.spend_io(params.iter().flatten().map(size).sum())?;
        let deadline = self.deadline;
        let r = self.within(move || db.run(mode, &sqls, &params, deadline))?;
        match r {
            Ok(out) => {
                self.spend_io(out.bytes)?;
                self.audit_extra += &match mode {
                    Mode::Query => format!(r#","rows":{}"#, out.rows.len()),
                    _ => format!(r#","changes":{}"#, out.changes),
                };
                Ok(Ok(out))
            }
            Err(e) => match classify(e) {
                Ok(e) => Ok(Err(e)),
                Err(Interrupted) => {
                    self.timed_out = true;
                    Err(wasmtime::Error::msg("host call timed out"))
                }
            },
        }
    }
}

/// The current call's deadline, shared by a connection's string functions.
pub type Deadline = Arc<Mutex<Instant>>;

/// The flags of every host string function: like the built-ins, deterministic and usable in
/// views and triggers of an untrusted schema.
const SCAN_FLAGS: c_int = ffi::SQLITE_UTF8 | ffi::SQLITE_DETERMINISTIC | ffi::SQLITE_INNOCUOUS;

/// Replace SQLite's `instr`, `replace`, `like` (two and three arguments) and `glob` on `conn`
/// with ports of them (SQLite 3.54.1's `func.c`) that check `deadline` every `SCAN_CHECK`
/// bytes scanned: the built-ins scan inside one VDBE step, where the progress handler never
/// runs, so `instr` over megabytes held the connection for minutes. Registered through the C
/// API rather than `create_scalar_function`, whose arguments arrive as Rust values: the
/// functions call the same `sqlite3_value_*` the built-ins call, so a number or blob becomes
/// text exactly as there (SQLite's own float rendering, the database's encoding). Public for
/// the corpus test that compares them with the built-ins.
pub fn register_scans(conn: &Connection, deadline: &Deadline) -> rusqlite::Result<()> {
    let functions: [(&CStr, c_int, Kind); 5] = [
        (c"instr", 2, Kind::Instr),
        (c"replace", 3, Kind::Replace),
        (c"like", 2, Kind::Like(LIKE)),
        (c"like", 3, Kind::Like(LIKE)),
        (c"glob", 2, Kind::Like(GLOB)),
    ];
    for (name, args, kind) in functions {
        let scan = Box::into_raw(Box::new(Scan {
            deadline: deadline.clone(),
            kind,
        }));
        // SAFETY: `conn` is open; SQLite owns `scan` from here and frees it with `drop_scan`
        // when the function is replaced, the connection closes, or this call fails.
        let rc = unsafe {
            ffi::sqlite3_create_function_v2(
                conn.handle(),
                name.as_ptr(),
                args,
                SCAN_FLAGS,
                scan.cast(),
                Some(scan_call),
                None,
                None,
                Some(drop_scan),
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(rusqlite::Error::SqliteFailure(ffi::Error::new(rc), None));
        }
    }
    Ok(())
}

/// A host string function's user data.
struct Scan {
    deadline: Deadline,
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    Instr,
    Replace,
    Like(Wildcards),
}

/// SQLite's `compareInfo`: the any-sequence, any-character and set characters (0 for none), and
/// whether ASCII letters match either case.
#[derive(Clone, Copy)]
struct Wildcards {
    all: u32,
    one: u32,
    set: u32,
    no_case: bool,
}

/// `LIKE` as SQLite has it by default (`PRAGMA case_sensitive_like` is denied).
const LIKE: Wildcards = Wildcards {
    all: b'%' as u32,
    one: b'_' as u32,
    set: 0,
    no_case: true,
};
const GLOB: Wildcards = Wildcards {
    all: b'*' as u32,
    one: b'?' as u32,
    set: b'[' as u32,
    no_case: false,
};

/// The run's deadline passed inside a string function.
struct Stop;

/// Counts the bytes a string function scans; every `SCAN_CHECK` it looks at the deadline (only
/// then, so a call on short strings costs no lock or clock read).
struct Budget<'a> {
    deadline: &'a Deadline,
    scanned: usize,
}

impl Budget<'_> {
    fn tick(&mut self, n: usize) -> std::result::Result<(), Stop> {
        self.scanned += n;
        if self.scanned >= SCAN_CHECK {
            self.scanned = 0;
            if Instant::now() >= *self.deadline.lock().unwrap_or_else(|p| p.into_inner()) {
                return Err(Stop);
            }
        }
        Ok(())
    }
}

unsafe extern "C" fn drop_scan(p: *mut c_void) {
    // SAFETY: `p` is the `Box<Scan>` `register_scans` gave SQLite, freed once.
    drop(unsafe { Box::from_raw(p.cast::<Scan>()) });
}

/// The entry point of every host string function: past the deadline the statement fails as
/// interrupted, which stops the run as the progress handler does.
unsafe extern "C" fn scan_call(ctx: *mut ffi::sqlite3_context, argc: c_int, argv: *mut *mut ffi::sqlite3_value) {
    let call = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: SQLite passes the user data `register_scans` set and `argc` live arguments.
        let (scan, args) = unsafe {
            (
                &*ffi::sqlite3_user_data(ctx).cast::<Scan>(),
                std::slice::from_raw_parts(argv, argc as usize),
            )
        };
        let mut budget = Budget {
            deadline: &scan.deadline,
            scanned: 0,
        };
        // SAFETY: `ctx` and `args` are this call's.
        let r = unsafe {
            match scan.kind {
                Kind::Instr => instr_call(ctx, args, &mut budget),
                Kind::Replace => replace_call(ctx, args, &mut budget),
                Kind::Like(w) => like_call(ctx, args, w, &mut budget),
            }
        };
        if r.is_err() {
            // SAFETY: `ctx` is this call's.
            unsafe { ffi::sqlite3_result_error_code(ctx, ffi::SQLITE_INTERRUPT) };
        }
    }));
    if call.is_err() {
        // SAFETY: `ctx` is this call's.
        unsafe { ffi::sqlite3_result_error(ctx, c"host string function failed".as_ptr(), -1) };
    }
}

/// `v` as text (SQLite converts a number and reads a blob's bytes in place), or `None` for NULL
/// or out of memory. Valid until `v` is next converted or the call returns.
unsafe fn text<'a>(v: *mut ffi::sqlite3_value) -> Option<&'a [u8]> {
    // SAFETY: `v` is a live argument; SQLite returns its text and that text's length.
    unsafe {
        let p = ffi::sqlite3_value_text(v);
        (!p.is_null()).then(|| std::slice::from_raw_parts(p, ffi::sqlite3_value_bytes(v) as usize))
    }
}

/// `v`'s bytes as a blob, or `None` out of memory (SQLite gives no pointer for an empty blob).
unsafe fn blob<'a>(v: *mut ffi::sqlite3_value) -> Option<&'a [u8]> {
    // SAFETY: as `text`.
    unsafe {
        let p = ffi::sqlite3_value_blob(v).cast::<u8>();
        let n = ffi::sqlite3_value_bytes(v) as usize;
        match (p.is_null(), n) {
            (_, 0) => Some(&[]),
            (true, _) => None,
            (false, n) => Some(std::slice::from_raw_parts(p, n)),
        }
    }
}

/// `sqlite3_limit` of the connection running `ctx`.
unsafe fn limit(ctx: *mut ffi::sqlite3_context, id: c_int) -> usize {
    // SAFETY: `ctx` is a live call's.
    unsafe { ffi::sqlite3_limit(ffi::sqlite3_context_db_handle(ctx), id, -1) as usize }
}

/// `instrFunc`: NULL if either is NULL; 1 for an empty needle; else 1 + the characters (bytes,
/// if both are blobs) before the first occurrence, or 0. A blob beside text is read as text.
unsafe fn instr_call(
    ctx: *mut ffi::sqlite3_context,
    args: &[*mut ffi::sqlite3_value],
    budget: &mut Budget,
) -> std::result::Result<(), Stop> {
    use ffi::{SQLITE_BLOB as BLOB, SQLITE_NULL as NULL};
    // SAFETY: `ctx` and `args` are a live call's; the copies are freed before returning.
    unsafe {
        let (th, tn) = (ffi::sqlite3_value_type(args[0]), ffi::sqlite3_value_type(args[1]));
        if th == NULL || tn == NULL {
            return Ok(());
        }
        ffi::sqlite3_value_bytes(args[0]);
        if ffi::sqlite3_value_bytes(args[1]) == 0 {
            ffi::sqlite3_result_int64(ctx, 1);
            return Ok(());
        }
        let mut copies = [std::ptr::null_mut(); 2];
        let found = if th == BLOB && tn == BLOB {
            blob(args[0]).zip(blob(args[1])).map(|(h, n)| (h, n, false))
        } else if th != BLOB && tn != BLOB {
            text(args[0]).zip(text(args[1])).map(|(h, n)| (h, n, true))
        } else {
            // As SQLite: text copies, so the arguments keep their type.
            copies = [ffi::sqlite3_value_dup(args[0]), ffi::sqlite3_value_dup(args[1])];
            text(copies[0]).zip(text(copies[1])).map(|(h, n)| (h, n, true))
        };
        let r = match found {
            Some((h, n, is_text)) => instr(h, n, is_text, budget).map(|i| ffi::sqlite3_result_int64(ctx, i)),
            None => {
                ffi::sqlite3_result_error_nomem(ctx);
                Ok(())
            }
        };
        for c in copies {
            ffi::sqlite3_value_free(c);
        }
        r
    }
}

/// The position of `needle` (not empty) in `hay`, counting characters if `is_text`, else bytes.
/// SQLite tries byte 0 and then, in text, each byte that is not a UTF-8 continuation byte; this
/// jumps between the tries whose first byte matches and counts the characters passed.
fn instr(hay: &[u8], needle: &[u8], is_text: bool, budget: &mut Budget) -> std::result::Result<i64, Stop> {
    let Some(last) = hay.len().checked_sub(needle.len()) else {
        return Ok(0);
    };
    let tried = |b: u8| !is_text || b & 0xc0 != 0x80;
    let (mut i, mut n) = (0, 1);
    loop {
        if hay[i] == needle[0] {
            budget.tick(needle.len())?;
            if hay[i..i + needle.len()] == *needle {
                return Ok(n);
            }
        }
        let Some(k) = hay[i + 1..=last].iter().position(|&b| b == needle[0] && tried(b)) else {
            return Ok(0);
        };
        let j = i + 1 + k;
        budget.tick(j - i)?;
        n += hay[i + 1..=j].iter().filter(|&&b| tried(b)).count() as i64;
        i = j;
    }
}

/// `replaceFunc`: NULL if the string or pattern is NULL; the string (as text) if the pattern's
/// text is empty or starts with a NUL byte; NULL if the replacement is NULL; else every
/// occurrence replaced, left to right, as text; `too-large` past `SQLITE_LIMIT_LENGTH`.
unsafe fn replace_call(
    ctx: *mut ffi::sqlite3_context,
    args: &[*mut ffi::sqlite3_value],
    budget: &mut Budget,
) -> std::result::Result<(), Stop> {
    // SAFETY: `ctx` and `args` are a live call's; results are copied (`SQLITE_TRANSIENT`).
    unsafe {
        let Some(s) = text(args[0]) else { return Ok(()) };
        let Some(pattern) = text(args[1]) else { return Ok(()) };
        let result = |out: &[u8]| {
            let p = out.as_ptr().cast();
            ffi::sqlite3_result_text64(
                ctx,
                p,
                out.len() as u64,
                ffi::SQLITE_TRANSIENT(),
                ffi::SQLITE_UTF8 as u8,
            )
        };
        if at(pattern, 0) == 0 {
            result(s);
            return Ok(());
        }
        let Some(rep) = text(args[2]) else { return Ok(()) };
        match replace(s, pattern, rep, limit(ctx, ffi::SQLITE_LIMIT_LENGTH), budget)? {
            Some(out) => result(&out),
            None => ffi::sqlite3_result_error_toobig(ctx),
        }
        Ok(())
    }
}

/// `s` with each `pattern` (not empty) replaced by `rep`, or `None` once longer than `max`.
fn replace(
    s: &[u8],
    pattern: &[u8],
    rep: &[u8],
    max: usize,
    budget: &mut Budget,
) -> std::result::Result<Option<Vec<u8>>, Stop> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i + pattern.len() <= s.len() {
        // The next byte that could start an occurrence; the bytes before it are copied.
        let Some(k) = s[i..=s.len() - pattern.len()].iter().position(|&b| b == pattern[0]) else {
            break;
        };
        out.extend_from_slice(&s[i..i + k]);
        i += k;
        budget.tick(k + pattern.len())?;
        if s[i..i + pattern.len()] == *pattern {
            out.extend_from_slice(rep);
            i += pattern.len();
            if rep.len() > pattern.len() && out.len() + s.len() - i > max {
                return Ok(None);
            }
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out.extend_from_slice(&s[i..]);
    Ok(Some(out))
}

/// `likeFunc`, for `like(pattern, s)`, `like(pattern, s, escape)` and `glob(pattern, s)`:
/// `X LIKE Y ESCAPE Z` is `like(Y, X, Z)`. A pattern over `SQLITE_LIMIT_LIKE_PATTERN_LENGTH`
/// bytes or an escape that is not one character is an error; a NULL escape, pattern or string
/// is NULL; else 1 or 0. Text ends at its first NUL byte, as in SQLite.
unsafe fn like_call(
    ctx: *mut ffi::sqlite3_context,
    args: &[*mut ffi::sqlite3_value],
    mut w: Wildcards,
    budget: &mut Budget,
) -> std::result::Result<(), Stop> {
    // SAFETY: `ctx` and `args` are a live call's.
    unsafe {
        let n = ffi::sqlite3_value_bytes(args[0]) as usize;
        if n > limit(ctx, ffi::SQLITE_LIMIT_LIKE_PATTERN_LENGTH) {
            ffi::sqlite3_result_error(ctx, c"LIKE or GLOB pattern too complex".as_ptr(), -1);
            return Ok(());
        }
        let escape = if args.len() == 3 {
            let Some(e) = text(args[2]) else { return Ok(()) };
            if char_len(e) != 1 {
                let msg = c"ESCAPE expression must be a single character";
                ffi::sqlite3_result_error(ctx, msg.as_ptr(), -1);
                return Ok(());
            }
            let e = utf8_read(e, &mut 0);
            if e == w.all {
                w.all = 0;
            }
            if e == w.one {
                w.one = 0;
            }
            e
        } else {
            w.set
        };
        if let (Some(pattern), Some(s)) = (text(args[0]), text(args[1])) {
            let m = compare(pattern, 0, s, 0, &w, escape, budget)?;
            ffi::sqlite3_result_int64(ctx, (m == Match::Yes) as i64);
        }
        Ok(())
    }
}

/// The byte at `i`, or 0 past the end: SQLite's strings end in a NUL byte.
fn at(z: &[u8], i: usize) -> u8 {
    z.get(i).copied().unwrap_or(0)
}

/// `sqlite3Utf8Trans1`: the payload bits of a lead byte `0xc0..`.
const UTF8_TRANS1: [u8; 64] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, //
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, //
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, //
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x00, 0x01, 0x02, 0x03, 0x00, 0x01, 0x00, 0x00, //
];

/// `sqlite3Utf8Read`: the character at `*i`, advancing past it; malformed UTF-8 reads as
/// SQLite reads it (an overlong form or a surrogate is U+FFFD, a stray continuation byte itself).
fn utf8_read(z: &[u8], i: &mut usize) -> u32 {
    let mut c = at(z, *i) as u32;
    *i += 1;
    if c >= 0xc0 {
        c = UTF8_TRANS1[(c - 0xc0) as usize] as u32;
        while at(z, *i) & 0xc0 == 0x80 {
            c = (c << 6) + (0x3f & at(z, *i)) as u32;
            *i += 1;
        }
        if c < 0x80 || c & 0xFFFF_F800 == 0xD800 {
            c = 0xFFFD;
        }
    }
    c
}

/// `SQLITE_SKIP_UTF8`.
fn skip_utf8(z: &[u8], i: &mut usize) {
    *i += 1;
    if at(z, *i - 1) >= 0xc0 {
        while at(z, *i) & 0xc0 == 0x80 {
            *i += 1;
        }
    }
}

/// `sqlite3Utf8CharLen(z, -1)`: the characters before the first NUL.
fn char_len(z: &[u8]) -> usize {
    let mut i = 0;
    let mut n = 0;
    while at(z, i) != 0 {
        skip_utf8(z, &mut i);
        n += 1;
    }
    n
}

/// `patternCompare`'s result: `NoWildcard` ends every enclosing search early.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Match {
    Yes,
    No,
    NoWildcard,
}

/// `patternCompare`, a line-for-line port: whether `s[si..]` matches `p[pi..]` under `w`, with
/// `other` the escape character (LIKE) or `[` (GLOB). Case folds ASCII only.
fn compare(
    p: &[u8],
    mut pi: usize,
    s: &[u8],
    mut si: usize,
    w: &Wildcards,
    other: u32,
    budget: &mut Budget,
) -> std::result::Result<Match, Stop> {
    let lower = |c: u32| (c as u8).to_ascii_lowercase() as u32;
    let upper = |c: u32| (c as u8).to_ascii_uppercase() as u32;
    let mut escaped = None;
    loop {
        budget.tick(1)?;
        let mut c = utf8_read(p, &mut pi);
        if c == 0 {
            break;
        }
        if c == w.all {
            // Skip further `*`s, and a string character for each `?` among them.
            loop {
                c = utf8_read(p, &mut pi);
                if c != w.all && (c != w.one || w.one == 0) {
                    break;
                }
                if c == w.one && utf8_read(s, &mut si) == 0 {
                    return Ok(Match::NoWildcard);
                }
            }
            if c == 0 {
                return Ok(Match::Yes);
            } else if c == other {
                if w.set == 0 {
                    c = utf8_read(p, &mut pi);
                    if c == 0 {
                        return Ok(Match::NoWildcard);
                    }
                } else {
                    // `[...]` right after `*`: try every position.
                    while at(s, si) != 0 {
                        budget.tick(1)?;
                        let m = compare(p, pi - 1, s, si, w, other, budget)?;
                        if m != Match::No {
                            return Ok(m);
                        }
                        skip_utf8(s, &mut si);
                    }
                    return Ok(Match::NoWildcard);
                }
            }
            // Find each place `c` occurs in `s` and match the rest of the pattern from there.
            if c < 0x80 {
                let (a, b) = if w.no_case { (upper(c), lower(c)) } else { (c, c) };
                let stop = |x: &u8| *x == 0 || *x as u32 == a || *x as u32 == b;
                loop {
                    // `strcspn`.
                    let k = s
                        .get(si..)
                        .map_or(0, |rest| rest.iter().position(stop).unwrap_or(rest.len()));
                    budget.tick(k)?;
                    si += k;
                    if at(s, si) == 0 {
                        break;
                    }
                    si += 1;
                    let m = compare(p, pi, s, si, w, other, budget)?;
                    if m != Match::No {
                        return Ok(m);
                    }
                }
            } else {
                loop {
                    budget.tick(1)?;
                    let c2 = utf8_read(s, &mut si);
                    if c2 == 0 {
                        break;
                    }
                    if c2 != c {
                        continue;
                    }
                    let m = compare(p, pi, s, si, w, other, budget)?;
                    if m != Match::No {
                        return Ok(m);
                    }
                }
            }
            return Ok(Match::NoWildcard);
        }
        if c == other {
            if w.set == 0 {
                c = utf8_read(p, &mut pi);
                if c == 0 {
                    return Ok(Match::No);
                }
                escaped = Some(pi);
            } else {
                // `[...]`, `[^...]`, with `]` first taken literally and `a-z` ranges.
                let mut prior = 0;
                let mut seen = false;
                let mut invert = false;
                c = utf8_read(s, &mut si);
                if c == 0 {
                    return Ok(Match::No);
                }
                let mut c2 = utf8_read(p, &mut pi);
                if c2 == b'^' as u32 {
                    invert = true;
                    c2 = utf8_read(p, &mut pi);
                }
                if c2 == b']' as u32 {
                    if c == b']' as u32 {
                        seen = true;
                    }
                    c2 = utf8_read(p, &mut pi);
                }
                while c2 != 0 && c2 != b']' as u32 {
                    budget.tick(1)?;
                    if c2 == b'-' as u32 && at(p, pi) != b']' && at(p, pi) != 0 && prior > 0 {
                        c2 = utf8_read(p, &mut pi);
                        if c >= prior && c <= c2 {
                            seen = true;
                        }
                        prior = 0;
                    } else {
                        if c == c2 {
                            seen = true;
                        }
                        prior = c2;
                    }
                    c2 = utf8_read(p, &mut pi);
                }
                if c2 == 0 || seen == invert {
                    return Ok(Match::No);
                }
                continue;
            }
        }
        let c2 = utf8_read(s, &mut si);
        if c == c2 {
            continue;
        }
        if w.no_case && c < 0x80 && c2 < 0x80 && lower(c) == lower(c2) {
            continue;
        }
        if c == w.one && escaped != Some(pi) && c2 != 0 {
            continue;
        }
        return Ok(Match::No);
    }
    Ok(if at(s, si) == 0 { Match::Yes } else { Match::No })
}
