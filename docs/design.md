# Design

## Three layers of assurance

| Layer | Question it answers | Where | Status |
|---|---|---|---|
| Static: checker and report | What can this program touch? | `lang` | Done: `main : input read, out create` |
| Enforced: host and limits | Can it touch anything else? | host, Wasmtime | Done: G1 to G5, fuel and every quota |
| Recorded: audit log | What did it touch, this run? | host | Done: `kio run --audit log.jsonl`; under `--listen`, one log with each line's request number |

Each layer holds without the one above it. The host enforces every guarantee even if the checker is wrong; the log records every call even if the program is hostile, because the host writes it. The log also checks the checker: every logged call must appear in the report, so a call the report did not predict is flagged as a checker bug.

## Components

```
 Agent writes          HOST (Rust, trusted)                              SANDBOX (Wasmtime)
┌──────────────┐   ┌───────────────────────────────────────┐   ┌────────────────────────────┐
│ .kio source: │──▶│ check ─► report ─► lower ─► IR        │   │ program.wasm + runtime     │
│ records, fns,│   │   │ accept/reject         └─► codegen ─┼──▶│ (refcounts, Str, List,     │
│ cap params   │   │   ▼ errors back to agent    + Cranelift│   │ Map), as native code       │
└──────────────┘   │                                       │   │                            │
                   │ host functions: dir.read, http.get,   │◀──┼─ the only way out: imports │
                   │ send, trap; check every handle, path, │   │   from caps.wit            │
                   │ quota; write the audit log            │   └────────────────────────────┘
                   └───────────────────────────────────────┘
```

| Component | Trusted | Role |
|---|---|---|
| `.kio` source | No | Business logic. Capabilities are opaque values that can be passed and called, never created. |
| Checker | Yes | Accepts or rejects the program against the capability table and emits the authority report. Runs in the host (decided 2026-11-06), once per run or once per server. |
| Lowering and codegen | Yes | Turn the checked program into an IR, then Wasm, linked with the runtime. A bug can give a wrong answer, not an escape: the sandbox and the host still enforce every grant. |
| Runtime | No more than the program | Refcounted values, strings, collections, `Json` and `HashMap` (one table source, lang/src/hmap.rs, shared with the interpreter), in the same module as the program. Small blocks come from size-class free lists over dlmalloc ([codegen.md](codegen.md) "How it is built" 18, 20). |
| Interpreter | Not shipped | Runs the IR natively in tests only: the reference compiled code must match (differential tests). |
| `caps.wit` | Yes | Declares every function that crosses the sandbox wall. Changing it is a security-review event. |
| Wasmtime | Yes | In-process sandbox. No ambient file, network, env or clock. Fuel, memory, stack and epoch limits. |
| Host functions | Yes | The only code that touches real resources. Each call validates the handle, its scope and quotas. |

Everything runs in one OS process, the `kio` binary. The sandbox is a memory wall inside the process. For production, add OS-level isolation as well: a container or VM, plus seccomp.

## Crates

- `lang`: parser, checker, lowering to IR, interpreter. No I/O, no dependencies. All language tests run natively.
- `runtime`: compiled kio's runtime library, `no_std`, built to one core module (`compiled` world).
- `codegen`: IR to Wasm, appended to the runtime and wrapped as a component.
- `host`: the `kio` binary. Wasmtime, grants, limits, host functions, the audit log, `--listen`, `kio mcp`.
- `tools/capgen` generates the checker's capability table from `caps.wit`; `tools/kiogen` generates programs for differential tests; `bench/` holds the benchmarks.

## Files and who owns them

| File | Written by | Read by | When |
|---|---|---|---|
| `caps.wit` | you, by hand, rarely | build step | at build |
| capability table (Rust) | generated from `caps.wit` by `tools/capgen` (checked in, CI diffs it) | checker, in the host | at build |
| runtime and host Rust bindings | generated from `caps.wit` by wit-bindgen and Wasmtime's `bindgen!` | rustc | at build |
| `prog.kio` | the agent | checker, then compiler, in the host | every run |
| grants (`--dir name=path`) | the operator | host | every run |

Adding a method to `caps.wit` regenerates the capability table, the runtime's import stub and the host trait. The host no longer compiles until the method is implemented. The three cannot drift.

