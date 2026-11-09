# Design

## Three layers of assurance

| Layer | Question it answers | Where | Status |
|---|---|---|---|
| Static: checker and report | What can this program touch? | `lang` | Done: `main : input read, out create` |
| Enforced: host and limits | Can it touch anything else? | host, Wasmtime | Done: G1 to G5, fuel and every quota |
| Recorded: audit log | What did it touch, this run? | host | Done: `kio run --audit log.jsonl` |

Each layer holds without the one above it. The host enforces every guarantee even if the checker is wrong; the log records every call even if the program is hostile, because the host writes it. The log also checks the checker: every logged call must appear in the report, so a call the report did not predict is flagged as a checker bug.

## Components

```
 Agent writes          HOST (Rust, trusted)                              SANDBOX (Wasmtime)
┌──────────────┐   ┌───────────────────────────────────────┐   ┌────────────────────────────┐
│ .kio source: │──▶│ check ─► report ─► lower ─► IR        │   │ compiled: program.wasm +   │
│ records, fns,│   │   │ accept/reject         └─► codegen ─┼──▶│ runtime (refcounts, Str,   │
│ cap params   │   │   ▼ errors back to agent    (C5: default)  │ List, Map), native code    │
└──────────────┘   │                                       │   │                            │
                   │ today `kio run`: source ──────────────┼──▶│ interpreter guest (lang)   │
                   │                                       │   │                            │
                   │ host functions: dir.read, http.get,   │◀──┼─ the only way out: imports │
                   │ send, trap; check every handle, path, │   │   from caps.wit            │
                   │ quota; write the audit log            │   └────────────────────────────┘
                   └───────────────────────────────────────┘
```

| Component | Trusted | Role |
|---|---|---|
| `.kio` source | No | Business logic. Capabilities are opaque values that can be passed and called, never created. |
| Checker | Yes | Accepts or rejects the program against the capability table and emits the authority report. Runs in the host (decided 2026-11-06); today the interpreter guest also checks what it runs. |
| Lowering and codegen | Yes | Turn the checked program into an IR, then Wasm, linked with the runtime. A bug can give a wrong answer, not an escape: the sandbox and the host still enforce every grant. |
| Runtime | No more than the program | Refcounted values, strings and collections, in the same module as the program. |
| Interpreter | No more than the program | Runs the IR inside the sandbox: `kio run` today, and the reference the compiler is tested against. |
| `caps.wit` | Yes | Declares every function that crosses the sandbox wall. Changing it is a security-review event. |
| Wasmtime | Yes | In-process sandbox. No ambient file, network, env or clock. Fuel, memory, stack and epoch limits. |
| Host functions | Yes | The only code that touches real resources. Each call validates the handle, its scope and quotas. |

Everything runs in one OS process, the `kio` binary. The sandbox is a memory wall inside the process. For production, add OS-level isolation as well: a container or VM, plus seccomp.

## Crates

- `lang`: parser, checker, lowering to IR, interpreter. No I/O, no dependencies. All language tests run natively.
- `guest`: `lang` built to `wasm32` as the interpreter component (`guest` world).
- `runtime`: compiled kio's runtime library, `no_std`, built to one core module (`compiled` world).
- `codegen`: IR to Wasm, appended to the runtime and wrapped as a component.
- `host`: the `kio` binary. Wasmtime, grants, limits, host functions, the audit log, `--listen`, `kio mcp`.

## Files and who owns them

| File | Written by | Read by | When |
|---|---|---|---|
| `caps.wit` | you, by hand, rarely | build step | at build |
| capability table (Rust) | generated from `caps.wit` by `tools/capgen` (checked in, CI diffs it) | checker, inside the guest | at build |
| guest and host Rust bindings | generated from `caps.wit` by wit-bindgen | rustc | at build |
| `prog.kio` | the agent | checker, then interpreter (compiler from C5) | every run |
| grants (`--dir name=path`) | the operator | host | every run |

Adding a method to `caps.wit` regenerates the capability table, the guest import stub and the host trait. The host no longer compiles until the method is implemented. The three cannot drift.

`caps.wit` is one file for the whole toolchain. It says what a `Dir` *can* do. A grant says which directory *this run* gets. The program says which roles it *needs*. Three layers, three owners.

## One run, step by step

Today `kio run` uses the interpreter guest, as below. Compiled runs (the default from C5) differ in steps 3 to 6 and 9: the host checks, lowers and compiles first, the guest's `run` receives the grants in `main`'s parameter order, and compiled code, not the interpreter, makes the import calls.

```
kio run prog.kio --dir input=/srv/jobs/42/in --dir out=/srv/jobs/42/out
```

