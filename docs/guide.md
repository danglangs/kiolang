# kio guide

The operator's and user's reference: every grant, limit and output of `kio run`. The language itself is [language.md](language.md).

```
kio check <prog.kio>             the authority report, or errors with hints
kio run <prog.kio> [grants] [limits] [--audit log] [--listen addr]
kio mcp                          the MCP server (README, "Use from an agent")
```

`kio run` prints the authority report, then `main`'s value; an `Err` exits 1. Every parameter of `main` is a capability, and each needs its grant: the flag's `name` is the parameter's name.

```
$ kio run examples/fx/report.kio --dir input=examples/in --dir out=examples/tour/out
examples/fx/report.kio:10: error: no grant for `fx: Http`
  hint: grant it: `--http fx=host`
```

## Grants

| Type | Flag | Grants |
|---|---|---|
| `Dir` | `--dir name=path` | a directory: `read`, `create`, `list`, `write`, `remove`, `exists` |
| `Http` | `--http name=host`, `--http-methods name=POST,..` | one host over HTTPS, GET unless more methods are granted; `--http name=http://127.0.0.1:9000` plain HTTP, for local testing |
| `Kv` | `--kv name=path`, `name=path:ro`, `name=:memory:` | a key-value store |
| `Db` | `--db name=path`, `name=path:ro`, `name=:memory:` | a SQLite database: `query`, `exec`, `transaction` |
| `Rng` | `--rng name`, `--rng name=seed:N` | random values from the OS, or a fixed stream |
| `Clock` | `--clock name`, `--clock name=fixed:T` | the system clock, or one fixed time |
| `Log` | `--log name=path`, `--log name=-` | an append-only log file, or stderr |
| `Incoming`, `Outgoing`, `WsMessage`, `WsSend` | `--listen addr` | a listening socket; bound by type, not name |

### Dir

A `Dir` reaches only paths beneath its root: `..`, absolute paths and symlinks out are refused, though the program type-checks.

```
$ kio run examples/tour/escape.kio --dir input=examples/tour/in
main : input read
error: IoErr::InvalidPath
```

### Http

`--http api=api.example` grants one host over HTTPS, GET only; `--http api=http://127.0.0.1:9000` allows plain HTTP for local testing. `--http-methods api=POST,PUT,PATCH,DELETE` adds methods (also `HEAD`, `OPTIONS`).

- `get(path) -> Result<Bytes, HttpErr>`: a non-2xx status is `Err(Status(n))`.
- `send(method, path, headers, body) -> Result<HttpResponse, HttpErr>`: headers are `"name: value"` strings; any status is `Ok`, read `r.status`, `r.headers`, `r.body`. Framing headers (`host`, `content-length`, `transfer-encoding`, `connection`, ..) and any CR or LF are `Invalid`; a method not granted is `Denied`; a body over 8 MB is `TooLarge`. Redirects are followed only for GET and HEAD.
- `send_all([HttpRequest { method, path, headers, body, timeout_ms }, ..])`: up to 64 requests at once; results in list order; a request past its timeout is `Err(Timeout)` in its place.
- The audit log records each request's method, path, status, byte counts and header names, never a header value.

A path, not a URL: `get(path)` takes a path on the granted host; anything else (`https://other.example/..`, `//other.example`) is `HttpErr::Denied` without a request. Redirects are rechecked against the grant, IP literals and private addresses are refused, DNS is pinned per run.

```
$ kio run examples/fx/report.kio --dir input=examples/in --http fx=api.frankfurter.dev --dir out=examples/tour/out
main      : input read, fx get, out write
summarize : none
Ok(["acme 102 EUR", "globex 38 EUR", "initech 256 EUR"])
```

### Kv

A store that outlives runs and is shared by every `--listen` request. `:memory:` keeps it for the process only; `--kv-mb n` caps its size (64 MB by default). `swap` updates a key atomically, `add` counts in one, `scan` pages through a prefix's entries.

`path:ro` is read-only: a program that can call `set`, `delete`, `swap` or `add` on it is refused, no lock is taken, and each `get`, `keys` and `scan` sees what another kio writing the file stored (a server reading tokens a separate job rotates).

