# Performance

Server and startup numbers. Compute benchmarks against Rust and Python, and the latest HTTP figure (73,800 req/s with keep-alive after C8b4, kio-web hello), are in [roadmap.md](roadmap.md) "Speed today".

## Compiled (C5, 2026-11-09)

Since C5 `kio run` and `--listen` run compiled code. Measured on an Apple Silicon Mac (16 cores), release builds, before (3f82d33, the interpreter guest) and after back to back, two rounds each, with another agent's build using most cores (load average 13-15): treat differences under 2x as approximate. Server: `kio run examples/serve/app.kio --listen`, `GET /hello/kio` (route and JSON), 20,000 requests per `ab` run, zero failed.

| | before (interpreter guest) | after (compiled) |
|---|---|---|
| `kio run` hello, wall | 5 ms | 29-31 ms (S3: every run compiles; no cache) |
| `kio run` primer (invoices.kio), wall | 6 ms | 30-38 ms |
| `kio check` primer | 5-9 ms | 3-8 ms |
| `--listen` startup to listening | 0.93-0.96 s | 0.16-0.18 s |
| idle `--listen` RSS after one request | 66-67 MB | 28-29 MB |
| `Server::handle` median (p99), serve example | 168-172 µs (450-620 µs) | 32 µs (39-81 µs) |
| `ab -c 12`, no keep-alive | 15,200-17,600 req/s, p99 7-10 ms | 17,800-20,200 req/s, p99 6-10 ms |
| `ab -c 12 -k` | 26,700-26,900 req/s, p99 2-3 ms | 43,000-53,400 req/s, p99 1-2 ms |
| fuel, one serve request | 1.79 M | 0.13 M |
| release binary | 34.5 MB | 24.3 MB |