| Step | What happens | Where |
|---|---|---|
| 1 | Host opens each granted directory with cap-std and builds the handle table `{0: input, 1: out}` | host, native |
| 2 | Host instantiates the guest with fuel, memory and epoch limits, and binds the WIT imports to its host functions | Wasmtime |
| 3 | Host calls `run(files, [{input, 0}, {out, 1}], [], [], [])` | wall crossed, into the guest |
| 4 | Guest parses and checks against the embedded capability table. On reject it returns the diagnostics. No I/O has happened. | guest |
| 5 | Guest binds each grant to the `main` parameter of the same name; a missing or mistyped grant is an error and no code runs | guest |
| 6 | Interpreter starts `main` with `Cap(Dir, 0)` and `Cap(Dir, 1)` | guest |
| 7 | On `input.read("invoices.csv")` the interpreter calls the import `dir.read(0, "invoices.csv")` | wall crossed, out of the guest |
| 8 | Host looks up handle 0, checks its kind, charges the call quota, resolves the path beneath the root, enforces the size limit, reads, returns bytes | host, native |
| 9 | Interpreter continues. Steps 7 and 8 repeat per capability call. | guest |
| 10 | `main` returns. The guest returns the report and `main`'s value; the host prints the report to stderr (when `main` takes capabilities) and the value (nothing for `()` or `Ok(())`; `Err(e)` as `error: e` on stderr), sets the exit status and drops the handle table. | host |

`kio check prog.kio` calls the `check` export: steps 3 and 4 only, no handles. It returns the report or the diagnostics, with spans and hints. The MCP tool calls the same export.

## The host

**Handle table.** A per-run vector of opened resources. The guest sees indices. An unknown index or a wrong kind is a denied call. Because the table holds only what was granted, guessing a different index gains nothing.

**Dir.** Opened once with cap-std at grant time. Only regular files are read, created, written or removed; anything else is `denied`. `list` returns regular files and directories only. `write` writes a temp file with an unpredictable name beside the target and renames it over, so readers never see a partial file. Every path from the guest is resolved relative to that open directory, which refuses absolute paths, `..` above the root and symlinks that lead out. On Linux this is `openat2` with `RESOLVE_BENEATH`.

**Http.** A grant names host and methods. The host checks the allowlist on the first request and on every redirect hop, refuses IP-literal hosts unless explicitly granted, and pins DNS resolution for the run. Response bodies are capped.

**Limits.** Wasmtime fuel bounds instructions. Epoch interruption bounds wall-clock, because fuel does not tick while a host call waits on a slow server. Each host call also has its own timeout and the run has a host-call count and total I/O byte budget.

## Limits

Defaults per run, or per request or WebSocket event under `--listen` (fuel 100 million, 64 MB, 5 s); each can be overridden by a `kio run` flag. A server also bounds slow clients: each connection lives at most 60 s (until it becomes a WebSocket), headers within 10 s (which also closes an idle keep-alive connection after 10 s), the body within 10 s more, headers to about 64 KB (hyper checks after each read, so one large read can reach about 114 KB), 1,000 connections, 100 requests queued. A WebSocket closes on a message over 1 MB (1009), over 64 KB of events waiting (1008), 5 minutes without a frame (1000), or a failed run (1011); its sends are written after each run, within 10 s. Exceeding one stops the run with an error naming the limit.

| Limit | Default | Flag | Bounds |
|---|---|---|---|
| Fuel | 1 billion | `--fuel` | instructions, so infinite loops end |
| Memory | 256 MB | `--memory` (MB) | guest linear memory |
| Wasm stack | 2 MB | `--stack` (KB) | recursion; in a release host, calls 1_000 deep (the interpreter's limit) need up to about 1.8 MB when they recurse through loops |
| Wall clock | 10 s | `--timeout` | slow hosts and long runs |
| Host calls | 10,000 | `--calls` | capability calls; at most 64 timed-out calls may still be finishing at once, process-wide |
| I/O | 100 MB total, 10 MB per read | `--io` | bytes read and written |

## Where the checker runs

Decided 2026-11-06: in the host. The host already holds every grant and enforces every limit, so a second check inside the sandbox adds no safety, and checking there cost the sandbox's memory and fuel on every request (open_questions #33). Adversarial source is then parsed outside the sandbox, so the front end must never panic or recurse without bound: the host runs it on a large stack, and the program generator (compiler C6) fuzzes it.

The host never relies on the checker for safety. The handle table and host functions enforce every guarantee on their own. The checker exists for early, readable rejection and for the authority report.

## Where state lives

| Lifetime | Where | How a program reaches it |
|---|---|---|
| One run or request | guest memory | `let mut`, `List`, `Map`: the language |
| Across requests, or across messages on one connection | host memory | `Kv` granted `--kv name=:memory:`: one map per grant, shared by every request (K1) |
| Across restarts | a file, a directory or another service | `Kv` granted `--kv name=path` (an append-only log the host owns and locks), `Dir` (flat files), or a network capability |

Only the first is in the language. Every other tier is a capability: named in `main`, granted by the operator, enforced and logged by the host, like any other I/O. A fresh sandbox per request is what makes the second tier necessary; other languages keep it in process globals, which kio does not have.

## Memory model

Value semantics without a garbage collector or lifetimes. The interpreter is the reference: later optimizations, and the compiler, must not change what a program observes; the tests that pin copy behavior and the differential harness are what make them safe.

| Step | Change | Observable behavior |
|---|---|---|
| v0 | every assignment, argument and container read clones | reference |
| copy on write (done) | values share storage with a reference count; any write to shared storage, including `x.f = v` and `xs[i] = v`, copies it first | unchanged |
| last-use moves | a variable's last use moves its storage instead of sharing it | unchanged |
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
