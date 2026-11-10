//! The `Db` capability (H17): one SQLite database per grant (`--db name=path`, `path:ro` or
//! `:memory:`), shared by every run of the process behind a mutex. SQL text is the program's,
//! and SQLite runs on the host outside Wasm fuel, so the connection is fenced: an authorizer
//! refuses every statement that could reach another file, change the connection or run code
//! (ATTACH, DETACH, PRAGMA but a read-only few, transactions outside `transaction`,
//! `load_extension`); `SQLITE_DBCONFIG_DEFENSIVE` and a limit of zero attached databases close
//! the back doors (`VACUUM INTO`, `writable_schema`); SQLite's own limits are lowered; a progress
//! handler interrupts a statement once the run's deadline passes; a process-wide hard heap limit
//! bounds SQLite's memory, where its temp store lives.
use crate::kio::caps::db::{Db as Handle, DbErr, HostDb, SqlValue};
use crate::State;
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, OpenFlags};
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

/// Open every `(name, spec)` grant, or say which one cannot be and why.
pub fn open_all(specs: &[(String, String)]) -> std::result::Result<Vec<(String, Arc<Db>)>, String> {
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
            ":memory:" => Db::memory(),
            _ => Db::open(Path::new(path), ro),
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
    /// A database lost when the process exits.
    pub fn memory() -> std::result::Result<Self, String> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_in_memory_with_flags(flags).map_err(|e| e.to_string())?;
        Db::fence(conn, ":memory:".into(), false, None)
    }

    /// Open (or create) the SQLite file at `path`; read-only with `ro`, which needs the file to
    /// exist. Never through a symlink. Errors name the file and say what to do.
    pub fn open(path: &Path, ro: bool) -> std::result::Result<Self, String> {
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
        let real = crate::cache::resolve(path).ok_or_else(|| {
            format!(
                "{shown} has `.` or `..` past the directories that exist\n  hint: name the database by a plain path"
            )
        })?;
        let conn = Connection::open_with_flags(&real, flags).map_err(|e| format!("{shown}: {}", message(&e)))?;
        let id = std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
        Db::fence(conn, shown.to_string(), ro, id).map_err(|e| format!("{shown}: {e}"))
    }

    /// Harden `conn` and install the authorizer: the one place every connection is set up.
    fn fence(
        conn: Connection,
        target: String,
        read_only: bool,
        id: Option<(u64, u64)>,
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
            Ok(())
        };
        set(&conn).map_err(|e| message(&e))?;
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

    /// The connection, or `None` once another call has kept it past `until`. Some statements
    /// SQLite cannot interrupt (`instr` or `replace` over megabytes is one VDBE step that runs
    /// for minutes), so a wait is bounded: the other requests of a server get `busy` instead of
    /// losing their whole deadline, and no worker lingers on the lock.
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
        Some(ErrorCode::TooBig | ErrorCode::OutOfMemory) => DbErr::TooLarge,
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