`caps.wit` is one file for the whole toolchain. It says what a `Dir` *can* do. A grant says which directory *this run* gets. The program says which roles it *needs*. Three layers, three owners.

## One run, step by step

```
kio run prog.kio --dir input=/srv/jobs/42/in --dir out=/srv/jobs/42/out
```

| Step | What happens | Where |
|---|---|---|
| 1 | Host reads the files, parses and checks them against the capability table. On reject it prints the diagnostics; nothing has run. | host, native |
| 2 | Host lowers the program to IR, generates its Wasm, appends it to the runtime and compiles the module to native code with Cranelift (or loads that code from the compile cache, `~/.cache/kio`) | host, native |
| 3 | Host opens each granted directory with cap-std and builds the handle table `{0: input, 1: out}` | host, native |
| 4 | Host binds each grant to the `main` parameter of the same name (host/src/grants.rs): a missing, unknown, repeated or mistyped grant is an error and no code runs | host, native |
| 5 | Host instantiates the module with fuel, memory, stack and epoch limits, binds its imports to the host functions, and calls `run([dir(0), dir(1)])`, the grants in parameter order | wall crossed, into the sandbox |
| 6 | Compiled `main` runs. On `input.read("invoices.csv")` it calls the import `dir.read(0, "invoices.csv")` | wall crossed, out of the sandbox |
| 7 | Host looks up handle 0, checks its kind, charges the call quota, checks the call is on `main`'s report line, resolves the path beneath the root, enforces the size limit, reads, returns bytes | host, native |
| 8 | Compiled code continues. Steps 6 and 7 repeat per capability call. A runtime error calls the `trap` import with its site; the host turns it into the error's message and span. | sandbox |
| 9 | `run` returns `main`'s value printed. The host prints the report to stderr (when `main` takes capabilities) and the value (nothing for `()` or `Ok(())`; `Err(e)` as `error: e` on stderr), sets the exit status and drops the handle table. | host |

`kio check prog.kio` is step 1 only. The MCP tool `kio_check` is the same. Under `--listen`, steps 1 and 2 run once at startup; each request or WebSocket event runs steps 4 to 9 in a fresh instance.

## The host

**Handle table.** A per-run vector of opened resources. The guest sees indices. An unknown index or a wrong kind is a denied call. Because the table holds only what was granted, guessing a different index gains nothing.

**Dir.** Opened once with cap-std at grant time. Only regular files are read, created, written or removed; anything else is `denied`. `list` returns regular files and directories only. `write` writes a temp file with an unpredictable name beside the target and renames it over, so readers never see a partial file. Every path from the guest is resolved relative to that open directory, which refuses absolute paths, `..` above the root and symlinks that lead out. On Linux this is `openat2` with `RESOLVE_BENEATH`.

**Http.** A grant names one host (`--http`) and its methods (`--http-methods`; GET always). The host builds every URL itself from the granted origin and the program's path, checks the method and headers before anything is sent (a method outside GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS, a header with a control character or one that frames the connection, such as `host`, `content-length` or `transfer-encoding`, is `invalid`), follows a redirect only for GET and HEAD and only within the origin, refuses IP-literal hosts unless explicitly granted, and pins DNS resolution for the run. Request bodies are capped at 8 MB, responses at 10 MB, a request at 10 s. `send_all` runs its requests on worker threads at once and returns them in list order; the audit log holds the method, path, status, byte counts and header names of each request, never a header value (H16, H16b, [open_questions](open_questions.md) #148-150).