```
$ kio run examples/ledger.kio --dir input=examples/in --kv totals=/tmp/ledger.kv
load  : totals scan
main  : input read, totals add scan
parse : none
Ok(["initech 300", "acme 120", "globex 45"])
$ kio run examples/ledger.kio --dir input=examples/in --kv totals=/tmp/ledger.kv:ro
kio: `totals` is granted read-only but `main` can call `totals add`
  hint: drop `:ro` or remove the writes
```

### Rng, Clock, Log

| Flag | Gives |
|---|---|
| `--rng r` | OS randomness: `r.bytes(n)`, `r.int(lo, hi)` |
| `--rng r=seed:1` | a stream the same every run (under `--listen`, keyed by the request number too). Anyone who knows the seed predicts every value: tests and replay only, never a server facing clients |
| `--clock now` | the system clock: `now.now_ms()` |
| `--clock now=fixed:2026-11-09T12:00:00Z` | always that time (also `fixed:<ms>`) |
| `--log events=app.log` | one appended line per `write`: control characters escaped, 64 KB a line, 1 MB a run, `req N: ` first under `--listen` |
| `--log events=-` | the same lines on stderr |

None of the three is reachable without its grant, so a run with fixed values replays exactly:

```
$ kio run examples/raffle.kio --dir input=examples/in --rng r=seed:1 --clock now=fixed:2026-11-09T12:00:00Z --log log=/tmp/raffle.log
main : input read, r int, now now_ms, log write
Ok("globex wins at 1794225600000")
```

### Db

A SQLite database (the bundled SQLite, in the host) that outlives runs and is shared by every `--listen` request, one statement at a time. `--db name=path` opens or creates the file (WAL mode); `:memory:` keeps it for the process only. The program runs SQL with `query(sql, params)` (the rows, as `List<List<SqlValue>>`), `exec(sql, params)` (a statement that returns no rows: the rows it changed) and `transaction(sqls, params)` (each statement with its own parameter list, all applied or none). Values go in as `?` parameters, `SqlValue::Int(n)`, `Text(s)`, `Float(x)`, `Blob(b)` or `Null`, never as SQL text; the schema is the program's to create (`CREATE TABLE IF NOT EXISTS ..`).

`path:ro` is read-only: a program that can call `exec` or `transaction` on it is refused at startup, and a `query` that would write is `Denied`.

The connection is fenced, since the SQL is the program's and SQLite runs outside the sandbox: `ATTACH`, `DETACH`, `VACUUM`, `BEGIN`/`COMMIT`/`ROLLBACK`/`SAVEPOINT` (use `transaction`), `load_extension` and every `PRAGMA` but a few reads (`table_info`, `index_list`, `foreign_keys`, ..) are `Denied`; the schema is protected (`SQLITE_DBCONFIG_DEFENSIVE`). Limits: one statement per call, 1 MB of SQL, 8 MB per value, 4,096 parameters, 1,000 statements per `transaction`, 10,000 rows or 8 MB per `query` (else `TooLarge`); a statement still running at the run's `--timeout` is interrupted and the run stops; SQLite's memory is capped at 256 MB per process; another process holding the file, or another request's statement holding the connection, for over 2 s is `Busy`. The database's path follows the `--kv` rules: not a symlink, not inside a granted `--dir` or the compile cache, not a store's or log's file.

```
$ kio run examples/totals.kio --dir input=examples/in --db totals=/tmp/totals.db
int   : none
main  : input read, totals exec query transaction
parse : none
text  : none
Ok(["initech 300", "acme 120", "globex 45"])
$ kio run examples/totals.kio --dir input=examples/in --db totals=/tmp/totals.db
..
Ok(["initech 600", "acme 240", "globex 90"])
$ kio run examples/totals.kio --dir input=examples/in --db totals=/tmp/totals.db:ro
kio: `totals` is granted read-only but `main` can call `totals exec`
  hint: drop `:ro` or remove the writes
```

The audit log has a line per call with the SQL (cut at 256 characters), the parameter count, the rows or rows changed and the bytes, never a parameter value:

```
{"event":"call","seq":4,"cap":"totals","kind":"Db","method":"query","arg":"SELECT name, amount FROM totals ORDER BY amount DESC LIMIT ?","bytes":49,"result":"ok","params":1,"rows":3,"ms":0.1}
```

