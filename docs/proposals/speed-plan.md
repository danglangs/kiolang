# Speed plan: closing the gap to Rust on code agents write

Measured 2026-11-19 on main (d313202, after C8c3 and C8c4), Apple M3 Max, release, rustc 1.99.1, Wasmtime 50.0.1, Python 3.13.2. Other agents' builds and tests shared the machine (load 7-30 for the table below, 60-230 at times), so times are best of 12-15 interleaved rounds (each round runs every variant of every row once), startup subtracted per variant; differences under 10% are noise. Fuel is exact and load-free. "Fuel off" is a scratch host with `consume_fuel(false)` and `epoch_interruption(false)` (bench/README.md); epochs alone measured within noise of on.

## Numbers

Compute rows are bench/'s programs. Four rows are new, modelled on booking.kio and kio-web (scratch, not yet in bench/; task B1 below): `csv` loads 18.5 k order lines into a 6-field struct (`split`, `parse_int`, an email check, `to_lowercase`) and totals them per city in a `Map<Str, Agg>`; `logscan` parses 9.9 k access-log lines with a `for c in line.chars()` state machine building `key`/`val` with `val = val + c`, then counts statuses and sums durations per route in `Map`s; `records` runs `parse_json` on data.json and walks the 9 k records with `get(..).and_then(as_*)`; `logsplit` is `logscan` written with `split(" ")` and `strip_prefix`. Each prints one number its Rust (std, `HashMap`, serde_json) and Python twins match.

| Row | Rust | Python | kio | kio, fuel off | metering | fuel | vs Rust | roadmap table said |
|---|---|---|---|---|---|---|---|---|
| `fib(35)` | 24.5 ms | 719 ms | 36.5 | 37.6 | ~0 | 1.060 G | 1.5x | 2.3x (C8c3 since) |
| 30 M integer loop | 99 | 967 | 111 | 112 | ~0 | 2.100 G | 1.1x | 1.1x |
| Mandelbrot | 10.9 | 316 | 12.3 | 12.5 | ~0 | 0.285 G | 1.1x | 1.2x |
| 100 k `s = s + ..` | 3.0 | 7.8 | 4.5 | 3.3 | noise | 0.056 G | 1.5x (3-5 ms, noisy) | 1.1x |
| 1 M list push, sort, sums | 16.5 | 410 | 27.9 | 22.0 | 21% | 0.958 G | 1.7x | 1.7x |
| tree 2^20 | 33.4 | 3,427 | 64.9 | 50.4 | 22% | 1.300 G | 1.9x | 2.1x |
| `map`/`filter` closures, 1 M | 2.5 | 79 | 13.0 | 8.8 | 32% | 0.214 G | **5.2x** | 2.3x (Rust measured 5.8 then) |
| word count, `Map` | 4.8 | 17.8 | 23.0 | 17.8 | 23% | 0.340 G | **4.8x** | 3.6x (Rust 6.7 then) |
| word count, `HashMap` | 4.8 | 17.8 | 19.4 | 13.2 | 32% | 0.244 G | **4.1x** | 3.8x |
| JSON, json.kio | 8.1 | 20.7 | 40.8 | 29.2 | 28% | 0.619 G | **5.1x** | 7.3x |
| JSON, built-in | 7.7 | 20.7 | 12.4 | 8.9 | 28% | 0.180 G | 1.6x | 1.5x |
| `csv` (new) | 7.0 | 18.4 | 11.6 | 8.6 | 26% | 0.172 G | 1.7x | |
| `logscan`, `chars()` (new) | 5.1 | 93.5 | 18.7 | 14.0 | 25% | 0.299 G | **3.7x** | |
| `logsplit`, `split` (new) | 5.1 | | ~7.5 (from fuel) | | | 0.120 G | ~1.5x | |
| `records`, `Json` accessors (new) | 7.3 | 17.4 | 15.3 | 10.2 | 33% | 0.202 G | 2.1x | |

