# kio guide

The operator's and user's reference: every grant, limit and output of `kio run`. The language itself is [language.md](language.md).

```
kio check <prog.kio> [--all]     the authority report of what main reaches (--all: every function), or errors with hints
kio check <new.kio> --base <old.kio> [--detail]   grants new adds or drops; exit 3 if any added
kio check <prog.kio> --grants <file> [flags]   bind the file's grants to main without running
kio run <prog.kio> [--grants file] [grants] [limits] [--audit log] [--listen addr]
... --json                       one JSON object on stdout (JSON output, below)
kio test <prog.kio> [--grants file] [--filter substr] [--cases n] [--seed s] [--verbose] [limits]   run the #[test] functions (Tests, below)
kio review <new.kio> --base <old.kio> [--grants file] [--test-grants file] [--requirements file] [--entry file [--entry-grants file]].. [--seed s] [--mutants n] [--mutations file] [--json]   one labelled page for a change (Reviewing a change, below)
kio deps pin [<prog.kio>]        rewrite the hashes in kio.deps beside prog (default ./kio.deps) (Dependencies, below)
kio mcp                          the MCP server (README, "Use from an agent")
```

`kio run` prints the authority report, then `main`'s value. The report shows `main` and each function it reaches that uses a capability, then under `reached only from tests:` those only a `#[test]` reaches, then a count of the rest ([language.md](language.md#authority-report)); `kio check --all` lists every function. The host holds a run to `main`'s line, the same in every form. An `Err` from `main` exits 1. Every parameter of `main` is a capability, and each needs its grant: the flag's `name` is the parameter's name.

```
$ kio run examples/fx/report.kio --dir input=examples/in --dir out=examples/tour/out
examples/fx/report.kio:10:21: error: no grant for `fx: Http`
  hint: grant it: `--http fx=host`
```

`kio check` and `--base` without installing kio: the [playground](../site/playground.html) runs the same checker in a browser, on one file; nothing runs.

## Dependencies

`mod web;` loads `web.kio` beside the root, unless `kio.deps` beside the root lists `web`: then it loads that path, and only if the file's sha256 is the one pinned. One line per module, `NAME  PATH  sha256:HEX`, PATH relative to `kio.deps` (never absolute, so the file reads the same on every checkout); `#` comments. Modules stay flat: a dependency that uses another (`web` uses `http`) needs `mod http;` in the root, and `http` listed or beside it. Local paths only: a sibling checkout, a submodule, a vendored copy.

```
$ cat kio.deps
# kio-web's library, from a sibling checkout
web  ../../web.kio  sha256:e4aad9a99560d8fd3d5f893e372764cc315c1b08611af9efce55b17e0fa6959e
$ kio check app.kio
...
dep web ../../web.kio sha256:e4aad9a99560d8fd3d5f893e372764cc315c1b08611af9efce55b17e0fa6959e
$ kio run app.kio            # after web.kio changed
app.kio:1:1: error: ../../web.kio does not match kio.deps:2 (sha256:e4aad9a99560.. pinned, sha256:91c0a1b2c3d4.. found)
  hint: review the change, then `kio deps pin app.kio`
$ kio deps pin app.kio
web  sha256:e4aad9a99560.. -> sha256:91c0a1b2c3d4..
```

`check` (each side of `--base` with its own `kio.deps`), `test`, `run`, `--listen` and `review` all refuse a mismatch before compiling; the file hashed is the file compiled. `kio deps pin` is the only command that writes `kio.deps`: it rehashes every line (a line `NAME  PATH` gets its hash), keeps comments and order, prints each change or `nothing changed`. A dependency cannot do anything on its own: its calls are charged to the caller's grants, so a change to it shows in `kio check --base`; `kio review` lists each dependency added, removed or changed under Not checked. A writable `--dir` holding `kio.deps` or a dependency is refused, as one holding the program is. Each `start` line of the audit log carries `deps`; `kio check --json` and `kio run --json` have `deps: [{name, path, sha256}]`. `kio check NEW --base OLD` ends with a line per dependency whose pin changed, `dep web changed (sha256:e4aad9a99560.. -> sha256:91c0a1b2c3d4..)`, `dep web added` or `dep web removed`; exit codes follow authority alone.

Errors (exit 2): a malformed line (`kio.deps:3: ... is not a dependency line`: an absolute path or a control character is one), a name listed twice, a line without a hash, a missing file (`kio.deps:2: ../../web.kio does not exist`) or one that is not a regular file, a mismatch, and a name both listed and beside the root (`` `web` is in kio.deps:2 and beside app.kio``, hint `delete one`). A listed name no `mod` declares is ignored, so one `kio.deps` serves several roots. `--json` gives them kind `deps`.

## Grants

| Type | Flag | Grants |
|---|---|---|
| `Dir` | `--dir name=path`, `name=path:ro` | a directory: `read`, `create`, `list`, `write`, `append`, `remove`, `exists` |
| `Http` | `--http name=host`, `--http-methods name=POST,..`, `--http-header name=Header:env:VAR` | one host over HTTPS, GET unless more methods are granted; `--http name=http://127.0.0.1:9000` plain HTTP, for local testing |
| `Kv` | `--kv name=path`, `name=path:ro`, `name=:memory:` | a key-value store |
| `Db` | `--db name=path`, `name=path:ro`, `name=:memory:` | a SQLite database: `query`, `exec`, `transaction` |
| `Rng` | `--rng name`, `--rng name=seed:N` | random values from the OS, or a fixed stream |
| `Clock` | `--clock name`, `--clock name=fixed:T` | the system clock, or one fixed time |
| `Log` | `--log name=path`, `--log name=-` | an append-only log file, or stderr |
| `Hmac` | `--hmac name=env:VAR`, `name=file:PATH`, either with `:verify` | HMAC-SHA256 under a key the program never holds: `sign`, `verify` |
| `Secret` | `--secret name=env:VAR`, `name=file:PATH` | a value the program can compare (`matches`), never read |
| `Incoming`, `Outgoing`, `WsMessage`, `WsSend` | `--listen addr` | a listening socket; bound by type, not name |

### Dir

A `Dir` reaches only paths beneath its root: `..`, absolute paths and symlinks out are refused, though the program type-checks.

```
$ kio run examples/tour/escape.kio --dir input=examples/tour/in
main : input read
error: IoErr::InvalidPath
```

`write` replaces a file atomically (a reader sees the old or the new file); `append(path, data)` creates the file if missing and adds `data` at its end in one `O_APPEND` write, so the concurrent requests of a `--listen` server never interleave within a call (a log file). Both charge `data` to `--io`; `append` on a directory, a symlink or a path ending in `/` is `Denied`, as `write` is.

`path:ro` is read-only: `write`, `create`, `append` and `remove` are `Err(IoErr::Denied)` (audited `denied`), and a program whose report shows any of them on that grant is refused at startup, under `--listen` and by `kio check --grants`. The audit header shows the grant as `path:ro`.

