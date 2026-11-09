# Roadmap: from v0 to a general language

Where kio stands against its goal of a general-purpose systems language with near-native speed, and what is missing.

## Speed today

Measured 2026-11-06 on an Apple Silicon Mac, release builds, best of 3. Compiled times are run time: the ~10 ms compile is measured separately (an empty `main`).

| Program | Rust | Python 3.11 | kio compiled | kio interpreter | compiled vs Rust | compiled vs Python |
|---|---|---|---|---|---|---|
| `fib(35)`, calls | 24 ms | 890 ms | 117 ms | 3.8 s | 4.9x slower | 7.6x faster |
| 30 M-iteration loop | 20 ms | 1.76 s | 46 ms | 5.9 s | 2.3x slower | 38x faster |
| HTTP request, route and JSON, keep-alive | | | | 24,100 req/s, p99 1 ms | | |

The compiler handles `Int`, `Bool`, control flow and calls today (C2); strings, collections, structs and capabilities follow (C3, C4). Until C5, `kio run` and `--listen` still run the interpreter, which is about 4x slower than CPython on compute. For I/O-bound work such as the web example the interpreter does not dominate: a fresh sandbox per request costs 175 µs in all (docs/perf.md).

The remaining gap to Rust is mostly checks Rust omits: kio traps on every integer overflow (Rust release builds wrap), and Wasm bounds-checks memory. No optimization pass has run yet.

## The path to near-native

```
tree-walker ─► P2 copy-on-write ─► P3 lowering ─► interpreter on IR ─► compile to Wasm ─► Cranelift
                (done)              (done)          (done, 3x)            (C2 done for Int,   native code
                                                                          Bool, control, calls)
```

- **P2** fixed a cost bug: passing a large value copied it, so loops over helpers went quadratic.
- **P3** resolved names, fields and calls to indexes: the interpreter got 3x faster, and the compiler has a typed IR to consume.
- **The compiler** emits Wasm from that IR, linked with a small runtime (refcounts, strings, collections) in one module; Wasmtime's Cranelift compiles it to native code. The checker, report, sandbox and host enforcement are unchanged: compiled code calls the same host functions. Design: [codegen.md](codegen.md); plan: [compiler-todo.md](compiler-todo.md). Correctness: every corpus program runs compiled and interpreted, and they must agree.
- **No bytecode VM** (decided 2026-11-05): even a good one stays an interpreter inside a sandbox.

## What a real app needs that kio lacks

Most-used Rust crates, and kio's answer to each:

| Need | Rust | kio today | Gap |
|---|---|---|---|
| HTTP server | axum, hyper, tokio | `--listen` plus kio-web | Done for buffered request/response, and WebSockets with a fresh sandbox per message (WS1). No streaming or HTTP/2. |
| HTTP client | reqwest | `Http.get` | GET only: POST, headers and JSON bodies need `caps.wit` methods. |
| Async | tokio | none: a sandbox per request | Fine per request. Concurrent upstream calls inside one handler need host-side async (web decision 4). |
| Serialization | serde, serde_json | json.kio (W12), untyped tree | Typed decode into structs needs generics or derive. |
| Database | sqlx, diesel, redis | none | The largest gap for CRUD apps. Needs `Sql` and `Kv` capabilities (`caps.wit`, owner decision): the host holds the connection pool, the program gets a capability scoped to one database or key prefix. |
| Time, randomness, UUIDs | chrono, rand, uuid | none, by design (deterministic) | A `Clock` and `Rng` capability: granted explicitly, logged, and replayable. |
| Crypto, hashing | ring, sha2, jsonwebtoken | none | Pure functions, so built-ins in `lang` (no capability needed), or host functions for speed. |
| Errors | anyhow, thiserror | `Result`, `?`, enums | No conversion between error types (`From`). |
| Logging, metrics | tracing | the audit log covers host calls | A `Log` capability for application logs. |
| Regex, text | regex | `split`, `contains`, `starts_with` | Built-ins. |
| CLI parsing | clap | none | Needs an `Args` capability; the operator grants argv. |
| Floats | f64 | `Float` (f64; interpreter only so far) | JSON numbers still need json.kio support. |
| Abstraction | traits, generics, closures | function values (`fn(T) -> R`, L6a) and closures (L6b), interpreter only | The largest language gap. Without generics, every container helper is written once per type. Built-ins taking functions (L6c) are in the interpreter; generics (L7) are next. |
| Packages | cargo, crates.io | copy one file | A package manager, once there are a few libraries. |

Non-starters today, in order of what blocks a real backend first: a database capability, generics, and an HTTP client beyond GET.

## A target app

A CRUD app over a `Dir` (W9) proves the language. A target app proves the ecosystem: a small, real web service that an agent writes and an operator runs with exactly its grants. Candidate: a link shortener or notes service with a JSON API, a `Kv` store, auth by a header token (a `Secret` capability), and HTML pages. Each piece it lacks becomes a task, as kio-web did for `--listen`. W9 first, then decide the `Kv` capability, then the target app.
