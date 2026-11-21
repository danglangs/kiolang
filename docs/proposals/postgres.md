# Postgres: an exploration (H42)

Proposal only, 2026-11-21 (roadmap "Order of work" 4). No code. Any caps.wit change and any new host dependency is the maintainer's; recommendation in section 5.

## 1. Who needs it, and what would show it

No A2 or A3 finding names Postgres. Both apps (an inventory service, a booking service with an outbox) were built on `Db` and shipped. The database evidence that does exist:

| Source | Finding | What it says about Postgres |
|---|---|---|
| A2 | 20,000-row CSV import in ~40 ms; `:ro` Db accepted for a nightly job; `DbErr::TooLarge` names no limit; the schema-at-each-request idiom was undocumented (#8) | SQLite's speed and `:ro` met the need; the gaps were documentation |
| A3 #5 | no migration guidance (F18); `RETURNING` through `query` (F4, fixed by H37); no rows from a transaction (F29, H37); two processes on one file (F39) | F39 is the nearest to a server database: a server and a job sharing one file, answered by WAL plus `:ro` |
| roadmap.md "Database" row | "Only SQLite: no server databases (Postgres, Redis); no typed row decode" | a known gap against sqlx and diesel, not a user's request |

What would count as evidence, in order of weight:

1. A target application (W1's F1 workflow service, an A4 outside-maintenance app, a kio-web example) whose data **already lives in Postgres** and is owned by someone else: the common production case, and the one SQLite cannot stand in for.
2. Two kio instances needing one writable store (behind a load balancer, or a server and a worker on two hosts): SQLite's file does not cross hosts.
3. Row-level security by role, or a dataset past one disk or `--db-mb`.
4. A friction entry of the form "built a Python sidecar to reach Postgres" or "copied the table into SQLite nightly".

Until one of these appears in a build log, Postgres is a feature list item, which the roadmap says not to build from.

## 2. The shape, if built

Everything below holds for options B and C of section 4 alike; it is what a Postgres grant means in kio.

```
main(orders: Pg)                  --pg orders=env:ORDERS_URL:ro
   │ orders.query("SELECT .. $1", [Text(id)])
   ▼ caps.wit (sql-value in, rows out; the DSN never crosses)
host  checkout conn ─► BEGIN READ ONLY; SET LOCAL statement_timeout=<left> ─► Parse/Bind/Execute ─► COMMIT ─► idle? ─► return
      │ at the epoch deadline: cancel request on a new socket; drop the conn if not idle
      ▼ audit: sql (256 chars), params, rows, bytes, sqlstate; never a value
Postgres  role shop_ro: GRANT SELECT .. (the operator's fence)
```

### Credentials

| Rule | Why |
|---|---|
| `--pg name=env:VAR` or `name=file:PATH`, optional `:ro`; the value is a DSN (`postgres://user:pw@host:5432/db?sslmode=verify-full`) | the same source forms and `read_secret` rules as `--secret`, `--hmac`, `--http-header`: read once at start, not inside a granted `--dir`, no other hard links, bounded |
| The DSN lives in a host `Secret` (Debug `<redacted>`); nothing in caps.wit returns it or any part of it | the program cannot read, log or send the credential, as with `--http-header` |
| `sslmode=verify-full` required unless the host is a loopback IP literal or a Unix socket | `--http-header`'s rule for `http://`: a credential never crosses a network in clear |
| Audit header and `kio check --grants` binds line show the source and the parsed non-secret parts: `orders=env:ORDERS_URL (db.internal:5432/shop as shop_ro, ro)` | a reviewer sees which database and role, never the password |
| Connects at startup; a DSN that does not connect is refused before the program runs (exit 2) | as a `--dir` path that does not exist |
| Rotation is a restart | as every secret today |

### Authority

| Facet | Conveyed by | Enforced by | Shown to a reviewer |
|---|---|---|---|
| Which database, as which role | the DSN | the server authenticates the role; the host connects to that one host:port and nothing else | binds line, audit header |
| Methods | `query`, `exec`, `exec_rows`, `transaction`, `transaction_rows`, as `Db` | the host, per call | report `main : orders query exec`; `kio check --base` `~ orders: Pg (+exec_rows)`; covers as #329 |
| `query` reads only | `BEGIN READ ONLY` per call; a write fails SQLSTATE 25006, mapped to `denied` with `Db`'s hint | the server, inside the host's transaction | `derived` from the report, `enforced` per call |
| `:ro` grant | refused at startup to a program whose report writes (as `Db :ro`); every transaction `READ ONLY`; `SET` statements refused | host and server | binds line `ro` |
| One statement per call | the extended query protocol refuses a second statement in a Parse | the server | `invalid` |
| Statement fence | the host runs a statement only when its first keyword (after comments) is one of `SELECT WITH VALUES TABLE INSERT UPDATE DELETE MERGE CREATE ALTER DROP TRUNCATE EXPLAIN`; `BEGIN COMMIT ROLLBACK SAVEPOINT SET RESET SHOW PREPARE EXECUTE LISTEN NOTIFY COPY LOAD DISCARD DO VACUUM` and anything else are `denied` | host, before the Parse | audit `denied` |
| After each call the connection must report idle (`ReadyForQuery` I); otherwise it is dropped, not pooled, and the call is `denied` | host | audit |
| Tables, schemas, columns, RLS | **the role's privileges**: kio does not parse Postgres SQL and cannot enforce a schema list | the operator (section 3) | the role name only |

The report does not tell DDL from DML (`exec` covers both); a serving role without `CREATE` is how an operator keeps migrations out of the server (Schema migrations, below).

### Transactions, pooling, deadlines

| Topic | Decision | Note |
|---|---|---|
| Every call is one server transaction the host owns | `query`: `BEGIN READ ONLY`; writes: `BEGIN`; `transaction`: one `BEGIN .. COMMIT` around all statements, `ROLLBACK` on the first error, all or nothing | as `Db`; no transaction open across calls (needs a session pinned to a run; concurrency.md's "state ownership first" applies) |
| Isolation | the server's default for the role (`READ COMMITTED` unless the operator sets `default_transaction_isolation`) | 40001 serialization failure, 40P01 deadlock, 55P03 lock timeout map to `busy` (retryable); the program retries, as it does `Db`'s |
| Pool | one pool per grant per process, `--pg-conns n` (default 8), opened at startup; a call checks a connection out and returns it; under `--listen` requests share it; waiting past 2 s (or the run's remainder) is `busy` | the fresh instance per request keeps no connection; session state (`CREATE TEMP TABLE`, `SET`) does not survive a call, which the fence already refuses; `Db`'s one connection per grant kept temp tables between calls, this does not |
| Wall-clock | per call `SET LOCAL statement_timeout = <ms left in the run>` and `SET LOCAL lock_timeout` to the same; at the epoch deadline the host sends a cancel request (the protocol's, a new socket with the backend key); a call that returns at or after the deadline stops the run as a timeout | three layers because each can fail alone: the server timer if the cancel packet is lost, the cancel if the server timer was reset, the host deadline always. `pg_cancel_backend` is the operator's tool, not the host's (it needs a privileged session) |
| Fuel | does not tick during a host call; the server's CPU is outside the sandbox, bounded by the deadline alone (as SQLite's, #153) | `--calls` counts each call; `--io` charges SQL text, parameter bytes and row bytes |
| Rows | 10,000 rows or 8 MB per result (`too-large`), read through a portal so the server stops early; a write whose rows pass the limit is rolled back | as `Db`'s limits (#283) |
| A run killed mid-call | the host's transaction never commits; the connection is cancelled and dropped | a `--listen` worker's death leaves nothing half-written |

### Parameters and types

Postgres numbers parameters `$1`, `$2`; the host does not rewrite `?` (rewriting is parsing, and the program's SQL is already dialect-bound). `sql-value` as in caps.wit; the host decodes by the server's declared type:

| kio | to the server | from the server |
|---|---|---|
| `Int(n)` | `int8` (the server coerces; `$1::int4` where inference fails) | `int2`, `int4`, `int8`, `oid`; `bool` as 0 or 1 |
| `Float(x)` | `float8`; NaN is a real NaN, not NULL (SQLite's rule does not apply; a difference from `Db`) | `float4`, `float8` |
| `Text(s)` | `text` | `text`, `varchar`, `char`, `name`, `uuid`, `json`, `jsonb`, `date`, `time`, `timestamp`, `timestamptz`, `interval`, `numeric`: their canonical text (`numeric` as text keeps every digit; kio has no decimal) |
| `Blob(b)` | `bytea` | `bytea` |
| `Null` | unknown-typed NULL | NULL |
| any other column type (arrays, enums, ranges, composite) | | `invalid: column 3 has type int4[]`, hint `cast it: col::text` |

Errors: `pg-err { invalid(string), denied, busy, too-large, constraint(string) }` as `db-err`, plus `unreachable` (the server gone mid-run, H38's rule), each audit line carrying `"sqlstate"`.

### Schema migrations

| Who | How | Grant |
|---|---|---|
| Not the server at each request | the `CREATE TABLE IF NOT EXISTS` idiom (guide, kio-web's sqlnotes) is wrong here: DDL takes locks on a shared server and the serving role should not have `CREATE` | the serving `--pg app=env:APP_URL` role has DML only |
| A `migrate.kio` entry (`kio run migrate.kio --pg db=env:MIGRATE_URL`) | the version-table idiom (D7): `SELECT version FROM schema_version` then `transaction([DDL.., "UPDATE schema_version .."])`; one transaction per version step (Postgres DDL is transactional) | an owner role, run by the operator or CI, its own audit log; `kio check --base` on migrate.kio shows the DDL as `exec`/`transaction` only, so the reviewer reads the SQL |
| An external tool (sqitch, Flyway, the team's own) | nothing in kio | the operator's |

### Fake and test execution

| Option | What a test gets | Cost | Verdict |
|---|---|---|---|
| T4-style recorded answers (`--pg db=fake:db.pg`: statement text, then rows) | brittle: the fake cannot evaluate SQL, so one test per exact statement; a changed `ORDER BY` fails the match | a FILE format and matcher, ~400 lines | no: it tests the fixture, not the program |
| SQLite standing in for Postgres | the wrong dialect (`$1`, types, `serial`, `ON CONFLICT` details); green tests, red production | nothing | no |
| **An ephemeral real database with rollback per test** (recommended): the test grants file names `--pg db=env:TEST_URL`; the host opens one transaction per test and rolls it back at the end; a program `transaction` inside becomes a savepoint; `kio review` accepts it since nothing survives for the other side | the real dialect, every test isolated and repeatable; CI uses a service container, locally `pg_tmp` or Docker (the operator's) | the rollback mode in the test binder, ~200 lines | yes |
| `--pg-record db=sent` (T4b's pattern: SQL and parameter count per call into a `Dir`) | assertions on what was sent | ~100 lines | later, on evidence; a test can `query` the table inside its own transaction instead |

Differential testing: the interpreter and the compiled program run the same test against the same server, each in its own rolled-back transaction; equal results are the oracle, as for `Db`. Mutants: `kio review` today skips a test holding a real host; a rolled-back Postgres is deterministic enough to run them, but `now()` and sequences are not: decide when built, default skip.

## 3. The operator's side

What the host cannot enforce and the binds line can only name. In a Postgres grant the role **is** the schema-level authority.

| Responsibility | Why it stays the operator's | Overridable by the program? |
|---|---|---|
| A least-privilege role per grant: `GRANT SELECT ON orders TO shop_ro`; no `CREATE`, no `SUPERUSER`, `NOINHERIT` | the host does not parse SQL; a table list in a flag would be unenforced | no |
| Row-level security policies | the server applies them per role; the host sees rows, not policies | no |
| `pg_hba.conf`: the role only from the kio host, TLS required; `CONNECTION LIMIT` on the role (at least `--pg-conns` times instances) | the network boundary is the server's | no |
| `ALTER ROLE app SET statement_timeout`, `idle_in_transaction_session_timeout` as backstops | the host sets both per call, but a hostile `SET` within a statement (refused by the fence) or a `DO` block could reset them; a role setting is one more layer, not a guarantee | yes (user-settable parameters): defence in depth, not a fence |
| Isolation level, `search_path` for the role | semantics the program relies on; the host does not choose them | within a call only |
| Backups, failover, the DSN's rotation (restart kio) | outside the sandbox | |
| Extensions the role may call (`dblink`, `pg_read_file`, `COPY .. PROGRAM`): revoke | a function call is a `SELECT` to the host | no, if revoked |

## 4. Options

| | A. Not yet | B. `Db` grows a Postgres backend | C. A `Pg` capability |
|---|---|---|---|
| Program | `db: Db` (SQLite); a service with an HTTP face (PostgREST, Supabase, Neon's HTTP API) through `api: Http` with `--http-header api=Authorization:env:..` and a kio-web library wrapper | `db: Db`, `--db orders=env:ORDERS_URL`; the same five methods, `$1` in the SQL | `orders: Pg`, `--pg orders=env:ORDERS_URL:ro`; five methods, `pg-err` |
| caps.wit | unchanged | unchanged | new `interface pg`, a `grant` case: a security-review event, the maintainer's |
| Host dependencies | none | `tokio-postgres`, `postgres-types`, `postgres-protocol`, `tokio-postgres-rustls` (rustls is already in the tree through `ureq`); pure Rust, host only, `lang` untouched | the same |
| What the type tells a reviewer | `Db` is SQLite; `Http` is a host | `Db` means "some SQL database": the dialect, the fence (SQLite's authorizer and `stmt_readonly` against a role plus `READ ONLY`), NaN, temp tables and `busy`'s meaning all depend on the grant, which the type no longer names | `Pg` names its fence and dialect; `Db` keeps meaning SQLite, the test oracle's engine |
| Tests | T4 fakes for the HTTP service; `Db` as today | `kio test` on `:memory:` SQLite passes programs Postgres rejects (`?`, types) | the rollback mode against a real server; no way to run a `Pg` test without one |
| Evidence gathered | the next target app tells whether 1-4 of section 1 appear | the app that needs it exists first | the same |
| One-way doors | none | `Db`'s semantics (programs rely on the dialect); the dependency | caps.wit; the dependency |
| Cost | 0 now; ~150 kio lines of wrapper in kio-web when a service needs one | host ~1,500 lines (pool, fence, types, cancel, audit) + the test mode ~200 + a Postgres in CI + docs; `Db`'s tests run on both engines | B's host work + bindings in capgen, interpreter, runtime and codegen (~200 mechanical lines, `Db`'s shape) + guide, language, design, open_questions; 2-3 weeks of agent work plus the security review |
| Gives | reach to any service with an HTTP API, under `Http`'s fence and fake | Postgres | Postgres, named |

Not an option: the program building a DSN from a `Value` or `Dir` and connecting over a raw socket. kio has no socket capability, and a credential the program holds is the thing every grant above exists to prevent.

## 5. Recommendation

**A, not yet**, with a trigger and a shape pinned so the decision takes one reading when the evidence lands:

1. Build nothing until a target application (order-of-work 2, W1/A4) hits section 1's case 1 or 2 and logs it. The next held-out build (A4) should be given a brief that could plausibly need Postgres (an app over an existing database) so the absence of a finding means something.
2. Meanwhile, two things that are not doors: D7's migration guidance for `Db` (the version-table idiom is the same one a Postgres `migrate.kio` would use), and a kio-web wrapper over PostgREST if any example needs a hosted database, which also exercises `--http-header` and T4 against a real service.
3. When built, **C over B**: a `Pg` type, `Db` left as SQLite. The reason is trust.md's gate: a reviewer reading `db: Db (exec)` must know which fence stands behind it; with B the answer is "read the grants file", with C it is in the signature. B saves ~200 mechanical lines and one caps.wit entry and costs the meaning of `Db` for every existing program and test.
4. Before C: the maintainer decides the caps.wit interface (five methods and `pg-err` as above, `unreachable` included), the dependency set, and whether a rolled-back Postgres counts as deterministic enough for `kio review`'s mutants.

The questions H42 asked that need no further exploration once C is chosen: credentials (`read_secret`, a host `Secret`, non-secret parts shown), authority (role plus five methods plus `READ ONLY`), deadlines (three layers), types (the table), migrations (a separate entry under an owner role), tests (rollback per test). The one that stays open until an application exists is whether anyone needs it.
