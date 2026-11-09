# Roadmap: from v0 to a general language

Where kio stands against its goal of a general-purpose systems language with near-native speed, and what is missing.

## Speed today

C8b5, 2026-11-09: Apple M3 Max (16 cores, 64 GB), release builds, rustc 1.99.1 `-O`, Python 3.13.2; best of 5, each column's startup subtracted (kio's cached start is 7 ms). Compile time is separate: a program's first `kio run` compiles the runtime and the program (about 20-25 ms), later runs load the cache. Runs pass a huge `--fuel` (the default is 10 G since C8a). Programs, method and rerunning: [bench/](../bench/README.md). The kio compiled and C8b4 columns are the two binaries run alternately, load 2.3-6; C8a is before C8b1. Rust, Python and the interpreter are C8b5's full `bench/run.sh` run (load 2.4-3.8). HTTP is C8b4's: kio-web's `web.kio` now uses WS1b's methods, which this build lacks. Fuel is Wasmtime's count (`--audit`'s `fuel_used`), exact and load-free.

| Benchmark | Rust | Python | kio compiled | C8b4 | C8a | fuel C8b4 to C8b5 | compile | kio interpreted | compiled vs Rust | vs Python |
|---|---|---|---|---|---|---|---|---|---|---|
| `fib(35)`, calls | 23 ms | 704 ms | 53-57 ms | 53 ms | 54 ms | 1.08 G, same | 22 ms | 1.54 s | 2.3x slower | 13x faster |
| 30 M-iteration integer loop | 97 ms | 933 ms | 106 ms | 106 ms | 110 ms | 2.10 G, same | 22 ms | 1.27 s | 1.1x slower | 8.8x faster |
| Mandelbrot 400x400, 100 iterations | 11 ms | 309 ms | 13 ms | 13 ms | 14 ms | 0.28 G, same | 21 ms | 958 ms | 1.2x slower | 24x faster |
| 1 M push, sort, index and `for` sums | 16 ms | 365 ms | 27 ms | 27 ms | 52 ms | 0.96 G, same | 23 ms | 193 ms | 1.7x slower | 13x faster |
| tree of 2^20 leaves built and folded | 33 ms | 2.87 s | 98 ms | 185 ms | 438 ms | 3.78 to 2.21 G | 22 ms | 1.03 s | 3.0x slower | 29x faster |
| `map`/`filter` with closures, 1 M | 5.8 ms | 77 ms | 13 ms | 13 ms | 36 ms | 0.21 G, same | 22 ms | 96 ms | 2.3x slower | 5.8x faster |
| word count, 1 MB of text | 6.5 ms | 17 ms | 45 ms | 70 ms | 98 ms | 1.12 to 0.68 G | 23 ms | 67 ms | 6.9x slower | 2.7x slower |
| JSON parse + encode, 1 MB (json.kio) | 8.0 ms | 17 ms | 66 ms | 81 ms | 554 ms | 1.12 to 0.93 G | 24 ms | 812 ms | 8.3x slower | 3.8x slower |
| JSON parse + encode, 1 MB (built-in `parse_json`, `to_json`; C12, load 6.5) | 7.8 ms | 17 ms | 17 ms | | | | 23 ms | 14 ms | 2.2x slower | 1.0x |
| 100 k `s = s + i.to_str() + ","` | 4.6 ms | 7.2 ms | 5.2 ms | 5.3 ms | 1.37 s | 0.08 G, same | 23 ms | 1.57 s | 1.1x slower | 1.4x faster |
| HTTP `GET /hello/kio`, `ab -k -c 12` (C8b4) | 237,000 req/s (hyper) | 29,100 req/s (`http.server`) | 73,800 req/s (kio-web, `--listen`) | | 72,000 | | once at startup | | 3.2x fewer | 2.5x more |

C8b5 (less work per operation; codegen.md "How it is built" 20) cut tree, word count and JSON, and the fuel they use: generated code pops the size-class free lists itself, an empty class carves 32 blocks from a 64 KB chunk instead of a dlmalloc call per block, freeing reads descriptors in place, and a `Str` map key compares its first 8 bytes as one number inside an inlined `BTreeMap` search. Scaled, the same two binaries alternating: tree at 2^22 leaves 743 to 399 ms, JSON ten times 796 to 635, word count ten times 697 to 443. Steps (fuel, tree / JSON / word count): free without descriptor copies 3.78 to 3.55 G / 1.07 G / -; inline pops with chunks 2.84 G / 0.97 G; batches of 32 2.21 G / 0.93 G; map key prefix and inlined compare word count 1.12 to 0.68 G. Fuel's share of the time (scratch build with fuel and epochs off, best of 5): tree 27% to 26% (185 to 99 ms with, 135 to 74 without), JSON 37% to 36%, word count 35% to 30%: the overhead per operation is the same, there are fewer operations. Peak memory: tree 121 to 90 MB (no dlmalloc header per block), JSON 14.9 to 16.0 MB, programs that allocate little +0.3 MB; a program that leaves one live block in every chunk needs more (open_questions #99). Profile after (2^22-leaf tree three times): freeing 54%, the program's own code 38%; word count: `BTreeMap` search 54% (`get` then `insert` searches twice), `split` 13%; JSON: freeing 18%, program code and conversions the rest.

The built-in JSON (C12) is the runtime parsing and encoding kio objects directly: 4.8x json.kio. Its profile (100 rounds): parse 72%, encode 19%, freeing 9%; `BTreeMap` inserts with their node allocations about 39%, `dlmalloc` about 40% inclusive, float and integer formatting (`core::fmt`) 6%.

C8b4 (borrowed parameters, borrowed `Str` map keys) moved word count (key copies gone from `get` and repeated `insert`: 6%) and JSON (`b: Bytes` lent to every `byte(b, i)`: 3%; ten times 820 to 799 ms). Tree is 1-2% slower (2^22 leaves 731-739 to 740-756 ms): `fold(k)` moved `k` into the call before and lends it now, the same reference count, so nothing is saved and the code moved.

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
| 4 | Done (C8b2, C8b4): last-use moves and borrowed local operands; C8b4 borrowed parameters (JSON 83 to 80, tree unchanged). With inline `Bytes` paths and one descriptor lookup per free, tree 284 to 244 ms, JSON 143 to 101 | 1.3-2x on JSON parse, tree | json, tree rows; `rt_inc`/`rt_dec` share of the profile |
| 5 | Done in part (C8b3): a free list per size class (8-64 bytes) over dlmalloc; tree 243 to 179 ms, JSON 100 to 82, word count 83 to 75, strings 7.9 to 5.2. C8b5: fresh blocks carved from 64 KB chunks, 32 at a time, and popped inline by generated code; tree 185 to 98 ms. Not done: a list's buffer inline in its object; freeing (54% of tree) is the next cost | 1.5x on tree, word count | tree, word count; `malloc`/`free` share |
| 6 | Done (C8b2): word-at-a-time `Str` `==` and order; word count unchanged (85 to 83 ms): its words differ in the first byte, and the time is `BTreeMap`'s linear search within nodes, a key copy per lookup (gone in C8b4: 74 to 70 ms), `split`'s allocations. C8b5: a `Str` key's first 8 bytes compared as one number, the compare inlined into the search: 70 to 45 ms. Left: `get` then `insert` searches twice (open_questions #101) | word count 1.3-2x | word count row; `memcmp` share |
| 7 | Cheaper work limit: Wasmtime fuel off, codegen counting at loop heads and calls of the program only (an owner decision: it changes what `--fuel` counts). Declined 2026-11-09 (#98): fuel stays, and C8b5 cut the operations it counts (tree 3.78 to 2.21 G); fuel and epochs still cost 26-36% of runtime-heavy runs | 10-25% on runtime-heavy code, after #3 less | a scratch no-fuel build is the bound (bench/README.md) |

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
| HTTP server | axum, hyper, tokio | `--listen` plus kio-web | Done for buffered request/response, and WebSockets with a fresh sandbox per message (WS1). No streaming or HTTP/2; a WebSocket program cannot see the upgrade's headers. |
| HTTP client | reqwest | `Http.get` | GET only: POST, headers and JSON bodies need `caps.wit` methods. |
| Async | tokio | none: a sandbox per request | Fine per request. Concurrent upstream calls inside one handler need host-side async (web decision 4). |
| Serialization | serde, serde_json | built-in `Json` (L9, C12), an untyped tree, 2.2x `serde_json` | Typed decode into structs needs a derive or traits; generics alone do not give it. |
| Database | sqlx, diesel, redis | `Kv` (K1): get/set/delete/keys/swap, in memory or an append-only file the host owns | No SQL. A `Sql` capability (`caps.wit`, owner decision): the host holds the connection pool, the program gets a capability scoped to one database. |
| Time, randomness, UUIDs | chrono, rand, uuid | none, by design (deterministic) | A `Clock` and `Rng` capability: granted explicitly, logged, and replayable. |
| Crypto, hashing | ring, sha2, jsonwebtoken | none | Pure functions, so built-ins in `lang` (no capability needed), or host functions for speed. |
| Errors | anyhow, thiserror | `Result`, `?`, enums | No conversion between error types (`From`). |
| Logging, metrics | tracing | the audit log covers host calls | A `Log` capability for application logs. |
| Regex, text | regex | `split`, `lines`, `trim`, `find`, `replace`, `strip_prefix`, `contains`, `starts_with`, `slice` | No regex. |
| CLI parsing | clap | none | Needs an `Args` capability; the operator grants argv. |
| Floats | f64 | `Float` (f64, compiled at Rust's speed) | |
| Abstraction | traits, generics, closures | function values (`fn(T) -> R`, L6a), closures (L6b) and built-ins taking them (`map`, `filter`, `any`, `all`, `and_then`, `map_err`; L6c), compiled (C9); generic functions (L7a) and generic structs and enums (L7b, L7d), monomorphized, with bounds `T: Eq`/`T: Ord` (L7c) | Bounds `T: Eq`/`Ord` (L7c) wait on the `<`-on-`Str` decision; no traits. |
| Packages | cargo, crates.io | copy one file | A package manager, once there are a few libraries. |

Non-starters today, in order of what blocks a real backend first: an SQL capability and an HTTP client beyond GET.

## A target app

A CRUD app over a `Dir` (W9a) proves the language. A target app proves the ecosystem: a small, real web service that an agent writes and an operator runs with exactly its grants. Candidate: a link shortener or notes service with a JSON API, a `Kv` store, auth by a header token (a `Secret` capability), and HTML pages. Each piece it lacks becomes a task, as kio-web did for `--listen`. `Kv` exists, and kio-web has notes apps over `Dir` and over `Kv`; the target app is next.