- Every `kio run` compiles the runtime and program with Cranelift (about 25 ms). C5 to S3 cached the native code in `~/.cache/kio` (8 ms runs); S3 removed it, since loaded native code runs as the host and a program could plant it (open_questions #75).
- `--listen` compiles once on one thread: a parallel compile starts in about 30 ms but leaves about 20 MB more resident on macOS (W13d). On a quiet machine, before this change: startup 27 ms, idle RSS 49 MB, `ab -k` 60,600 req/s, no keep-alive 21,400 req/s.
- Without keep-alive the ceiling is still opening a TCP connection per request; with keep-alive, a request costs about 0.2 ms in all.
- The instance pool holds at least one instance per worker, its limits counted from the component (open_questions #135). Before, it counted core instances as if a component had one (it has three), so keep-alive clients past 21 got 503s (`ab -k -n 3000`, 16 cores: c=32 29 failed, c=64 416; with `--audit` 2,444 and 2,422); now 0 at c=32 and c=64, with or without `--audit`, RSS unchanged.
- Fuel defaults: 10 billion per `kio run` (a few seconds of compiled compute; fib(35) uses 1.1 billion), 10 million per request: see open_questions #76.
- `cargo test --release --test serve -- --nocapture` prints the `Server::handle` median; `cargo test --release --test idle_rss -- --nocapture` prints idle RSS.

## `--listen` request cost (C17, 2026-11-19)

Apple M3 Max, release, load 22-36 from other agents' builds. Five interleaved rounds; each runs every build on both workloads: 30,000 `ab -k -c 12` requests after 3,000 to warm up, server CPU time from `ps` (getrusage) over the 30,000. Hello is kio-web's `GET /hello/kio`; JSON is the speed plan's `POST /bookings` (parse, validate four fields, 201 with JSON). Best (mean) of five.

| Build | hello CPU a request | JSON CPU a request | hello req/s | JSON req/s |
|---|---|---|---|---|
| before (main, e0410e4) | 120 µs (131) | 139 µs (147) | 45.9 k | 40.9 k |
| `table_keep_resident` only | 112 (124) | 126 (134) | 50.1 k | 46.7 k |
| heap change only | 91 (96) | 84 (92) | 56.0 k | 56.8 k |
| C17 (both) | 82 (90) | 84 (88) | 52.4 k | 49.2 k |

- A request's peak memory: hello 1.9 MB to 1.25 MB, JSON 2.1 to 1.25 MB, the initial memory: neither grows it. 64 KB chunks put a fresh chunk per size class past the initial memory; with 8 KB chunks and 128 KB of free initial heap (`--initial-heap`) both fit.
- macOS: Wasmtime resets a slot's memory by mapping fresh zero pages over it whatever `linear_memory_keep_resident` says (the option is Linux-only), so the memory option does nothing here; the speed plan's -30% from it was `table_keep_resident`, set in the same scratch build. Less memory touched is what saves the `mmap` at drop and the page faults.
- Idle RSS of the JSON server (`ps`, 2 s after the last request; two runs each): after one request 39-41 MB before and after; after 20 k requests at `-c 12` 42-45 MB both; then 20 k at `-c 64` 48-51 MB both. On Linux each slot a burst used keeps up to the initial memory resident (1.25 MB; slots used are at most the 2-per-core workers), so up to about 40 MB more on 16 cores: measured with the Linux numbers.
- Linux (CPU per request, RSS, `pagemap_scan` on and off) is the next step: there memory reset is `memset` of the kept 1.25 MB instead of `madvise` (#322, #323).

## Regression gate

The baseline is CI's (Linux x86_64, the Rust pinned in rust-toolchain.toml: the runtime is compiled by it, so another Rust changes fuel). Fuel counts the program's work from `run` on, not Wasmtime's memory initialization (free on Linux, a page-padded copy elsewhere; #268), so the same Rust gives the same numbers on every OS.

CI's `perf` job runs `bench/gate.py` on every push and pull request: each `bench/kio` program once (the empty program is the `startup` row), with run.py's inputs and limits, compared with `bench/baseline.json`. Two rows have no Rust or Python twin and are gated only (bench/gate.py `ROWS`): `chars`, a 100-character `chars()` loop, and `map_insert`, 1,000 inserts of new `Str` keys (P9). Compiled `for c in s.chars()` builds no list: 53 fuel a character over 100 ASCII characters, 51 over 1,000 (startup subtracted; a byte loop about 29).

| Gated | Fails when |
|---|---|
| fuel (`used.fuel`, counted exactly, so load-free) | grows over 2% |
| peak memory (`used.peak_memory`) | grows over 2% |
| the printed number | differs at all (never waived) |
| the row set | a row is missing from the baseline or from the run |

Time (`ms`) is printed, not gated: runners are noisy. A row shrinking over 2% prints a note.

- Update: `python3 bench/gen_data.py && python3 bench/gate.py --update`, then commit `bench/baseline.json` (it records the `kio --version` it came from).
- Waive: name the rows in the commit message (a push's head commit; a pull request's title or body): `perf-ok: fib, tree` or `perf-ok: all`. Then update the baseline, or the next push fails again.
- Locally: `python3 bench/gate.py [--kio PATH] [--message TEXT]`; about 1.5 s on an Apple Silicon Mac after the release build.

## Interpreter guest (until C5)

History: the guest, wizer and the pre-initialized checked program no longer exist (C5).

Measured 2026-11-05 with ApacheBench on an Apple Silicon Mac, 12 cores, 32 GB, `kio` built in release. Server: `kio run examples/serve/app.kio --listen`, the kio-web example (route match and JSON, or a 2.7 KB file through a `Dir` grant). 20,000 requests per run, keep-alive off, zero failed requests in every run.

| Concurrency | Route + JSON (req/s) | Mean | p99 |
|---|---|---|---|
| 1 | 1,590 | 0.63 ms | 1 ms |
| 12 | 6,210 | 1.9 ms | 2 ms |
| 64 | 6,070 | 11 ms | 19 ms |

With keep-alive (`ab -k`) at concurrency 12: 24,100 req/s, p99 1 ms. So without keep-alive the ceiling is opening a TCP connection per request, not kio.

File serving at concurrency 12 (before W13b): 5,880 req/s, p99 2 ms. Host memory stayed at about 14 MB before W13b. Idle `--listen` RSS (macOS, `ps`, after the first request): about 11 MB at 2866620, 91-100 MB from W13b (footprint dirty 87 MB), about 53 MB after W13d (dirty 43 MB). Cause: startup allocates and frees about 90 MB (wizer's run on a parallel, optimizing Cranelift compile, then the in-process `precompile_component`), and macOS malloc keeps those freed pages resident and dirty (`vmmap`: MALLOC_SMALL (empty) 53 MB; `malloc_zone_pressure_relief`, a dropped engine, a thread that exits and `mi_collect` do not return them; a C test shows the same allocator keeps them only for this allocation pattern). Fix: do not allocate it. The wizer run uses an unoptimized single-pass engine (it only runs `init`; the output is recompiled) and both compiles are single-threaded on throwaway engines. Startup to listening goes from about 0.18 s to 0.61 s (single-threaded compile). The remaining ~40 MB over the old 11 is the one single-thread compile's peak. `cargo test --release --test idle_rss -- --nocapture` prints idle RSS.

Each request gets a fresh sandbox. The program is checked once at startup (W13b): the guest is pre-initialized with the checked program, so `Server::handle` takes a median of 175 µs on the serve example (500 µs when each request re-checked), at the cost of about 150 ms more startup. Throughput levels off at the core count, as expected for synchronous handlers run on a worker pool; extra concurrency only queues.

What this implies:
- The per-request sandbox is not the bottleneck at these rates; a fresh, isolated instance per request costs well under a millisecond.
- The next win is per-request work inside the guest. P2 (copy-on-write) and P3 (lowering; the interpreter on the IR is 3x faster) are done; the compiler replaced the interpreter at C5. Compute benchmarks against Rust and Python: docs/roadmap.md.
