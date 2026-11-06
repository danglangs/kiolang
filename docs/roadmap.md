# Roadmap: from v0 to a general language

Where kio stands against its goal of a general-purpose systems language with near-native speed, and what is missing. Measured 2026-11-05 on an Apple Silicon Mac, release builds.

## Speed today

| Program | Native Rust | Python 3.11 | kio | kio vs Rust |
|---|---|---|---|---|
| `fib(25)`, calls | 0.34 ms | 7 ms | 120 ms | about 350x |
| 3 M-iteration loop, arithmetic | 5.7 ms | 169 ms | 850 ms | about 150x |
| HTTP request, route and JSON, keep-alive | | | 24,100 req/s, p99 1 ms | |

kio times are `kio run` wall time minus about 20 ms of startup. kio is a tree-walking interpreter running inside Wasm: it re-resolves names and copies values as it goes. It is about 5x slower than CPython on compute. For I/O-bound work such as the web example, the interpreter does not dominate: a fresh sandbox per request costs 175 µs in all (docs/perf.md).

## The path to near-native

```
v0 tree-walker ──► P2 copy-on-write ──► P3 lowering ──► bytecode VM ──► compile to Wasm
   (today)         values shared,        names, fields     5-20x over      kio source becomes Wasm,
                   no quadratic copies   and variants      a lowered       which Wasmtime compiles
                                         resolved once     tree-walker     to native code: within
                                                                           about 1.5-3x of Rust
```

- **P2** (done) fixed a correctness-of-cost bug, not a constant factor: passing a large value to a function copied it, so loops over helpers went quadratic. W12's JSON parser on 100 KB went from 1.7 s to 0.58 s, and passing a 10,000-element list to a helper is about 40x faster.
- **P3 lowering** turns the checked AST into a resolved form: variables as slot numbers, fields and variants as indexes, calls as function indexes. It is the prerequisite for both the VM and a compiler, and alone it is typically a 2-5x speedup for a tree-walker, since most of today's time is string lookups and clones.
- **Bytecode VM**: worthwhile only if kio keeps interpreting. Inside Wasm it remains an interpreter in a sandbox, so its ceiling is roughly CPython-to-Lua speed.
- **Compile to Wasm** is how "near native" is reached. kio has a static type system with no GC, no references and no reflection, so lowering to Wasm functions is direct. Wasmtime's Cranelift then compiles that to native code, typically within 1.5-3x of native Rust. The checker and report stay as they are; the host and its enforcement are unchanged, since compiled code still calls the same host functions. This replaces the guest's interpreter, not the sandbox.

So: P3 is critical, because both the VM and the compiler need it. The recommendation is to skip the bytecode VM and go from P3 to a Wasm backend, unless a profile after P3 shows interpretation is good enough for kio's workloads for a long time.

## What a real app needs that kio lacks

Most-used Rust crates, and kio's answer to each:

| Need | Rust | kio today | Gap |
|---|---|---|---|
| HTTP server | axum, hyper, tokio | `--listen` plus kio-web | Done for buffered request/response. No streaming, WebSockets (owner decision) or HTTP/2. |
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
| Floats | f64 | none | Needed for most numeric work and for JSON numbers. |
| Abstraction | traits, generics, closures | none | The largest language gap. Without generics, every container helper is written once per type. Closures or function values are needed for middleware and route tables. |
| Packages | cargo, crates.io | copy one file | A package manager, once there are a few libraries. |

Non-starters today, in order of what blocks a real backend first: a database capability, generics and function values, floats, and an HTTP client beyond GET.

## A target app

A CRUD app over a `Dir` (W9) proves the language. A target app proves the ecosystem: a small, real web service that an agent writes and an operator runs with exactly its grants. Candidate: a link shortener or notes service with a JSON API, a `Kv` store, auth by a header token (a `Secret` capability), and HTML pages. Each piece it lacks becomes a task, as kio-web did for `--listen`. W9 first, then decide the `Kv` capability, then the target app.