A writable `--dir` that contains the program's own source (the root file or a module file, also through any link of a symlink chain or a symlinked directory) is refused by `kio run`, `--listen` and `kio check --grants`: the program could rewrite itself for the next run. Keep the program outside every granted directory, or grant that directory `:ro`. A source file that changes between being read and being checked (`app.kio` changed while kio was starting) is refused too; run again.

```
$ kio run examples/fx/report.kio --dir input=examples/in:ro --dir out=examples/tour/out:ro --http fx=api.frankfurter.dev
kio: `out` is granted `:ro` but the program can write it (`out write`)
  hint: drop `:ro`, or remove the write
```

### Http

`--http api=api.example` grants one host over HTTPS, GET only; `--http api=http://127.0.0.1:9000` allows plain HTTP for local testing. `--http-methods api=POST,PUT,PATCH,DELETE` adds methods (also `HEAD`, `OPTIONS`).

The report names each call's method when it is a literal or a constant: `api send PUT`, `api send_all POST` (every request that call sends is a POST: its argument is built from `HttpRequest` literals, named functions that build them (`xs.map(f)` too) and locals filled with those), `api send *` when the checker cannot see it (a variable, a parameter, a computed string). A program whose report shows a literal method the grant does not allow, or capitals that are not a method (`"PUTX"`), is refused at startup, under `--listen` and by `kio check --grants`; a `*` is checked per request, as every request is.

```
$ kio run push.kio --http crm=api.crm.example --http-methods crm=POST    # main : crm send PUT
kio: `crm` sends PUT but `--http-methods crm=POST` grants only POST
  hint: add PUT to --http-methods, or remove the call
```

- `get(path) -> Result<Bytes, HttpErr>`: a non-2xx status is `Err(Status(n))`.
- `send(method, path, headers, body, timeout_ms) -> Result<HttpResponse, HttpErr>`: `timeout_ms` 0 is the 10 s limit, less cuts the request off as `Err(Timeout)`, negative is `Invalid`; headers are `"name: value"` strings; any status is `Ok`, read `r.status`, `r.headers`, `r.body`. Framing headers (`host`, `content-length`, `transfer-encoding`, `connection`, ..) and any CR or LF are `Invalid`; a method not granted is `Denied`; a body over 8 MB is `TooLarge`. Redirects are followed only for GET and HEAD.
- `send_all([HttpRequest { method, path, headers, body, timeout_ms }, ..])`: up to 64 requests at once; results in list order; a request past its timeout is `Err(Timeout)` in its place.
- The audit log records each request's method, path, status, byte counts and header names, never a header value.

