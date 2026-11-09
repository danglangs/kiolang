# Roadmap: from v0 to a general language

Where kio stands against its goal of a general-purpose systems language with near-native speed, and what is missing.

## Speed today

C8b3, 2026-11-09: Apple M3 Max (16 cores, 64 GB), load average 2-9 (other builds running), release builds, rustc 1.99.1 `-O`, Python 3.13.2; best of 5, each column's startup subtracted (kio's cached start is 7 ms). Compile time is separate: a program's first `kio run` compiles the runtime and the program (about 20-25 ms), later runs load the cache. Runs pass a huge `--fuel` (the default is 10 G since C8a). Programs, method and rerunning: [bench/](../bench/README.md). The C8b2 column is the C8b2 binary run alternately with C8b3's, under the same load; HTTP was not rerun (C8b2's run); C8a is before C8b1. Rust, Python and the interpreter are C8b2's run.

| Benchmark | Rust | Python | kio compiled | C8b2 | C8a | compile | kio interpreted | compiled vs Rust | vs Python |
|---|---|---|---|---|---|---|---|---|---|
| `fib(35)`, calls | 24 ms | 705 ms | 53 ms | 53 ms | 54 ms | 20 ms | 1.53 s | 2.2x slower | 13x faster |
| 30 M-iteration integer loop | 98 ms | 931 ms | 108 ms | 104 ms | 110 ms | 24 ms | 1.26 s | 1.1x slower | 8.6x faster |
| Mandelbrot 400x400, 100 iterations | 11 ms | 310 ms | 13 ms | 13 ms | 14 ms | 23 ms | 954 ms | 1.2x slower | 24x faster |
| 1 M push, sort, index and `for` sums | 17 ms | 367 ms | 27 ms | 27 ms | 52 ms | 24 ms | 191 ms | 1.6x slower | 13x faster |
| tree of 2^20 leaves built and folded | 33 ms | 2.87 s | 179 ms | 243 ms | 438 ms | 24 ms | 1.04 s | 5.4x slower | 16x faster |
| `map`/`filter` with closures, 1 M | 2.8 ms | 77 ms | 13 ms | 13 ms | 36 ms | 23 ms | 95 ms | 4.6x slower | 6.0x faster |
| word count, 1 MB of text | 7.4 ms | 16 ms | 75 ms | 83 ms | 98 ms | 23 ms | 66 ms | 10x slower | 4.7x slower |
| JSON parse + encode, 1 MB (json.kio) | 8.7 ms | 17 ms | 82 ms | 100 ms | 554 ms | 23 ms | 813 ms | 9.4x slower | 4.8x slower |
| 100 k `s = s + i.to_str() + ","` | 5.0 ms | 6.9 ms | 5.2 ms | 7.9 ms | 1.37 s | 23 ms | 1.57 s | 1.0x slower | 1.3x faster |
| HTTP `GET /hello/kio`, `ab -k -c 12` | 236,000 req/s (hyper) | 28,900 req/s (`http.server`) | 73,500 req/s (kio-web, `--listen`) | 73,500 | 72,000 | once at startup | | 3.2x fewer | 2.5x more |

C8b3 (size-class free lists, `runtime/src/heap.rs`) moved tree, JSON, word count and strings. Before it, dlmalloc's `malloc`, `free` and `unlink_chunk`, zeroing and the allocation shim were about 41% of tree (2^22 leaves) and 24% of JSON (ten times); after, 23% (dlmalloc's fresh blocks while the tree is built, all of them alive) and 9%. Scaled runs, best of 5 alternating the two binaries: tree at 2^22 leaves 980 to 725 ms, JSON ten times 972 to 820 ms.

C8b2 moved tree (freeing reads each descriptor once: 15%; moves and borrows the rest) and JSON (borrowed `Bytes` operands, inline `Bytes` `len`/index/`push`: 30%); word count is where it was (see #6 below). Scaled runs, best of 5 alternating the two binaries: tree at 2^22 leaves 1.14 s to 0.97 s, JSON five times 736 to 536 ms, word count ten times 832 to 843 ms.

Since C8b1 string building needs about 0.1-0.12 G fuel (116 G before: every concatenation copied and zero-filled the whole string).

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
| 4 | Done in part (C8b2): last-use moves and borrowed local operands (borrowed parameters remain); with inline `Bytes` paths and one descriptor lookup per free, tree 284 to 244 ms, JSON 143 to 101 | 1.3-2x on JSON parse, tree | json, tree rows; `rt_inc`/`rt_dec` share of the profile |
| 5 | Done in part (C8b3): a free list per size class (8-64 bytes) over dlmalloc; tree 243 to 179 ms, JSON 100 to 82, word count 83 to 75, strings 7.9 to 5.2. Not done: a list's buffer inline in its object; carving fresh small blocks from larger chunks (dlmalloc's `malloc` is still 17% of tree, while it is built) | 1.5x on tree, word count | tree, word count; `malloc`/`free` share |
| 6 | Done (C8b2): word-at-a-time `Str` `==` and order; word count unchanged (85 to 83 ms): its words differ in the first byte, and the time is `BTreeMap`'s linear search within nodes, a key copy per lookup, `split`'s allocations | word count 1.3-2x | word count row; `memcmp` share |
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
