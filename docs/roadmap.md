# Roadmap: from v0 to a general language

Where kio stands against its goal of a general-purpose systems language with near-native speed, and what is missing.

## Speed today

C8b5, 2026-11-09: Apple M3 Max (16 cores, 64 GB), release builds, rustc 1.99.1 `-O`, Python 3.13.2; best of 5, each column's startup subtracted (kio's cached start is 7 ms). Compile time is separate: a program's first `kio run` compiles the runtime and the program (about 20-25 ms), later runs load the cache. Runs pass a huge `--fuel` (the default is 10 G since C8a). Programs, method and rerunning: [bench/](../bench/README.md). The kio compiled and C8b4 columns are the two binaries run alternately, load 2.3-6; C8a is before C8b1. Rust, Python and the interpreter are C8b5's full `bench/run.sh` run (load 2.4-3.8). HTTP is C8b4's: kio-web's `web.kio` now uses WS1b's methods, which this build lacks. Fuel is Wasmtime's count (`--audit`'s `fuel_used`), exact and load-free.

C8c (2026-11-10) re-measured the five allocation-heavy rows on the C8b7 build with fuel on and off, counted the objects each allocates and attributed the time; the plan and the ranked tasks are in [proposals/allocation-speed.md](proposals/allocation-speed.md). In short: fib 54 ms (2.3x; the gap is Wasmtime's per-call prologue and the lack of inlining, not allocation), `Map` word count 27 (5.5x of Rust's `HashMap<&str>`, 1.7x of Rust's own `BTreeMap<&str>` at 15.7 ms), `HashMap` 22 (4.7x), tree 86 (2.6x; 5.24 M objects against Rust's 1.05 M mallocs, 2.1 M of them `Stats` structs), json.kio 62 (8.3x; 1.49 M objects for 157 k values and keys, a call per byte). Metering is 25-40% of the runtime-heavy rows (a sandbox cost, reported apart).

| Benchmark | Rust | Python | kio compiled | C8b4 | C8a | fuel C8b4 to C8b5 | compile | kio interpreted | compiled vs Rust | vs Python |
|---|---|---|---|---|---|---|---|---|---|---|
| `fib(35)`, calls | 23 ms | 704 ms | 53-57 ms | 53 ms | 54 ms | 1.08 G, same | 22 ms | 1.54 s | 2.3x slower | 13x faster |
| 30 M-iteration integer loop | 97 ms | 933 ms | 106 ms | 106 ms | 110 ms | 2.10 G, same | 22 ms | 1.27 s | 1.1x slower | 8.8x faster |
| Mandelbrot 400x400, 100 iterations | 11 ms | 309 ms | 13 ms | 13 ms | 14 ms | 0.28 G, same | 21 ms | 958 ms | 1.2x slower | 24x faster |
| 1 M push, sort, index and `for` sums | 16 ms | 365 ms | 27 ms | 27 ms | 52 ms | 0.96 G, same | 23 ms | 193 ms | 1.7x slower | 13x faster |
| tree of 2^20 leaves built and folded (C8b6: 97 to 101 ms, fuel same, #117; C8b7: 101 to 90 ms, fuel 2.26 to 1.80 G) | 33 ms | 2.87 s | 90 ms | 185 ms | 438 ms | 3.78 to 2.21 G | 22 ms | 1.03 s | 2.7x slower | 32x faster |
| `map`/`filter` with closures, 1 M | 5.8 ms | 77 ms | 13 ms | 13 ms | 36 ms | 0.21 G, same | 22 ms | 96 ms | 2.3x slower | 5.8x faster |
| word count, 1 MB of text (C8b6: 45.7 to 29.3 ms, fuel 0.68 to 0.43 G; C8b7: 29.8 to 28.2, 0.43 to 0.40 G) | 6.7 ms | 17 ms | 28 ms | 70 ms | 98 ms | 1.12 to 0.68 G | 27 ms | 69 ms | 4.2x slower | 1.6x slower |
| word count on `HashMap` (C8b6: 31.8 to 25.0 ms, fuel 0.49 to 0.36 G; follow-up, one search for `insert(k, get(k)..)`: 25.0 to 23.2 ms, 0.361 to 0.337 G; C8b7: 23.6 to 22.4, 0.337 to 0.306 G) | 5.0 ms | 17 ms (`map.py`) | 22 ms | | | | 24 ms | 47 ms | 4.5x slower | 1.3x slower |
| JSON parse + encode, 1 MB (json.kio; C8b7: 66 to 64 ms, fuel 0.87 to 0.80 G) | 8.0 ms | 17 ms | 66 ms | 81 ms | 554 ms | 1.12 to 0.93 G | 24 ms | 812 ms | 8.3x slower | 3.8x slower |
| JSON parse + encode, 1 MB (built-in `parse_json`, `to_json`; C8b6: 15.3 to 12.8 ms, fuel 0.23 to 0.18 G) | 8.8 ms | 18 ms | 13 ms | | | | 26 ms | 14 ms | 1.5x slower | 1.4x faster |
| 100 k `s = s + i.to_str() + ","` (C8b6: 5.1 to 4.8 ms; C8b7: 5.0 to 4.5) | 4.6 ms | 7.2 ms | 5.2 ms | 5.3 ms | 1.37 s | 0.08 G, same | 23 ms | 1.57 s | 1.1x slower | 1.4x faster |
| HTTP `GET /hello/kio`, `ab -k -c 12` (C8b4) | 237,000 req/s (hyper) | 29,100 req/s (`http.server`) | 73,800 req/s (kio-web, `--listen`) | | 72,000 | | once at startup | | 3.2x fewer | 2.5x more |

C8b7 (cheaper freeing; codegen.md "How it is built" 25), C8b6 and C8b7 binaries alternating, best of 5, load 2-10: freeing reads one precomputed plan per object (pointer slots as a bit mask, size, kind) instead of walking the descriptor table, runtime calls get the table by reference, and `dec` frees an object holding no pointers without the work list. Freeing was 51% of tree's fuel (2^22 leaves: 4.61 of 9.05 G, about 40% of the time); it is now 2.75 G. Tree 101 to 90 ms (fuel -20%), word count -5%, `HashMap` -5%, json.kio -4%, strings -10%, others within noise; fib 5% slower at the same fuel, which a build with one unused function added removes (code placement). Scaled: tree 2^22 395 to 356 ms. Left in tree's freeing: the free-list push (~25 instructions a block), the work list, and a `List`'s separate buffer (one more block per node).

C8b6 (word count and JSON; codegen.md "How it is built" 22), C8b5 and C8b6 binaries alternating, best of 5, load 1.4-3.1; the word count, `HashMap` and built-in JSON rows are C8b6's `bench/run.py` (load 1.6-2.1): `insert(k, get(k)..)` on a `Map` searches once (the object remembers the entry `get` found), `split` at a one-byte separator sizes its list once and finds separators 8 bytes at a time, freeing a `List<Str>` frees its words in place, one-slot variants are built without a second descriptor lookup, integers are formatted without `core::fmt`, and blocks up to 512 bytes come from size-class lists. Scaled: word count x10 452 to 286 ms, `HashMap` x10 313 to 243, JSON x100 1,454 to 1,197, tree 2^22 383 to 403 (same fuel: the larger classes, open_questions #117). Profile after (word count x10): `BTreeMap` search in `get` 39%, freeing 12%, `insert` 11%, `split` 11%.

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
| 5 | Done in part (C8b3): a free list per size class (8-64 bytes) over dlmalloc; tree 243 to 179 ms, JSON 100 to 82, word count 83 to 75, strings 7.9 to 5.2. C8b5: fresh blocks carved from 64 KB chunks, 32 at a time, and popped inline by generated code; tree 185 to 98 ms. C8b6: classes up to 512 bytes (JSON fuel -9%; tree 4-5% slower, #117). Not done: a list's buffer inline in its object. C8b7: freeing by precomputed plans, tree 101 to 90 ms | 1.5x on tree, word count | tree, word count; `malloc`/`free` share |
| 6 | Done (C8b2): word-at-a-time `Str` `==` and order; word count unchanged (85 to 83 ms): its words differ in the first byte, and the time is `BTreeMap`'s linear search within nodes, a key copy per lookup (gone in C8b4: 74 to 70 ms), `split`'s allocations. C8b5: a `Str` key's first 8 bytes compared as one number, the compare inlined into the search: 70 to 45 ms. C8b6: `insert(k, get(k)..)` searches once on `Map` (#116), `split` cheaper: 45 to 29 ms. Left: the search itself (39%); `HashMap` searches once too since the C8b6 follow-up (#118) | word count 1.3-2x | word count row; `memcmp` share |
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
| HTTP server | axum, hyper, tokio | `--listen` plus kio-web | Done for buffered request/response, and WebSockets with a fresh sandbox per message (WS1). WebSocket programs see the upgrade's path and headers (WS1b). `send` answers the client at once and `main` runs on (H18). No streaming or HTTP/2; `Http` works under `--listen` (H19): DNS pinned per request, at most 64 outbound calls in flight per request, 1,000 calls per request by default. |
| HTTP client | reqwest | `Http.get`, `send`, `send_all` | Any method the operator grants, headers, bodies, parallel batches (H16, H16b). No streaming bodies. |
| Async | tokio | none in the language: a sandbox per request (decided: no async/await, no Wasm threads) | Fine per request. Concurrent upstream calls: `send_all` (H16b); work after the response: `send` flushes at once (H18). CPU fan-out `List.par_map` (L18) and `scope`/`spawn` (L19) are pinned for when a program needs them ([proposal](proposals/concurrency.md)). |
| Serialization | serde, serde_json | built-in `Json` (L9, C12), an untyped tree: `parse_json`, `to_json`, accessors; 1.5x `serde_json`, 1.4x faster than Python's `json` | Typed decode into structs needs a derive or traits; generics alone do not give it. |
| Database | sqlx, diesel, redis | `Db` (H17): SQLite in the host, `query`/`exec`/`transaction` with `?` parameters, `--db`, a fenced connection, `:ro` readers; `Kv` (K1, K2): get/set/delete/keys/swap/scan/add, in memory or an append-only file the host owns; `:ro` readers follow a writer (H12) | Only SQLite: no server databases (Postgres, Redis); no typed row decode. |
| Time, randomness, UUIDs | chrono, rand, uuid | `Clock` (`now_ms`) and `Rng` (`bytes`, `int`) capabilities (H13, H14): granted explicitly (`--clock`, `--rng`), audited, fixable for replay (`fixed:`, `seed:`); a `Log` for the program's own lines (H15) | Date arithmetic and formatting as built-ins; UUIDs from `Rng.bytes(16)` in library code. |
| Crypto, hashing | ring, sha2, jsonwebtoken | `sha256`, `to_hex`, constant-time `ct_eq` (L11) | No HMAC, signatures or encryption: pure functions, so built-ins in `lang` (no capability needed), or host functions for speed. |
| Collections | `HashMap`, `BTreeMap`, tuples | `List`, `Map` (key order), `HashMap` (insertion order, L10, C13), `Int` or `Str` keys | No sets, tuples, `contains_key` or `for` over a map (open_questions #114). |
| Errors | anyhow, thiserror | `Result`, `?`, enums | No conversion between error types (`From`). |
| Logging, metrics | tracing | `Log` (H15) for the program's own lines; the audit log covers host calls | No levels, structured fields or metrics. |
| Regex, text | regex | `split`, `lines`, `trim`, `find`, `replace`, `strip_prefix`, `contains`, `starts_with`, `slice`, `join`, `to_lowercase`/`to_uppercase` | No regex. |
| CLI parsing | clap | none | Needs an `Args` capability; the operator grants argv. |
| Floats | f64 | `Float` (f64, compiled at Rust's speed) | |
| Abstraction | traits, generics, closures | function values (`fn(T) -> R`, L6a), closures (L6b) and built-ins taking them (`map`, `filter`, `any`, `all`, `and_then`, `map_err`; L6c), compiled (C9); generic functions (L7a) and generic structs and enums (L7b, L7d), monomorphized, with bounds `T: Eq`/`T: Ord` (L7c) | No traits: a bound is `Eq` or `Ord` only, `Ord` for `Int` and `Str`. |
| Packages | cargo, crates.io | copy one file | A package manager, once there are a few libraries. |

The two that blocked a real backend first, an HTTP client beyond GET (H16) and SQL (H17), are built. `Http` under `--listen` (H19) is built too. What is left, in order: typed JSON decode, streaming bodies, a server database.

## A target app

A CRUD app over a `Dir` (W9a) proves the language. A target app proves the ecosystem: a small, real web service that an agent writes and an operator runs with exactly its grants. It is kio-web's links app (examples/links): a link shortener with a JSON API, a `Kv` store, a bearer token stored hashed, redirects that count hits, random slugs, expiring links, a log and HTML pages. Each piece it lacked became a task, as kio-web did for `--listen`: L11-L17, H12-H15, K2 and W14.

## Can models write kio?

The design bets that models write kio from language.md and the checker's hints. Evidence so far is agents with the docs in context (the links app). Unmeasured: a cold or smaller model.

Eval (E1, E2; eval/, 2026-11-10): 45 tasks (E1's 30: compute, `Dir`, `Kv`, `Db`, HTTP handlers and clients; E2's 15 harder: retries against a flaky upstream, idempotent replays across restarts, a ledger with overflow checks, a resumable sync, webhook signatures, a parser), up to 5 rounds with `kio check` and test feedback, through `claude -p`. kio gets language.md; Python gets nothing; "kio, no spec" gets one line ("a subset of Rust with capabilities as parameters").

| Model | kio first try | kio by round 5 | Python first try | Python by round 5 | kio, no spec, by round 5 | Cost kio / Python |
|---|---|---|---|---|---|---|
| Haiku 5.5 | 42/45 | 45/45 | 39/45 | 45/45 | 0/45 | $0.30 / $0.14 |
| Sonnet 5.5 | 43/45 | 45/45 | 44/45 | 45/45 | 1/45 | $2.10 / $0.93 |
| Opus 5.5 | 43/45 | 45/45 | 45/45 | 45/45 | 1/45 | $3.81 / $1.85 |

With the spec, kio matches Python: every model solves every task, and most first-try misses are logic errors Python's runs share (43, 25). kio-specific misses: `bool` for `Bool`, `to_str()` on a `Str`, an assignment as a match arm, a tuple; each fixed by the next round. No model asked for a grant the task did not give. Without the spec, models write Rust (`&str`, `Vec`, `i64`, macros, `as`, lifetimes, tuples) and the checker reports one parse error per round, so 5 rounds rarely suffice: the spec (`kio mcp`'s primer) is what makes models fluent. kio costs about 2x Python: language.md rides in every prompt.