**A secret the program never holds.** `--http-header api=Authorization:env:API_AUTH` (or `:file:PATH`) has the host add `Authorization: <value>` to every request on `api`: `get`, `send`, each `send_all` request and each redirect hop. The value is read once at start (a missing variable or file, or an empty or whitespace-only value, is refused; a file's trailing newline is dropped) and never reaches the program, the audit log, `--log`, an error or stderr; the audit header lists `injects=authorization` and each request `"injected":["authorization"]`. Repeatable, one header name once per grant. A program that sends a header of the same name (any case) gets `Err(HttpErr::Invalid)`, nothing sent. Refused on an `http://` origin that is not a loopback IP literal (`http://127.0.0.1:PORT`), where the key would cross the network in clear. A key file inside a granted `--dir`, or with other hard links (one could be inside), is refused: the program could read it.

```
$ API_AUTH="Bearer $(cat ~/.keys/stripe)" kio run charge.kio --http stripe=api.stripe.com \
    --http-methods stripe=POST --http-header stripe=Authorization:env:API_AUTH
```

Grant only origins that do not echo request headers back: an endpoint like httpbin's `/headers` returns the key in its response body, which the program can read and leak.

`--http api=fake:api.http` answers from a file instead of a server, in `kio test`'s grants file only ([Tests](#a-fake-host)); `kio run`, `--listen`, `kio check --grants` and `kio review --grants` refuse it: "--http api=fake:api.http: a fake answers tests only".

A path, not a URL: `get(path)` takes a path on the granted host; anything else (`https://other.example/..`, `//other.example`) is `HttpErr::Denied` without a request. Redirects are rechecked against the grant, IP literals and private addresses are refused, DNS is pinned per run.

Under `--listen` each request gets its own `Http` handle (its own DNS lookup, pinned for that request) with the same grant, methods and limits; its audit lines carry `"req":N`. A request holds at most 64 HTTP requests in flight; the process 64 per worker (at least 1,024), so a running request is never refused one. Past that (workers a deadline left behind) a new `get`, `send` or `send_all` is `Err(Timeout)` at once, nothing sent (audited `"busy":true`); other calls, a `Dir` read for one, are unaffected. A request may make 1,000 calls (`--calls`). Every outbound HTTP request, each redirect hop and each `send_all` request included, takes one call before it is sent, so a run sends at most `--calls` requests; a hop with no call left is not sent and the run stops for `--calls` (each request's audit line has `"redirects":N`, the hops it sent after the first). A path built from request input is the program's to check: the host keeps it on the granted host, but `?`, `/` and `..` inside it reach that host as given.

```
$ kio run examples/fx/report.kio --dir input=examples/in --http fx=api.frankfurter.dev --dir out=examples/tour/out
main : input read, fx get, out write

1 more uses no capability; --all lists every function
Ok(["acme 102 EUR", "globex 38 EUR", "initech 256 EUR"])
```

### Kv

A store that outlives runs and is shared by every `--listen` request. `:memory:` keeps it for the process only; `--kv-mb n` caps its size (64 MB by default). `swap` updates a key atomically, `add` counts in one, `scan` pages through a prefix's entries.

`path:ro` is read-only: a program that can call `set`, `delete`, `swap` or `add` on it is refused, no lock is taken, and each `get`, `keys` and `scan` sees what another kio writing the file stored (a server reading tokens a separate job rotates).

```
$ kio run examples/ledger.kio --dir input=examples/in --kv totals=/tmp/ledger.kv
main : input read, totals add scan
load : totals scan

1 more uses no capability; --all lists every function
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

### Hmac

`--hmac stripe=env:STRIPE_WHSEC` (or `file:PATH`) grants `stripe: Hmac`: `stripe.sign(msg) -> Result<Bytes, HmacErr>` (32 bytes) and `stripe.verify(msg, mac) -> Bool` (constant time; a MAC of the wrong length is `false`). The key is the variable's or file's text as given (a provider's `whsec_..` string is the key), read once at start under `--http-header`'s rules (a missing, empty or whitespace-only value, a file that is not a regular file, inside a granted `--dir`, or the `--audit` or a `--log` file, is refused, and so is a file with other hard links, since the program could read one; a file's one trailing newline is dropped; at most 8 KB). It never reaches the program, the audit log, `--log`, an error or stderr. `:verify` at the end (`--hmac stripe=env:STRIPE_WHSEC:verify`) makes `sign` return `Err(HmacErr::Denied)`, so a webhook receiver cannot mint signatures; a program whose report shows `sign` on that grant is refused at startup, as `:ro` refuses a writer. Under `--listen` every request uses the one key. The audit log records each call's method, the message length and, for `verify`, `"result":"true"` or `"false"`; a refused `sign` is `"denied"`.

```
fn signed(k: Hmac, header: Str, body: Bytes) -> Bool {   // stripe-signature: t=..,v1=<hex>
  let mut t = "";
  let mut sig = "";
  for part in header.split(",") {
    if let Some(v) = part.strip_prefix("t=") { t = v; }
    if let Some(v) = part.strip_prefix("v1=") { sig = v; }
  }
  let mut msg = (t + ".").to_bytes();
  msg.extend(body);
  t != "" && k.verify(msg, unhex(sig))      // unhex: your own hex decoder
}
```

### Secret

`--secret token=env:API_TOKEN` (or `file:PATH`) grants `token: Secret`, a value with one method: `token.matches(presented: Str) -> Bool`, compared in the host in time that depends only on the presented string's length (another length is `false`; not even the secret's length can be timed). The program cannot read, log, send or hash the value; it is read once at start under `--http-header`'s rules (as `--hmac`'s key: missing, empty, not a regular file, inside a granted `--dir`, the `--audit` or a `--log` file, or with other hard links is refused; a file's one trailing newline is dropped; at most 8 KB) and never reaches the program, the audit log, `--log`, `--json`, an error or stderr. The audit log records each call as `"method":"matches"` with `"result":"true"` or `"false"` and the presented length as `bytes`, not the presented string. The report shows the grant as `token: Secret (matches)`; `kio test` needs it named in the grants file, as `Hmac`.

```
fn main(req: Incoming, res: Outgoing, token: Secret) -> Result<(), WebErr> {
  let presented = header(req, "authorization").strip_prefix("Bearer ").unwrap_or("");   // header: your own lookup
  if token.matches(presented) { res.send(200, [], [], "ok".to_bytes()) } else { res.send(401, [], [], "no".to_bytes()) }
}
```

The alternative without a grant: keep the token's `sha256().to_hex()` in a file in a `:ro` dir and compare it to `presented.to_bytes().sha256().to_hex().to_bytes()` with `ct_eq`; a reviewer then reads the hash file, and the program still holds no secret.

### Db

A SQLite database (the bundled SQLite, in the host) that outlives runs and is shared by every `--listen` request, one statement at a time. `--db name=path` opens or creates the file (WAL mode); `:memory:` keeps it for the process only. The program runs SQL with `query(sql, params)` (the rows, as `List<List<SqlValue>>`), `exec(sql, params)` (a statement that returns no rows: the rows it changed) and `transaction(sqls, params)` (each statement with its own parameter list, all applied or none). Values go in as `?` parameters, `SqlValue::Int(n)`, `Text(s)`, `Float(x)`, `Blob(b)` or `Null`, never as SQL text; the schema is the program's to create (`CREATE TABLE IF NOT EXISTS ..`). Nothing runs once at a server's startup, so a `--listen` server runs that statement at the start of every request: an `exec` that changes nothing once the table exists, and one `call` line per request in the audit log (kio-web's sqlnotes).

`path:ro` is read-only: a program that can call `exec` or `transaction` on it is refused at startup, and a `query` that would write is `Denied`.

The connection is fenced, since the SQL is the program's and SQLite runs outside the sandbox: `ATTACH`, `DETACH`, `VACUUM`, `BEGIN`/`COMMIT`/`ROLLBACK`/`SAVEPOINT` (use `transaction`), `load_extension` and every `PRAGMA` but a few reads (`table_info`, `index_list`, `foreign_keys`, ..) are `Denied`; the schema is protected (`SQLITE_DBCONFIG_DEFENSIVE`). Limits: one statement per call, 1 MB of SQL, 8 MB per value, 4,096 parameters, 1,000 statements per `transaction`, 10,000 rows or 8 MB per `query` (else `TooLarge`); `--db-mb n` caps each database (64 MB by default, as `--kv-mb`; a write past it is `TooLarge` and changes nothing; a file already larger may not grow); a statement still running at the run's `--timeout` is interrupted and the run stops, inside `instr`, `replace`, `LIKE` and `GLOB` too (the host's versions, which give SQLite's results; `LIKE` does not use an index); SQLite's memory is capped at 256 MB per process; another process holding the file, or another request's statement holding the connection, for over 2 s is `Busy`. The database's path follows the `--kv` rules: not a symlink, not inside a granted `--dir`, not a store's or log's file, and not another grant's `-wal`, `-shm` or `-journal`.

```
$ kio run examples/totals.kio --dir input=examples/in --db totals=/tmp/totals.db
main : input read, totals exec query transaction

3 more use no capability; --all lists every function
Ok(["initech 300", "acme 120", "globex 45"])
$ kio run examples/totals.kio --dir input=examples/in --db totals=/tmp/totals.db
..
Ok(["initech 600", "acme 240", "globex 90"])
$ kio run examples/totals.kio --dir input=examples/in --db totals=/tmp/totals.db:ro
kio: `totals` is granted read-only but `main` can call `totals exec`
  hint: drop `:ro` or remove the writes
```

The audit log has a line per call with the SQL (cut at 256 characters), the parameter count, the rows or rows changed and the bytes, never a parameter value; a `too-large` result names the limit hit, `"limit":` `"rows"`, `"bytes"` (a result over 8 MB), `"value"`, `"sql"`, `"statements"`, `"memory"` or `"db-mb"`:

```
{"event":"call","seq":4,"cap":"totals","kind":"Db","method":"query","arg":"SELECT name, amount FROM totals ORDER BY amount DESC LIMIT ?","bytes":49,"result":"ok","params":1,"rows":3,"ms":0.1}
```

### --listen: HTTP and WebSockets

A listening socket is a grant too. Each request runs in a fresh sandbox; `main` takes `req: Incoming, res: Outgoing` (any names) plus its other grants, `Http` included. `send` answers the client at once, and `main` may go on working under the request's limits (a webhook after the response, say).

`--timeout` covers the whole run, the work after `send` included. An `Http` call (`get`, `send`, `send_all`) still waiting at the deadline stops the run as a timeout; the program never sees its result (a client not yet answered gets a 500; after `send`, stderr says `request failed after its response was sent`). To act on a slow upstream, give the call less than the run has: its `timeout_ms` (`send`, `send_all`) (payhook posts with 2 s under `--timeout 20`).

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

## Grants file

`--grants app.grants` reads flags from a file, kept and reviewed beside the code. One flag per line, as on the command line, split at the first space with no quoting (the value is the rest of the line, spaces and quotes included); blank lines and lines starting with `#` are skipped. Any `kio run` flag but `--grants` may appear.

```
# payhook's grants (kio-web examples/payhook/app.grants)
# The signing secret and the chat key come from the environment; the program never holds either.
--hmac stripe=env:STRIPE_WHSEC:verify
--clock now
--db db=payhook.db
--log log=payhook.log
--http slack=https://chat.example
--http-methods slack=POST
--http-header slack=Authorization:env:CHAT_AUTH
--timeout 20
--audit audit.jsonl
--listen 127.0.0.1:8080
```

```
$ STRIPE_WHSEC=whsec_... CHAT_AUTH='Bearer ...' kio run app.kio --grants app.grants
```

Flags on the command line are added after the file's. A flag that takes one value for the run (`--listen`, `--audit`, a limit, `--kv-mb`, `--db-mb`) given in both, or twice in the file, is refused, naming the file's line; one grant name in both is two grants of it, refused as on the command line. The file is refused if the program could change it for its next run: inside a granted `--dir`, the `--audit`, a `--log` or a store's file, or with other hard links. Paths in it are relative to the working directory, as on the command line.

`kio check app.kio --grants app.grants` binds the file's grants to `main`'s parameters and prints the report, without running: missing, extra and wrong-kind grants, a `:ro` or `:verify` grant the program would write or sign with, and every flag's syntax are errors as under `kio run`. It opens no granted path and reads no secret (an `env:` or `file:` source is checked for its form only), so it runs in CI. A pull request then shows both halves of the authority: `kio check --base` for what the code calls on each grant, the grants file's diff for what each grant is.

## Limits

Every run is bounded; each limit's error names the flag that raises it. Under `--listen` the limits are per request, and the defaults are smaller: fuel 10,000,000, memory 64 MB, timeout 5 s, calls 1,000 (the flags override them). `kio test` uses the `kio run` defaults, 1,000 times a request's fuel, and notes a test that would not fit in a request ([Tests](#tests)).

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

`--audit log.jsonl` records every host call: capability, method, argument, bytes and result (never a drawn random value or a log line). A call the report did not predict stops the run as a checker bug. The log is appended to, never truncated, and refused at startup if it is a `--kv` store's or `--db` database's file, or inside a granted `--dir`.

```
$ grep denied /tmp/leak.jsonl
{"event":"call","seq":3,"cap":"fx","kind":"Http","method":"get","arg":"https://collect.example/?d=acme,120\u000aglobex,45\u000ainitech,300\u000a","bytes":0,"result":"denied","ms":0.0}
```

Each run writes a `start` line (program hash, dependencies, grants, limits), its calls, and an `end` line (outcome, calls, I/O, peak memory, fuel used). Under `--listen` it is one log for the server: the header once, then each request's calls and trailer, every line starting with `"req":N` (and `"conn":N` for a WebSocket event); the trailer counts calls made after the response was sent (`"after_send":N`).

## JSON output

`--json` (anywhere after `run` or `check`) makes stdout one line, one JSON object, in every outcome; kio prints nothing else (stderr carries only `--log name=-` lines). Exit codes are unchanged. Not with `--listen` or `mcp`, and not in a grants file.

| field | |
|---|---|
| `ok` | no errors |
| `value` | `run`: `main`'s value as text; for a `Result`, the `Ok` payload alone, a `Str` unquoted (return `Ok(j.to_json())` to hand back JSON); `null` for `()` and failures |
| `errors` | `[{"kind", "message", "hint", "file", "line", "col"}]`; `file`, `line`, `col` (1-based, in characters) are `null` without a place |
| `report` | the authority report's lines: `main`'s, then each function it reaches that uses a capability (`check --all`: every function) |
| `report_tests` | the lines of functions only a test reaches |
| `hidden` | the functions counted without a line: `{"no_capability", "unreached", "unreached_with_capability"}`; `null` when no report was made |
| `diff` | `check --base`: `{"grants", "funcs", "added", "deps"}`, `deps` each dependency whose pin changed, `[{"name", "old", "new"}]` (`null` where absent) |
| `audit` | the `--audit` path (appended across runs) |
| `used` | `run`: `{"fuel", "calls", "io", "peak_memory", "ms"}` |
| `deps` | `check`, `run`: each dependency loaded through `kio.deps`, `[{"name", "path", "sha256"}]` |

Kinds: `check`, `grant` (a grant that does not bind, exit 1; one the host refuses at startup, exit 2), `usage` (exit 2), `deps` (a `kio.deps` error, exit 2), `err` (`main` returned `Err`), `runtime`, `fuel`, `memory`, `timeout`, `stack`, `calls`, `io` (a limit; the hint names its flag), `denied` (the host stopped a call), `internal`.

```
$ kio run job.kio --dir input=in:ro --json
{"audit":null,"deps":[],"diff":null,"errors":[],"hidden":{"no_capability":0,"unreached":0,"unreached_with_capability":0},"ok":true,"report":["main : input read"],"report_tests":[],"used":{"calls":1,"fuel":98619,"io":5,"ms":0.5,"peak_memory":1966080},"value":"{\"alice\": 1}"}
$ kio run job.kio --json
{"audit":null,"deps":[],"diff":null,"errors":[{"col":9,"file":"job.kio","hint":"grant it: `--dir input=path`","kind":"grant","line":1,"message":"no grant for `input: Dir`"}],"hidden":{"no_capability":0,"unreached":0,"unreached_with_capability":0},"ok":false,"report":["main : input read"],"report_tests":[],"used":null,"value":null}
$ kio run spin.kio --fuel 1000 --json
{"audit":null,"deps":[],"diff":null,"errors":[{"col":null,"file":null,"hint":"raise it with --fuel","kind":"fuel","line":null,"message":"out of fuel (limit 1000)"}],"hidden":{"no_capability":0,"unreached":0,"unreached_with_capability":0},"ok":false,"report":["main : none"],"report_tests":[],"used":{"calls":0,"fuel":1000,"io":0,"ms":0.2,"peak_memory":1179648},"value":null}
```

## Tests

`kio test prog.kio [--grants file] [--filter substr] [--cases n] [--seed s] [--json] [--verbose] [limits]` runs every `#[test]` function ([language.md](language.md#tests)) whose name contains `--filter`, sorted by name (`util::t` for one in `util.kio`). The program needs no `main`. Each test is compiled as the entry and runs in a fresh instance with the default limits (or the limit flags given). Each parameter gets the grants file's grant of its name and type (`--kv k=..` is for `k: Kv`, `--hmac k=..` for `k: Hmac`, `--secret k=..` for `k: Secret`); the file's other grants are ignored for that test, and a test is held to its own authority as `kio run` holds `main` (`:ro`, `:verify`, the source and grants-file checks). A parameter the file does not name gets, by type:

| Type | Default |
|---|---|
| `Dir` | a new empty directory under the system's temporary directory (`$TMPDIR`), mode 0700, removed afterwards |
| `Kv`, `Db` | `:memory:` |
| `Clock` | fixed at 2026-01-01T00:00:00Z |
| `Rng` | `seed:1` |
| `Log` | a buffer, printed when the test fails |
| `Http`, `Hmac`, `Secret` | none (an `Http` can be a [fake](#a-fake-host)): the test fails with "test `t` needs `api: Http`", hint "name it in a --grants file" |
| `Incoming`, `Outgoing`, `WsMessage`, `WsSend` | never (a grants file's `--listen` is refused): "test `t` needs `req: Incoming`", hint "only `kio run --listen` grants a server's capabilities; test the handler's logic in a function" |

A test fails on a failed `assert` or `assert_eq`, an `Err` return (shown as `Err("..")`), a runtime error, a limit or a refused grant. Exit 0 when every test passes (a file with no tests prints `0 passed, 0 failed`), 1 when any fails, 2 for a check error or a usage error: a bad limit, or a `--filter` no test name contains ("no test matches `x`"). A failure's message and hint are printed with control characters escaped as `Log` lines are (`\n`, `\u{1b}`); `--json` carries them as they are. `--audit`, `--listen`, `--ws-origin` and `--host` are refused, and grants go in the file, not on the command line.

```
$ kio test app.kio
ok   parses
FAIL totals
  app.kio:12:3: assert_eq failed: left `3`, right `4`
  log:
    read 2 rows
1 passed, 1 failed
$ kio test app.kio --filter parses --json
{"failed":0,"ok":true,"passed":1,"tests":[{"error":null,"fuel":6739,"log":"","ms":41.2,"name":"parses","ok":true,"peak_memory":1769472}]}
```

`--json`'s `error` is an `errors` entry of [JSON output](#json-output) (an assertion is `runtime`, an `Err` return `err`). A check or usage error prints `{"ok": false, "passed": 0, "failed": 0, "tests": [], "errors": [..]}`.

Each `--json` entry has `fuel` and `peak_memory` (bytes): what the run used, `null` when it never started (a missing grant); a property's fuel is summed over its cases, its peak memory their largest. `--verbose` adds both to each text line. An example test that uses more fuel than `--listen` gives a request (10,000,000) gets a note, so a handler's logic tested here is not first stopped in production:

```
$ kio test app.kio --verbose
ok   handles  (fuel 14,006,746; peak memory 1728 KB)
note: handles used 14,006,746 fuel; --listen gives each request 10,000,000 (raise it with --fuel)
ok   parses  (fuel 6,739; peak memory 1728 KB)
2 passed, 0 failed
```

### A fake host

In the grants file, `--http hook=fake:hook.http` binds `hook: Http` to the answers in `hook.http` (a path relative to the working directory) instead of a server; nothing is sent. `--http-methods` and `--http-header` apply as to a real grant, and the host's own checks run first: a method not granted or a bad path is `Denied`, a bad header `Invalid`, a body over 8 MB `TooLarge`. The header's value never reaches the fake.

```
# hook.http: one entry per response, `---` between.
# METHOD /path, then a status or `timeout`, response headers, a blank line, the body.
POST /v1/events
503
---
POST /v1/events
200
content-type: application/json

{"ok":true}
---
POST /v1/slow
timeout
```

- An entry answers a request with the same method and path (query included). The n-th entry for one `METHOD /path` answers the n-th such request and the last repeats, so `503` then `200` tests a retry; `send_all` counts in list order.
- `timeout` is `Err(Timeout)` at once. A 3xx is the response (`send`) or `Err(Status(3xx))` (`get`): never followed. `get` turns any non-2xx into `Err(Status(n))`.
- No entry is `Err(Denied)`, as a real grant refuses a path outside it. A failing test lists the requests that had none: `hook: 3 requests, 1 unmatched: POST /v1/evnts` (`--json`: `unmatched`).
- The file is read once, before the first test: UTF-8, at most 1 MiB, a regular file with no other hard links, not inside a writable `--dir` nor a file of a `--log`, `--kv` or `--db` of the same grants file (a database's `-wal`, `-journal` and `-shm` included; "give kio a fake file of its own"). Blank lines and `#` lines before an entry's request line are skipped; an entry's path follows a request's rules (one leading `/`, no space, `\`, `#` or control character); header names are lowercased; a body runs to the next `---` line, its last newline dropped; a line's trailing `\r` is dropped, so a CRLF file answers as an LF one. Each error names its line (`hook.http:7: expected METHOD /path (GET, HEAD, POST, PUT, PATCH, DELETE or OPTIONS)`, `expected a status 100-599 or timeout`, `expected name: value`), and an entry for a method the grant lacks is refused (`hook.http:3: PUT /x: --http hook grants only GET, POST`).
- Each request counts toward `--calls` and `--io`, as a real one.

A test reads back what it sent: `--http-record hook=sent` in the same grants file writes each request on `hook` into the test's default temporary `sent: Dir` before its answer returns, as `hook-1`, `hook-2`, .. (numbered per test from 1; `send_all` in list order). A record is the request as HTTP writes it, byte-exact (a binary body too): `METHOD /path`, the program's headers (names lowercased) then each injected one as `name: <injected>`, never its value, a blank line, the body.

```
POST /v1/events
content-type: application/json
authorization: <injected>

{"id":1}
```

```
#[test]
fn deliver_retries(hook: Http, sent: Dir) -> Result<(), IoErr> {
    deliver(hook, Event { id: 1 });                          // hook.http: 503, then 200
    assert_eq(sent.list("")?.len(), 2);
    let second = sent.read("hook-2")?.to_str();
    assert(second.starts_with("POST /v1/events\n"), "method and path");
    assert(second.ends_with("\n\n{\"id\":1}"), "body");
    Ok(())
}
```

- A request with no entry is recorded (it was sent); one the host refused before the lookup (a method not granted, a bad path, header or body, a `send_all` over 64) is not.
- A record's bytes beyond the body (request line, headers, blank line) are charged to `--io` before it is written, so a run's records stay within `--io` (the body is charged once, as sent); a record is not a call. Each is a new file: a `hook-2` the program made first stops the run ("cannot write `hook-2`: File exists"). A test with `hook` but no `sent: Dir` runs without a record.
- Refused: a `--http-record hook=snet` no test can take ("no test takes both `hook: Http` and `snet: Dir`"); a `hook` that is not a fake ("hook is not a fake", hint "a real grant's requests are in the audit log (--audit)"); a file that also grants `--dir sent` ("records go to a fresh temporary Dir", hint "remove --dir sent"); one grant recorded twice; `--http-record` on `kio test`'s command line, and in `kio run`, `--listen`, `kio check --grants` and `kio review --grants` ("`--http-record` is not a flag of kio run").

### Properties

A test with data parameters is a property ([language.md](language.md#properties)). It runs on `--cases` generated inputs (default 100, at most 100,000) from `--seed` (default 1); case `i` depends only on the seed and `i`, so a run repeats exactly. Each case runs compiled, through an entry that calls the test with the case's values built in IR (never printed and reparsed), in a fresh instance with fresh grants (a new temporary `Dir` per case) under the limits. A failing case is shrunk, each candidate another compiled run (at most 200, keeping the failure's kind; "shrinking stopped after 200 runs" when the cap ends it), and the smallest runs once more. kio's own CI also runs every case its tests generate in the interpreter and fails on any difference; `kio test` itself has no interpreter.

A property may not take `Http` ("a property runs many times; `api: Http` would repeat real requests"; for a fake, "a property may not take `api: Http` yet; a fake's answers are counted per test, not per case"; hint "test the call in an example test, or pass the data as a parameter"), nor a `Dir`, `Kv` or `Db` the grants file names unless it is `:ro` (or `:memory:`), nor a `Log` to a file (`--log log=-` is allowed). A failing input is shown clipped at 4,096 bytes and `...`: the whole line in text, the literals together in `--json`. More than 90% of cases discarded by `assume` fails it ("`t` discarded 95 of 100 inputs").

A generated `Str` is printable ASCII (space to `~`), with about one character in ten from `é ß λ ж 中 日 😀 🦀`; it never holds a control character (`\n`, `\r`, `\t`, `\0`) or any other character. A property will not find a bug that only those trigger: test them in example tests (`"two\nlines"`).

```
$ kio test app.kio
ok   reverse_twice  (100 cases compiled; seed 1)
FAIL small  (4 cases compiled; seed 1)
  minimal input: n = 6, o = Order { id: "", total: 0 }
  shrunk in 44 compiled runs
  app.kio:13:5: assertion failed: too big
  rerun: kio test app.kio --filter small --seed 1
1 passed, 1 failed
```

`--json` adds to a property's entry `cases`, `compiled`, `seed`, `discarded`, `shrinks` and `input` (each parameter's literal, or `null` when it passed). Each case compiles the whole program again, so its cost grows with the program: measured on an Apple M3 Max, release build, about 31 ms a case for a 20-line file, 45 ms for kio-web's flow (515 lines and its library), 52 ms for a 1,200-line app; 100 cases take 3-5 s, a shrink that hits the cap up to 200 cases more. Tests run one after another; use `--filter` while editing.

## Reviewing authority in a pull request

`kio check NEW --base OLD` diffs the two programs' reports: what a reviewer approves. The two sides are usually one file at two commits: `--base` takes a path, not a git revision, so write the old version out first (`git show main:app.kio > /tmp/base.kio`, then `kio check app.kio --base /tmp/base.kio`), or, for a program with modules, use `tools/authority-diff.sh` below. One line per grant (`main` parameter) whose authority changed, with the methods `main` reaches on it through any function: `+` a new grant, `~` methods gained (`+m`) or lost (`-m`) on one of the same name and type, `-` a removed grant (a changed type is one removed and one added).

```
$ kio check app.kio --base old/app.kio
~ req: Incoming (+body)
~ db: Db (+query)
+ stripe: Hmac (verify)
authority added (exit 3); --detail lists each function
```

`--detail` adds a blank line, then each function's capability methods added (`+`) or dropped (`-`). A function on one side only counts in full, so one moved into a module shows as removed and added. The function lines are shown without `--detail` when the grant lines show no addition, as when a helper starts calling a method `main` already reached elsewhere (`+ summarize: fx get`). Only functions `main` reaches count: one no run can call (a test helper, an unused library function) adds no authority, and one `main` starts or stops reaching counts in full.

```
$ kio check app.kio --base old/app.kio --detail
~ req: Incoming (+body)
~ db: Db (+query)
+ stripe: Hmac (verify)

+ check_signature: stripe verify
+ handle: req body, db query, stripe verify
+ main: req body, db query, stripe verify
+ store: db query
authority added (exit 3)
```

A gained method is not new authority when a method the base reached on the same grant already covers it: `write` covers `create`; `read` covers `exists`; `send V` and `send_all V` cover each other (`send_all V`: every request that call sends is V, so a request built but never sent, or sent on another grant, covers nothing), `send *` and `send_all *` cover every method and `get`, `send GET` covers `get` (a new method is never covered by another, and `*` never by literal ones: `~ api: Http (+send PUT -send_all POST)` adds); `scan` covers `get` and `keys`; `exec` and `transaction` cover each other; `sign` covers `verify`; `WsSend`'s `text` and `binary` cover each other and `close`. The line says so and adds nothing; the other direction (`create` to `write`) is added. A function line applies the rule against that function's base methods on the same parameter (same name and type). A function new to the diff counts in full, so a covered method moved into a new helper shows on that helper's line as added.

```
$ kio check app.kio --base old/app.kio
~ out: Dir (+create -write); create is covered by write
no authority added
```

Exit 0: no authority added (`no change in authority`, `authority removed only`, or `no authority added` when every gain is covered). Exit 3: some added. Exit 1: a side does not check (the message names `base` or `new`). Exit 2: a file is missing.

Each side's `mod NAME;` lines resolve to `NAME.kio` beside that side's entry (or where that side's `kio.deps` says), so an `old/app.kio` copied alone fails with `mod web: cannot read old/web.kio`: copy its modules beside it, or let `tools/authority-diff.sh` check the whole base out.

`tools/authority-diff.sh BASE_REF ENTRY.kio [--detail]` (in the repository, not in the release archives: copy it into yours) checks `BASE_REF` out with `git worktree add` into a temporary directory and diffs `ENTRY.kio` against it (`KIO` names the binary). In CI, as a GitHub Actions job step that fails a pull request adding authority until someone with the right to approve it does: It checks out only this repository: a `kio.deps` path outside it must exist at the same relative place for the base side too.

```yaml
- uses: actions/checkout@v4
  with: { fetch-depth: 0 }
- name: Authority diff
  run: tools/authority-diff.sh origin/${{ github.base_ref }} app.kio
```

## Reviewing a change

`kio review NEW --base OLD` puts the evidence kio has about a change on one page. A walkthrough from getting the two versions to a decision: [review-a-change.md](review-a-change.md). Every line carries its label: `enforced` (the host refuses otherwise), `derived` (computed from the code by the checker; the host enforces per grant, not per function), `tested` (passed on generated or written cases), `measured` (deterministic numbers), `not checked` (a person must look or accept it), `error` (a failure; exit 1); a line under a labelled one shares its label. It never gives a verdict.

Review runs the change's own test code, OLD's and NEW's, under `--test-grants T` or else the defaults (a temporary `Dir`, `:memory:` stores, a fixed clock); never under `main`'s grants.

```
$ kio review new/app.kio --base old/app.kio --requirements README.md
Requirements
  not checked  what the change claims; kio does not check it
               --- old/README.md
               +++ new/README.md
               @@ -1,3 +1,4 @@
               ...
Authority
  enforced     ~ d: Dir (+write)
  derived        write in main
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 4 passed, 0 failed
  tested       new: 4 passed, 0 failed
  tested       added: fresh
  not checked  removed: gone
  not checked  changed: counts
  not checked  changed, reached by no test: main
  tested       small: 100 cases compiled; seed 1
Resources
  measured     counts: fuel 57753 -> 107753 (+86.58%), peak memory 1728 KB -> 1728 KB (+0.00%)
  measured     added: fresh: fuel 7853, peak memory 1728 KB
  measured     total over 3 tests on both sides: fuel 845756 -> 895756 (+5.92%), largest peak memory 1728 KB -> 1728 KB (+0.00%)
Not checked
  not checked  d: Dir write: what is written or removed (changed: main)
  not checked  tests and properties were written by the same author as the code unless stated
Identity
  kio 0.1.4 (<commit>)
  old old/app.kio sha256:9e33a9...
  new new/app.kio sha256:1eccc5...
  ...
  test seed 1
```

- **Requirements** (`--requirements FILE`, a relative path, beside each side's entry, at most 1 MB): the file's unified diff, what the change claims.
- **Authority**: `kio check NEW --base OLD`'s lines: grant lines `enforced`, function lines (`+ flow::init: db exec`) `derived`. Under each `+` or `~` grant line, one `derived` line per gained method naming the functions `main` reaches in NEW that use it and did not in OLD (`  query in flow::store`; `main` only when no other function does). Exit 3 with no grant added says so first: `derived  grants unchanged; new functions reach: flow::init` (each function with a `+` line; `no grant added; ..` when a grant line only removes). `--grants G` is part of the change: its flags against the file at the same path beside OLD's program, `enforced  app.grants: + --dir rejects=rejected` (`- ..`, `unchanged`, or `new file` when OLD has none; `not checked  G: outside the program's directory; ..` when G is not under NEW's directory). OLD's tree may be anyone's: its file is read only if it is a regular file of at most 64 KiB with no other hard links, inside OLD's directory and reached through no symlink; else `not checked  app.grants: the base's app.grants is not a regular file inside its directory; not compared` (or `is over 64 KiB`), and it is not hashed. When OLD's program is in NEW's directory the file there is NEW's own: `not checked  app.grants: the base shares this directory; not compared`. A grants file over 1 MiB is refused, here and by `kio run --grants`. Then G binds to NEW's `main` as `kio check --grants` does: `enforced  --grants G binds NEW's main: db=var/inv.db:ro, hook=http://127.0.0.1:9099 (POST)`, or the errors labelled `error`. The binds line lists each grant in G's order as written (`name=value`), an `Http` grant's `--http-methods` and `--http-header`s in parentheses after it, and `--listen ADDR`, `--host NAME` and `--ws-origin O` (who may reach the program); limits and `--audit` are left out. It is ASCII: `\` is doubled and every other character escaped (`d=./data:r\u{43e}`, a Cyrillic `о`, does not read as `:ro`), and a part holding `,`, `;`, `(`, `)`, `"` or a space is quoted. A secret (`--hmac`, `--secret`, a header's value) shows only its source, `env:NAME` or `file:PATH`: binding refuses any other form. `--entry worker.kio` (repeatable; a relative path without `..`, beside each side's root) adds another entry's lines after its name, `enforced  worker.kio: + dest: Http (send POST)`; one OLD does not have is new, all its authority `+`. `--entry-grants G2` right after it binds G2 to that entry's `main`, compared with OLD's as G is. Tests stay the root's.
- **Tests**: `kio test` on both sides with T (else the defaults) and seed 1: the grants used (each [fake](#a-fake-host) T gives named after them, `grants: test.grants (hook answered from hook.http)`; `--json` `tests.fakes`, `[{"grant", "path"}]`; the name and FILE ASCII-escaped and quoted as on the binds line), counts (`0 tests` for a side with none), each test added or removed by name, each test on both sides that changed or reaches a function that changed (`not checked  changed: NAME`, `changed: NAME (reaches changed count)`: read it like an added one; a function changed when its tokens differ, so whitespace outside string literals and comments do not count; a new function counts; a changed `const` or type is not traced), the changed functions `main` reaches that no test reaches (`not checked  changed, reached by no test: deliver, main`, statically from every test as `reached only from tests:` is; an entry's after its name, `.. no test: worker.kio: main`, where it counts as reached when the root's tests reach a function of its name and every function it may run, and every constant and type those use, has the same tokens in both programs; reach is static, so a function named on a test's call path counts even if that path never runs; `--json` `tests.unreached_changed`, `[{"entry", "function"}]`), each property's cases, and each NEW failure as `kio test` prints it (`error`; an added test that fails is `error  added: NAME`). OLD's own failures are `not checked` (`old: 2 passed, 1 failed; the base fails its own tests`, `failed in old: NAME`): they back nothing, and do not fail the review. A secret T names that cannot be read (`--hmac key=env:KEY` with `KEY` unset) is one `error` and no test runs. A removed test, a property that ran fewer cases than in OLD, or a property now an example test is a `removed:` line (`not checked`: a person accepts it). A test that failed on either side stopped early: it is left out of these lines and of Resources (`2 tests failed in new; left out of Resources`, counting every NEW failure, added tests and properties included; `1 test failed in old only; ..`). T may not give a writable `--dir` or a file-backed `--kv` or `--db`: OLD's tests would leave their writes for NEW's (use `:ro` or `:memory:`). Nor a `--dir`, `:ro` too, that holds one of NEW's files (root, module, `kio.deps` or an entry's; by identity, as H28 decides it, so a symlink to the directory or a symlinked directory on the way counts), nor any `--dir` while one of those files has other hard links: a test could read the code under review instead of running it (`--test-grants t.grants: `--dir src=.:ro` holds NEW's source `new/app.kio`; ..`, hint: grant a directory that holds no program file, e.g. `--dir data=testdata:ro`). Each `--http` name in T must be an identifier, and a fake FILE must be inside NEW's directory, so the change shows it ("keep the fake file beside the program it tests").
- **Mutants** (in Tests): kio edits NEW's text in the changed functions a `main` reaches (an entry's too, as the root's function of its name when it runs as the root's: the same tokens in every function it may run and every constant and type they use) and a test reaches, one token at a time: `<` and `<=`, `>` and `>=`, `==` and `!=`, `&&` and `||` swapped where the parser found that operator (`LIMIT > n` is one, `Vec<Int>` is not), `true` and `false`; an integer literal plus 1; a string literal emptied; `Some(x)` to `None`, `Ok(` to `Err(`, `is_ok`/`is_err` and `is_some`/`is_none` swapped; a statement dropped. Never in a test, a dependency, a grants file, `kio.deps` or a constant (`not checked  constants are not mutated: LIMIT used by over`), and never in OLD. `--mutants N` (default 20, at most 500, 0 for none) are drawn by `--seed S` (default 1, which is also both sides' tests' seed), so the same seed gives the same page: each function's sites and the functions are shuffled, then taken one per function in turn, so a long function cannot take every draw; one that gets none is `not checked  no mutant drawn: pad (80 sites)`. Each mutant is checked as NEW is (one that does not is counted, not run), then runs the tests that passed on NEW and reach it, examples first, under T as every test (fresh sandboxes; never `--grants G`), each at 10x the fuel it used on NEW, until one fails; a property stops at its first failing case, unshrunk. A test T gives a real host, the system clock or OS randomness runs no mutant, since its answer could change from run to run: `not checked  mutants skipped zero_sends_nothing: it holds a real host (h), which can tell runs apart; use --http h=fake:FILE`. Lines: `tested  mutants: 15 of 18 killed (generated at seed 1: 20 drawn of 43 sites, 2 did not check)`, `tested  killed by: tests::a_claim_needs_the_job_still_due_and_unchanged 7, ..` (a kill that was not a failed assertion, `Err` or run-time error is named: `counts 5 (1 out of fuel)`; `timeout; machine-dependent`), and per survivor ``not checked  survived: generated: flow::headers: `"Content-Type: application/json"` -> `""` in `..` ``: no test noticed that edit, the snippet ASCII-escaped as on the binds line. Some survivors change nothing (an equivalent mutant); read each. Two endings are not kills: a stop at T's own `--fuel` (when it, not 10x, was the cap), `--timeout`, `--memory`, `--stack`, `--calls` or `--io`, counted apart (`mutants: 4 of 6 killed, 1 stopped by T's limits`, `not checked  stopped by T's --fuel 11753 in counts: ..`), and a property that discards every case (`survived: .. (kept discarded every case)`). A survivor does not change the exit code; mutant runs past 300 s together, or one that fails inside kio, are `error  mutants: 3 of 20 ran; budget exhausted (300 s)`, exit 1. `--json`: `tests.mutants`, `{seed, requested, sites, drawn, made, did_not_check, error, skipped, constants, rows}`, a row `{source, function, file, line, from, to, description, killed_by, stopped_by, discarded_by, how, tests_run, fuel}`. Resources are the unmutated runs only.
- **Written mutants** (`--mutations FILE`, in Tests): the mutants a reviewer asks for, run as generated ones are, before them. An entry is a function as the page names it, what the mutant breaks, and one edit; the `-` text must occur exactly once in that function in NEW (no line numbers, so a reflowed file keeps the entry; text that is gone means the code it pinned changed):

  ```
  # flow.mutants
  flow::finish: off by one on the limit, so a job gets one attempt too many
  - job.attempts >= limit
  + job.attempts > limit
  ---
  worker.kio: deliver: any status counts as delivered
  - resp.status >= 200 && resp.status < 300
  + true
  ```

  The function must be one `main` reaches (a test, or a helper only tests call, would pad the count). `worker.kio: deliver` mutates that `--entry`'s program (`--entry ./worker.kio` matches too) and runs its tests that reach `deliver`. `+` alone removes the text; `---` and `#` lines are skipped. The headline counts each source apart: `mutants: 19 of 22 killed (generated 15 of 18 at seed 1: 20 drawn of 43 sites, 2 did not check; flow.mutants 4 of 4)`. A written mutant no test kills is ``not checked  survived: flow.mutants:17: flow::finish: `job.attempts >= limit` -> `job.attempts > limit`, ran 3 tests: "off by one on the limit, .."`` (the edit first, the description quoted, so it cannot pass for an edit) and **exit 3**: the reviewer asked for exactly this to be caught. One stopped by a limit T sets is `error`, exit 1 (`the mutant was stopped by T's --calls 3 in probes, ..`, hint: raise that limit): it neither passed nor failed. An entry that cannot run is `error`, exit 1, with a hint: ``the `-` text occurs 2 times in flow::finish`` (lengthen it), ``the `-` text is not in F``, `NEW has no function F`, `worker.kio is not an --entry of this review`, `F is not reached from NEW's main`, `F is in a dependency`, `the mutant does not check: ..`. FILE is read once as OLD's grants file is (a regular file by its own name, no symlink or second hard link, at most 64 KiB; one that does not parse is a usage error, exit 2), hashed in Identity (`mutations flow.mutants sha256:..`; changed by the end of the run: `error`), and noted `(inside NEW's tree)` on both lines when it sits in NEW's directory by its path as given or its canonical path (a symlinked directory in NEW's tree counts), since then the change's author could have written it. Everything shown from it is ASCII-escaped. `--json`: `tests.mutants.mutations` `{path, inside_new, entries, errors: [{message, hint}]}`, every row has `written` (`false` for generated), written rows `source` (FILE's path, which may be any name, `generated` included), `entry` (its line) and `program`; `identity.mutations` `{path, sha256, inside_new}`.
- **Resources**: per test on both sides, fuel and peak memory (a property's: fuel summed over its cases, peak memory their largest) when either changed by more than 2%; each test only in NEW that passed, `added: NAME: fuel N, peak memory M KB` (`--json` `resources.added`); then the total fuel and the largest peak over the tests on both sides. Percentages are rounded away from zero, so a row never reads `2.00%`. Tests only; no wall time. Each run starts by setting up a table of every type the program uses, `main`'s included, so a type a test never touches still costs it about 150 fuel: adding an `Http` call to `main` (its `HttpResponse`, `HttpErr`, ..) moves every test by about 1,000 fuel, and a small test shows it as several percent.
- **Not checked**: for each method NEW's `main` (and each `--entry`'s, after its name) reaches, what the host does not look at (the SQL, the bodies sent, what is written, ...), ending `(changed: flow::store)` with the changed functions (as for tests) that the functions using those methods reach, themselves included (`main` only when no other function uses them, and then everything it reaches); a function uses a grant's methods when the grant is passed to it, so with two grants of one type each line names only its own; each dependency added, removed or changed (`dependency web changed (sha256:e4aad9a99560.. -> sha256:91c0a1b2c3d4..): its code is not shown; its authority is above`), compared per program (each its own `kio.deps`), an entry's after its name (`worker.kio: dependency web changed ..`); one added or changed exits 3, as authority added does; each fake T gives, `hook: Http: tests ran against hook.http, not a server; the real endpoint's answers are untested` (`not_checked.fakes`; escaped as on the grants line).
- **Identity**: kio's version and commit, and the sha256 of every file each side loads (root, modules and dependencies, and a `dep NAME PATH sha256:..` line per dependency), of G and OLD's file beside it (`old grants`), of T and each fake FILE it names (`test fake hook.http sha256:..`, `identity.test_fakes`, escaped as on the grants line; read once, both sides' tests answered from the same bytes; a FILE `kio test` refuses has no line), of each `--entry-grants` (and OLD's), and of the requirements files; a module two entries share once. Each is read again at the end; one that changed while the review ran is an `error`.

Exit 1: a side or entry does not check, G or an entry's grants do not bind, T cannot be applied, a side's tests do not run, a NEW test fails, the mutant runs did not finish, or a file changed during the review; 3: authority added, a dependency added or changed, or a test removed; 0 otherwise; 2: usage. `--json` gives one object with a field per section. `--base` takes a path: for a git revision, write each file out with `git show REV:app.kio > old/app.kio`, modules beside it.

## Compiling

Every `kio run` compiles the program to native code in memory (about 20-25 ms of a hello run's 30 ms); nothing compiled is kept on disk or loaded from it, since native code loaded from a file a program could write would run as the host. `--listen` compiles once at startup. Earlier versions kept compiled programs in `~/.cache/kio` (or `$XDG_CACHE_HOME/kio`); kio no longer reads that directory, and you can delete it.

## Built-ins

`Json` (`s.parse_json()` gives `Result<Json, Str>`, `j.to_json()`, accessors `get`, `at`, `as_str`, ..), `HashMap<K, V>` (insertion-ordered, beside the key-ordered `Map`), generic functions and types with `T: Eq` and `T: Ord` bounds, `<` on `Str` by bytes, `sort_by`, `join`, `sha256`, `to_hex`, `from_hex`, `try_to_str` (`None` for bytes that are not UTF-8) and `ct_eq` (constant-time compare). Every method: [language.md](language.md).

## From source

`cargo test` runs every suite; `cargo run -p kio -- run file.kio` runs without installing. `examples/tour/demo.sh` walks through the report, a rejected helper, a refused escape and a fuel stop.