### --listen: HTTP and WebSockets

A listening socket is a grant too. Each request runs in a fresh sandbox; `main` takes `req: Incoming, res: Outgoing` (any names) plus its other grants. `send` answers the client at once, and `main` may go on working under the request's limits.

```
$ kio run examples/serve/app.kio --dir public=examples/tour/in --listen 127.0.0.1:8080
main             : req body header_names header_values method target, res send, public read
...
listening on http://127.0.0.1:8080
$ curl http://127.0.0.1:8080/hello/kio
{"hello": "kio"}
```

A WebSocket server's `main` takes `msg: WsMessage, conn: WsSend` and gets a fresh sandbox per event (open, each message, close), run in order per connection; state between events goes in a `Kv`. `msg.target()` and `msg.header(name)` give the upgrade request's path and headers on every event, to route or check a token.

```
$ kio run examples/ws/echo.kio --listen 127.0.0.1:8080
main : msg data kind, conn binary text
program sha256:...
listening on ws://127.0.0.1:8080
```

| Flag | Does |
|---|---|
| `--host app.example` | on a loopback `--listen`, a request's `Host` must be `localhost`, `127.0.0.1` or `[::1]` (against DNS rebinding); this allows another name, and is the only restriction on a non-loopback bind |
| `--ws-origin https://app.example` | a browser may open a WebSocket only from the server's own host; this allows another origin. Clients that send no `Origin` (websocat) are not browsers and are accepted |

Throughput: 73,800 requests/s with keep-alive (kio-web hello, roadmap.md "Speed today"), 17,800-20,200 without ([perf.md](perf.md)), a fresh sandbox per request.

## Limits

Every run is bounded; each limit's error names the flag that raises it. Under `--listen` the limits are per request, and the defaults are smaller: fuel 10,000,000, memory 64 MB, timeout 5 s (the flags override them).

| Flag | Default |
|---|---|
| `--fuel n` | 10,000,000,000 (Wasmtime fuel) |
| `--timeout secs` | 10 |
| `--memory mb` | 256 |
| `--stack kb` | 2,048 (at most 8 MB) |
| `--calls n` | 10,000 host calls |
| `--io mb` | 100 |

```
$ kio run examples/tour/forever.kio
out of fuel (limit 10000000000; raise it with --fuel)
```

## Audit log

`--audit log.jsonl` records every host call: capability, method, argument, bytes and result (never a drawn random value or a log line). A call the report did not predict stops the run as a checker bug. The log is appended to, never truncated, and refused at startup if it is a `--kv` store's or `--db` database's file, inside a granted `--dir` or inside the compile cache.

```
$ grep denied /tmp/leak.jsonl
{"event":"call","seq":3,"cap":"fx","kind":"Http","method":"get","arg":"https://collect.example/?d=acme,120\u000aglobex,45\u000ainitech,300\u000a","bytes":0,"result":"denied","ms":0.0}
```

Each run writes a `start` line (program hash, grants, limits), its calls, and an `end` line (outcome, calls, I/O, peak memory, fuel used). Under `--listen` it is one log for the server: the header once, then each request's calls and trailer, every line starting with `"req":N` (and `"conn":N` for a WebSocket event); the trailer counts calls made after the response was sent (`"after_send":N`).

## Compile cache

`kio run` keeps compiled programs in `~/.cache/kio`, so a second run of the same program skips the compile (about 20-25 ms). A run granted a `--dir` that contains, or is inside, the cache does not use it, and a `--kv` store there is refused.

## Built-ins

`Json` (`s.parse_json()` gives `Result<Json, Str>`, `j.to_json()`, accessors `get`, `at`, `as_str`, ..), `HashMap<K, V>` (insertion-ordered, beside the key-ordered `Map`), generic functions and types with `T: Eq` and `T: Ord` bounds, `<` on `Str` by bytes, `sort_by`, `join`, `sha256`, `to_hex` and `ct_eq` (constant-time compare). Every method: [language.md](language.md).

## From source

`cargo test` runs every suite; `cargo run -p kio -- run file.kio` runs without installing. `examples/tour/demo.sh` walks through the report, a rejected helper, a refused escape and a fuel stop.