Startup and services (the parts the rows above leave out):

| | Rust | Python | kio | kio, fuel off |
|---|---|---|---|---|
| process start, `kio run` of an empty program | 2 ms | 14 ms | 36-39 ms (`kio check`: 3.6) | 29 ms |
| `csv` end to end (start + work) | 9 ms | 32 ms | **50 ms** | 38 ms |
| HTTP POST JSON (new: parse, validate 4 fields, 201 with JSON), server CPU per request | 14 µs | 58 µs | **309-320 µs** | 257 µs |
| same, `ab -k -c 12` req/s (load 14-66) | 183-187 k | 22.5 k | 37-38 k | 46 k |
| `GET /hello/kio` (kio-web), CPU per request | 12-13 µs | 63 µs | 159-163 µs | |
| a handler that only sends "ok", CPU per request | | | 128-131 µs | |
| `GET /hello/kio` req/s (run.py, load 69 to 15) | 229 k | 28.5 k | 50.0 k (table: 73.8 k at C8b4, quieter machine) | |

CPU per request is `ps` CPU time of the server over 50 k `ab -k -c 12` requests, robust to load where req/s is not. The JSON request's compiled code is 54 k fuel (about 3 µs); the hello request 33 k.

So: compute-only code is 1.1-1.5x Rust; allocation-heavy rows are 1.6-2.1x except five (closures 5.2x, `Map` 4.8x, json.kio 5.1x, `HashMap` 4.1x, `logscan` 3.7x). The two largest gaps on realistic work are outside the table: a request costs 22x Rust's CPU and 5x Python's, and every `kio run` spends 33 ms compiling the runtime.

## Where the time goes

Compute rows: `bench/profile.py` on 20x-scaled programs (a patched copy that also drops the attach stall when it is over 25% of samples), 140-1,000 samples each, so +-10%. Metering is inside every line and given apart above.

