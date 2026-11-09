# Roadmap: from v0 to a general language

Where kio stands against its goal of a general-purpose systems language with near-native speed, and what is missing.

## Speed today

C8b1, 2026-11-09: Apple M3 Max (16 cores, 64 GB), load average 4-8 (another job running), release builds, rustc 1.99.1 `-O`, Python 3.13.2; best of 5, each column's startup subtracted (kio's cached start is 7 ms). Compile time is separate: a program's first `kio run` compiles the runtime and the program (about 20-30 ms), later runs load the cache. Runs pass a huge `--fuel` (the default is 10 G since C8a). Programs, method and rerunning: [bench/](../bench/README.md). The C8a column is the run before C8b1 (in-place append, no zero-fill, inline refcounts and list access).

| Benchmark | Rust | Python | kio compiled | C8a | compile | kio interpreted | compiled vs Rust | vs Python |
|---|---|---|---|---|---|---|---|---|
| `fib(35)`, calls | 24 ms | 732 ms | 56 ms | 54 ms | 21 ms | 1.62 s | 2.4x slower | 13x faster |
| 30 M-iteration integer loop | 97 ms | 928 ms | 111 ms | 110 ms | 21 ms | 1.32 s | 1.1x slower | 8.4x faster |
| Mandelbrot 400x400, 100 iterations | 12 ms | 333 ms | 15 ms | 14 ms | 25 ms | 1.08 s | 1.2x slower | 22x faster |
| 1 M push, sort, index and `for` sums | 16 ms | 366 ms | 28 ms | 52 ms | 21 ms | 196 ms | 1.7x slower | 13x faster |
| tree of 2^20 leaves built and folded | 33 ms | 2.87 s | 284 ms | 438 ms | 23 ms | 1.05 s | 8.6x slower | 10x faster |
| `map`/`filter` with closures, 1 M | 4.2 ms | 79 ms | 13 ms | 36 ms | 21 ms | 101 ms | 3.2x slower | 5.9x faster |
| word count, 1 MB of text | 6.0 ms | 17 ms | 85 ms | 98 ms | 22 ms | 65 ms | 14x slower | 5.1x slower |
| JSON parse + encode, 1 MB (json.kio) | 7.5 ms | 16 ms | 144 ms | 554 ms | 22 ms | 868 ms | 19x slower | 9.0x slower |
| 100 k `s = s + i.to_str() + ","` | 3.5 ms | 7.0 ms | 8.5 ms | 1.37 s | 21 ms | 1.57 s | 2.4x slower | 1.2x slower |
| HTTP `GET /hello/kio`, `ab -k -c 12` | 239,000 req/s (hyper) | 31,700 req/s (`http.server`) | 75,900 req/s (kio-web, `--listen`) | 72,000 | once at startup | | 3.2x fewer | 2.4x more |

JSON is from a second run (load 4-7): in the full run, under load 8, it measured 189 ms with Rust at 12 ms. String building now needs about 0.1-0.12 G fuel (116 G before: every concatenation copied and zero-filled the whole string).

Rust and Python are idiomatic: Rust's string building appends in place (`push_str`), word count uses a `HashMap`, JSON is `serde_json::Value`; Python's `+=` on a string and `dict` are built in C. Every version prints the same number.

**Where the time went (C8a, before C8b1).** Code that stays in compiled Wasm (integers, floats, calls) runs within 1.1-2.3x of Rust; overflow checks cost Rust itself 23% on `fib`, 0% on the loop. Everything that calls the runtime is 3-270x slower, and word count is slower than the interpreter (99 against 69 ms). Profiles (`bench/profile.py`, macOS `sample` with JIT frames named):

