# Performance

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
- The next win is per-request work inside the guest: the tree-walking interpreter (P2 copy-on-write, P3 lowering, then a bytecode VM).