| Row | Top items (share of samples) |
|---|---|
| `Map` word count | `BTreeMap` search 52%, `split` 17%, `insert` 14%, alloc/free 9%, `lines` 3% |
| json.kio | freeing 24%, program code 22%, allocation 10%, conversions (`from_utf8`, `to_bytes`/`to_str`) 10%, `memory.copy` libcall + host `memmove` 8%, map inserts 7% |
| `csv` | `split` 31%, `contains` 21% (core's substring searcher set up per call, on 10-byte strings), freeing 10%, `Map` 10%, `memory.copy` 9% |
| `logscan` | `val = val + c` 51% (`rt_str_append`, `text::append`, `memory.copy` libcall 12.5%), `rt_str_eq` 11%, program 18%, freeing 7% |
| `records` | `parse_json` 40%, freeing the document 21%, accessors (`rt_json_get_u`, the receiver boxed per call by `variant1`, `fn` wrappers) 12%, map inserts 7% |
| closures | the two closures called through `fn`-value wrappers 55%, the loops 44% |
| tree | freeing 48%, `build` 30% (`rt_list_new` 11%), `fold` 22% |

`--listen`, POST JSON, `sample` of the server under `ab`, busy samples only (idle threads removed), JIT frames named from `kio-interp --native`:

| Item | Share | What it is |
|---|---|---|
| dlmalloc `malloc`/`memalign` self time | 29% | first touch of fresh pages (zero-fill faults land on the touching instruction): each size class's first 64 KB chunk is `memalign`ed per request, growing memory from 1.06 to 2.0-2.2 MB (`peak_memory`) |
| `mmap` at `Store` drop | 19% | the pool decommitting the instance's memory |
| `memmove` under `instantiate` | 10% | the 40 KB data image copied into fresh pages (Linux maps it copy-on-write instead) |
| `mprotect` | 10% | `memory.grow` (7%) and instance allocation (3%) |
| sockets, hyper, tokio | ~10% | |
| heap init, `cabi_realloc`, Wasmtime bookkeeping | ~6% | |
| the handler's compiled code | under 2% | 54 k fuel |

Per-request cost tracks memory touched, not work: the "ok" handler is 130 µs, hello 160, JSON 315, while their compiled code differs by 2-3 µs. Wasmtime's `linear_memory_keep_resident(4 MB)` (scratch host) cut JSON to 217-222 µs (-30%, req/s +38%) and the "ok" handler to 109-124; `mmap` at drop and `mprotect` for grown pages remain.

Two hypotheses tested and refuted:
- **`memory.copy` as a host call is the small-copy cost.** A runtime built without bulk memory (`-bulk-memory,-bulk-memory-opt`: copies through compiler_builtins' Wasm `memcpy`) made every string row slower (logscan +38%, strings +42%, `HashMap` +26%, json.kio +16%; tree -5%). The share is real, but only an inline path for short operands can win it.
- **Epoch checks cost time.** Within noise on every row.

## Ranked plan

Gain is on the rows above, predicted from the profile share or measured where it says so. Cost: S under 100 lines, M 100-400, L more.

| # | Change | Kind | Predicted gain | Cost | Evidence |
|---|---|---|---|---|---|
| 1 | **L1. Pool keeps instance memory resident** (`linear_memory_keep_resident`, `table_keep_resident` sized to the measured per-request footprint, 2 MB to start) | Wasmtime config | JSON request 315 to ~220 µs CPU, req/s +38%; "ok" handler 130 to ~115 | S | measured, scratch host |
| 2 | **L2. A request's heap fits its initial memory**: each size class's first chunk small (4-16 KB) and carved from pages the instance already has, `memalign` only past them; no `memory.grow` for a small request | runtime (heap.rs) | JSON request ~220 to ~130-150 µs (to the "ok" handler's floor), so ~2.2x requests per core with 1 | M | dlmalloc first touch + `mprotect` 39% of busy; JSON vs "ok" handler 315 vs 130 µs for 3 µs of code |
| 3 | **S1. The runtime compiled once, at build time**: its native code embedded in the `kio` binary (a separate core module, or Cranelift's incremental cache seeded at build), only the program compiled per run | host + codegen | `kio run` start 36 to ~8-10 ms; `csv` end to end 50 to ~22 ms (Python 32); every `kio test` and job likewise | M-L | empty program 36 ms, `kio check` 3.6, json.kio adds 6 |
| 4 | **C1. Closures inlined at `map`/`filter`/`any`/`all`/`sort_by_key` call sites** when the argument is a closure literal or a function item (C8c3's inliner, applied to the closure body) | codegen | closures 13 to ~6-7 ms (5.2x to ~2.6x); every `.map(\|x\| ..)` in agent code | S-M | wrappers and closure bodies 55% of samples |
| 5 | **T1. Short appends inline**: `s = s + c` / `s += c` with an operand of at most 8 bytes, a unique `s` and room, as two loads and stores in generated code (as list `push` is); a short-copy path in `text::append` that skips `memory.copy` | codegen + runtime | logscan 18.7 to ~11 ms (3.7x to ~2.2x); json.kio -5%; any hand-built string | M | append 51% of logscan; refuted test above says keep `memory.copy` for long copies |
| 6 | **T2. `contains`/`find`/`split` with a short needle**: a byte scan (memchr-style) for one-byte needles and short haystacks instead of core's two-way searcher | runtime (text.rs) | csv 11.6 to ~9.8 ms (-15%); every email/route check | S | `contains` 21% of csv |
| 7 | **C8c9. Fuel per loop and per call** | runtime + codegen; **maintainer decision** | -21 to -33% on every allocating row (`Map` 23 to 18, `HashMap` 19 to 13, tree 65 to 50, json.kio 41 to 29, csv 12 to 9, logscan 19 to 14, records 15 to 10, closures 13 to 9), `--listen` -18% CPU, `kio run` start -7 ms (less code to compile) | L | the fuel-off column is its ceiling, less ~5% for the counter |
| 8 | **J1. `Json` accessors take an unboxed receiver** (the `_u` with `(tag, payload)` noted after C8c1) | runtime + codegen | records -5% | S | receiver boxing 5% of records |
| 9 | **C8c5. Lists with the buffer inline** | runtime + codegen | tree ~-5 ms (to ~1.75x), `split`'s lists one allocation fewer (csv, word count -3-5%) | L | as pinned |
| 10 | **C8c6. Branchless reference counts** | codegen | fuel -3-5% on pointer rows, time within noise with fuel on | S | as pinned |
| 11 | **I1. Teach the fast idioms**: a `kio check` hint on a `for c in s.chars()` loop that builds a `Str` with `+`, and a guide table (`split`/`strip_prefix`/`find`/`slice` over character loops; `parse_json` over hand-written parsers; `HashMap` when order is not needed) | diagnostics + docs | logscan to logsplit: fuel 0.299 to 0.120 G (-60%, ~1.5x Rust); json.kio to built-in 41 to 12 ms | S-M | measured fuel; built-in JSON row |
| 12 | **B1. bench/ gains the four rows** (`csv`, `logscan`, `records`, HTTP POST JSON with CPU per request) with Rust/Python twins, and the gate tracks their fuel | bench | none; keeps the gap visible where agents work | S | |
| 13 | **M1. `Map` as a hash index with sorted order on iteration** (C8c7's "value store", re-pinned) | runtime | `Map` word count 23 to ~17 ms | L | search 52% of the row; `HashMap` is 19 today, so the ceiling is small |

Order to work in: 1, 12, 4, 6, 2, 5, 3, 8, then 9, 10, 13. Linux first for 1-2: production servers run there, Wasmtime maps the data image copy-on-write and resets memory with `madvise` on Linux, so the 10% data copy and the `mmap` share above may differ; measure CPU per request on the CI runner before and after.

## Rows that cannot reach 2x, and why

| Row | Best predicted | Why not 2x |
|---|---|---|
| word count, `Map` / `HashMap` | ~2.7-3.7x even with fuel off | Rust borrows each word (`&str`) into hashbrown; kio copies every word out of `split` (views were declined: memory) and searches twice per word in a runtime table |
| json.kio | ~3.6x with fuel off | the program's shape (a `Str` built per value, a byte at a time); the built-in parser is 1.6x, so the fix is I1 |
| HTTP per request | ~130 µs floor vs Rust's 13 | a fresh instance per request (web decisions 3-5, 9) costs memory setup that hyper never pays; throughput still beats Python by using every core. Below ~100 µs needs instance reuse (decision 2) |
| closures | ~2.5x after C1 | Rust fuses `map` and `filter` into one loop without the intermediate list |

Every other row is at or under 2.1x now, or reaches it with the changes above.

## Decisions for the maintainer

1. **C8c9, fuel per loop and per call** (one-way: what `--fuel` counts). It is now the largest single lever on every allocating row (21-33%) and on `--listen` (-18% CPU). #98 kept fuel as is. Recommend: take it after 1-6 land, so the gain is measured on code that is no longer wasteful.
2. **Instance reuse under `--listen`**: reset a used instance's heap instead of a fresh instance per request. The only route from ~130 µs to tens of µs per request; it weakens the per-request isolation the web decisions chose. Recommend: no for now; do L1/L2 and measure on Linux first.
3. **Native runtime code embedded in the binary (S1).** S3 removed the on-disk cache because loaded native code runs as the host and a program could plant it. Code built into the `kio` binary is not plantable, but it is loaded through Wasmtime's unsafe deserialize. Recommend: yes, with an adversarial review of how the bytes are produced and checked.
4. **Resident memory for the pool (L1)** trades idle RSS for request CPU: up to 2 MB per used slot (64 slots: up to 128 MB after a burst, against perf.md's 28 MB idle). Two-way and taken as such unless you object; RSS after load will be reported in its task entry.