**Kv.** `--kv name=:memory:` or a file: one store per grant, shared by every request; mutations go to an append-only log the host owns and locks, compacted when it grows ([open_questions](open_questions.md) #41-46). `--kv name=path:ro` is a reader: refused at startup to a program whose report shows a write on it, and any write that reaches the host stops the run. It takes no lock; before each `get`, `keys` and `scan` it applies the whole, checksummed records the writer appended since, and replays the file from the start when the path names a new inode (a compaction's rename) or the file shrank; a replay that finds the file corrupt is not repeated until the file changes (#119-122, #139). `scan` pages through a prefix's entries and `add` counts under the store's lock, storing decimal text (K2, #138).

**Rng, Clock, Log.** Time, randomness and a program log are grants like any other, so a run with none is deterministic and one with fixed values (`--rng r=seed:1`, `--clock now=fixed:..`) replays. `Rng`: the OS's bytes (`getrandom`), or ChaCha20 keyed by the seed with the request number as the stream (0 for `kio run`), so a seeded server replays only under the same request order; `int` is rejection sampling in the host, the same for both paths; the guest refuses a count over 65,536 or an empty range as a runtime error at the call, and the host refuses it again. `Clock`: `now_ms` only, coarse by design; every call's value is audited. `Log`: one file opened once per process to append (`O_APPEND`, `O_NOFOLLOW`), one write per line under a lock, every control character escaped, 64 KB a line and 1 MB a run; its path is refused under the audit log's rules (not a store, not inside a granted dir or the cache, not another log's file). The audit log records each call's count, range, value or byte length, never a drawn value or a line (#140-144).

**Db.** `--db name=path`, `path:ro` or `:memory:`: one SQLite connection per grant (the bundled SQLite via `rusqlite`), shared by every request behind a mutex. The SQL is the program's and SQLite runs on the host outside Wasm fuel, so the connection is fenced (host/src/db.rs): an authorizer refuses `ATTACH`, `DETACH`, transactions and savepoints (only `transaction` opens one), every `PRAGMA` but a few reads, `load_extension`, and on `:ro` every write; `SQLITE_DBCONFIG_DEFENSIVE`, no trusted schema, no attached databases (`SQLITE_LIMIT_ATTACHED` 0, which also closes `VACUUM INTO`), SQLite's limits lowered (1 MB of SQL, 8 MB values, 4,096 parameters), a progress handler that interrupts a statement at the run's deadline, a process-wide hard heap limit (256 MB) over the in-memory temp store, and a 2 s busy timeout. Results are capped at 10,000 rows and 8 MB; parameter and result bytes are charged to `--io`. A `:ro` grant is refused to a program whose report shows `exec` or `transaction` on it, and the host stops either call if one arrives. Path rules as a `Kv` file (not a symlink, not inside a granted dir or the cache, not another store's or log's file; SQLite's `-wal`, `-shm` and `-journal` count as the database's). The audit log records the SQL (cut at 256 characters), the parameter count, rows or rows changed and bytes, never a parameter value (#151-155).

**Audit under `--listen`.** One log per server: the header (grants with their kinds, limits) once at startup, then each request's call lines and trailer, every line starting with `"req":N` (numbered as runs start) and, for a WebSocket event, `"conn":N`. Each line is one write under a lock, so concurrent requests never interleave mid-line (#123). A trailer of a run that sent a response has `"after_send":N`, the calls made after it (#147).

**Serving.** `--listen` binds `Incoming`/`Outgoing` (HTTP) or `WsMessage`/`WsSend` (WebSockets) to `main`'s parameters by type, whatever their names. On a loopback bind a request's `Host` must be `localhost`, `127.0.0.1`, `[::1]` or a `--host` name (DNS rebinding); a WebSocket upgrade from a browser needs a same-host `Origin` or `--ws-origin`. The upgrade request's target and headers are kept for the connection's life and read by every event through `WsMessage.target`/`header` (WS1b). `Outgoing.send` hands the response to the client at once; `main` runs on in the same sandbox, under the request's limits and holding its worker, and a failure after the send reaches only stderr and the audit log (H18, #146). `WsSend` frames are written after the run.

**Limits.** Wasmtime fuel bounds instructions. Epoch interruption bounds wall-clock, because fuel does not tick while a host call waits on a slow server. Each host call also has its own timeout and the run has a host-call count and total I/O byte budget.

## Limits

Defaults per run, or per request or WebSocket event under `--listen` (fuel 10 million, 64 MB, 5 s); each can be overridden by a `kio run` flag. A server also bounds slow clients: each connection lives at most 60 s (until it becomes a WebSocket), headers within 10 s (which also closes an idle keep-alive connection after 10 s), the body within 10 s more, headers to about 64 KB (hyper checks after each read, so one large read can reach about 114 KB), 1,000 connections, 100 requests queued. A WebSocket closes on a message over 1 MB (1009), over 64 KB of events waiting (1008), 5 minutes without a frame (1000), or a failed run (1011); its sends are written after each run, within 10 s. Exceeding one stops the run with an error naming the limit.