- **String building** (271x): 99% in `rt_concat`. `s = s + x` copies `s` every time (quadratic: 59 GB moved for a 0.6 MB result), and the copy's buffer is zero-filled first: 52% `memmove`, 47% `bzero`. Wasmtime charges fuel per byte of `memory.copy` and `memory.fill`: the run uses 116 G fuel, so the default 100 M stops a program after about 100 MB copied or zeroed.
- **JSON** (69x): about half `json::encode`, nearly all `rt_concat` (`out = out + encode(x)`, the same quadratic copy and zero-fill); half `json::parse`, where `BTreeMap` inserts, allocation and `rt_inc`/`rt_dec` of the `Bytes` passed to `byte(b, i)` for every byte dominate. `rt_dec` is 27% inclusive.
- **Word count** (14x, slower than interpreted): `memcmp` inside the runtime's `BTreeMap` is Wasm's byte-at-a-time loop (14-48% across runs), where the interpreter uses the host's vectorized one; `split` allocates every word.
- **Closures** (14x; profiled at 10 M): `rt_list_get` 45% (a runtime call per element, the element `rt_inc`'d), `rt_unique` 16% (a call per `push`), the `fn`-value wrapper and `call_indirect` 25%.
- **Tree** (12x): about 1/3 building (4 allocations per node against Rust's 1: the enum, the `List` header and its buffer), 2/3 folding, of which freeing the tree is most (`rt_dec` 53%: descriptor lookups `record`/`size`, `free`); `rt_unique` 11% for `s.sum += ..` on a local struct.
- **Fuel and epoch checks**, measured with scratch builds that turn them off: runtime-heavy runs lose 18-36% to them (wall time, both on to both off: tree 475 to 302 ms, closures 45 to 31, word count 105 to 71, list 59 to 45, JSON 583 to 477; fuel is half to two thirds of it), compiled-only code little (loop and Mandelbrot 0-5%, fib 5%). Each runtime call pays fuel bookkeeping and an epoch check at entry. Wasm memory bounds checks are free (Cranelift defaults: `opt_level` speed, 4 GB reservation with guard pages, so no explicit checks).

**C8b candidates, by expected gain:**

| # | Change | Expected | Measure |
|---|---|---|---|
| 1 | Done (C8b1): in-place `Str` append when `s` is unique and dead after (`s = s + x` reuses `s`, capacity doubling); strings 1.37 s to 8.5 ms, JSON 554 to 144 ms | strings 100x+, JSON encode 5-10x | strings, json rows; strings' `fuel_used` (`--audit`) from 116 G to under 10 M |
| 2 | Done (C8b1): no zero-fill of `Str`/`Bytes` data | up to 2x on concatenation now, ~5% after #1 | strings, json; `__bzero` gone from the profile |
| 3 | Done (C8b1): `rt_inc`/`rt_dec`/`rt_unique` fast paths and list `len`/`get`/`push` inline; closures 36 to 13 ms, list 52 to 28, tree 438 to 284 | 1.5-2x on closures, list, tree; removes most of the fuel/epoch share | closures, list, tree rows; calls per run (profile) |
| 4 | Borrowed parameters and Perceus-style elision of `inc`/`dec` pairs at last uses | 1.3-2x on JSON parse, tree | json, tree rows; `rt_inc`/`rt_dec` share of the profile |
| 5 | Fewer allocations: a list's buffer inline in its object, a small-object free list per size class over dlmalloc | 1.5x on tree, word count | tree, word count; `malloc`/`free` share |
| 6 | Word-at-a-time `memcmp`/`eq` for `Str` in the runtime | word count 1.3-2x | word count row; `memcmp` share |
| 7 | Cheaper work limit: Wasmtime fuel off, codegen counting at loop heads and calls of the program only (an owner decision: it changes what `--fuel` counts) | 10-25% on runtime-heavy code, after #3 less | a scratch no-fuel build is the bound (bench/README.md) |

## The path to near-native

```
tree-walker ─► P2 copy-on-write ─► P3 lowering ─► interpreter on IR ─► compile to Wasm ─► Cranelift
                (done)              (done)          (done, 3x)            (done, C5:          native code
                                                                          the whole language)
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
| Floats | f64 | `Float` (f64; interpreted and compiled, C10) | JSON numbers still need json.kio support. |
| Abstraction | traits, generics, closures | function values (`fn(T) -> R`, L6a), closures (L6b) and built-ins taking them (`map`, `filter`, `any`, `all`, `and_then`, `map_err`; L6c), interpreted and compiled (C9); generic functions (L7a) and generic structs and enums (L7b), monomorphized | Bounds `T: Eq`/`Ord` (L7c) next; no traits. |
| Packages | cargo, crates.io | copy one file | A package manager, once there are a few libraries. |

Non-starters today, in order of what blocks a real backend first: a database capability and an HTTP client beyond GET.

## A target app

A CRUD app over a `Dir` (W9) proves the language. A target app proves the ecosystem: a small, real web service that an agent writes and an operator runs with exactly its grants. Candidate: a link shortener or notes service with a JSON API, a `Kv` store, auth by a header token (a `Secret` capability), and HTML pages. Each piece it lacks becomes a task, as kio-web did for `--listen`. W9 first, then decide the `Kv` capability, then the target app.