| Limit | Default | Flag | Bounds |
|---|---|---|---|
| Fuel | 10 billion | `--fuel` | Wasm instructions, so infinite loops end; a typical program uses under 5 million |
| Memory | 256 MB | `--memory` (MB) | guest linear memory |
| Wasm stack | 2 MB | `--stack` (KB, at most 8 MB) | recursion: about 43,000 calls of a small function, 21,000 through a loop |
| Wall clock | 10 s | `--timeout` | slow hosts and long runs |
| Host calls | 10,000 | `--calls` | capability calls; at most 64 timed-out calls may still be finishing at once, process-wide |
| I/O | 100 MB total, 10 MB per read | `--io` | bytes read and written |

## Where the checker runs

Decided 2026-11-06, done in C5: in the host. The host already holds every grant and enforces every limit, so a second check inside the sandbox adds no safety, and checking there cost the sandbox's memory and fuel on every request (open_questions #33). The sandbox receives compiled code only. Adversarial source is then parsed outside the sandbox, so the front end must never panic or recurse without bound: the host runs it on a large stack, and the program generator (compiler C6) fuzzes it.

The host never relies on the checker for safety. The handle table and host functions enforce every guarantee on their own. The checker exists for early, readable rejection and for the authority report.

## Where state lives

| Lifetime | Where | How a program reaches it |
|---|---|---|
| One run or request | guest memory | `let mut`, `List`, `Map`: the language |
| Across requests, or across messages on one connection | host memory | `Kv` granted `--kv name=:memory:`: one map per grant, shared by every request (K1); `Db` granted `--db name=:memory:` (H17) |
| Across restarts | a file, a directory or another service | `Kv` granted `--kv name=path` (an append-only log the host owns and locks; other processes may read it with `path:ro`), `Db` granted `--db name=path` (a SQLite file; `path:ro` for a reader), `Dir` (flat files), or a network capability |

Only the first is in the language. Every other tier is a capability: named in `main`, granted by the operator, enforced and logged by the host, like any other I/O. A fresh sandbox per request is what makes the second tier necessary; other languages keep it in process globals, which kio does not have.

## Memory model

Value semantics without a garbage collector or lifetimes. The interpreter is the reference: later optimizations, and the compiler, must not change what a program observes; the tests that pin copy behavior and the differential harness are what make them safe.

| Step | Change | Observable behavior |
|---|---|---|
| v0 | every assignment, argument and container read clones | reference |
| copy on write (done) | values share storage with a reference count; any write to shared storage, including `x.f = v` and `xs[i] = v`, copies it first | unchanged |
| last-use moves, borrowed parameters (done, C8b2, C8b4) | a variable's last use moves its storage instead of sharing it; a callee that only reads a parameter borrows it | unchanged |
| `&mut` parameters | a callee changes the caller's value; shared storage is still copied first | new feature |

No stored references and no shared mutable objects means no cycles, so reference counting never leaks. Compiled code uses the same model: the runtime holds the counts, codegen inserts the increments and decrements ([codegen.md](codegen.md), Ownership), and every differential test checks that no object is left alive.

`&mut` rules:
- **No escape.** Parameters only: never stored, returned or put in a container. It cannot outlive the call.
- **No overlap.** First version: an `&mut` argument is a whole local or a forwarded `&mut` parameter, and all arguments in one call have distinct roots. Field and index arguments (`&mut xs[i]`) wait until overlap checking is defined.

Linked structures use indexes into a `List`. Copies stay implicit, as in Swift's value types. Hylo's explicit-copy default is not adopted, because switching later would change existing programs.

Lowering turns a checked `Program` into `ir::Module`, with every name resolved to an index; the interpreter and the compiler both run it.

## Open questions

- **Attenuation.** `dir.sub("invoices")` would let `main` hand a helper less than it has. It conflicts with "caps are parameters only" because it returns a cap. Options: allow it only in argument position, or make the host mint sub-handles. Deferred to v1.
- **Relaxing second-class caps.** Storing caps in records would need flow analysis in the checker. Not until there is a program that needs it.
- **Grant format.** CLI flags for v0. A manifest file once there are more than a handful of grants per run.
