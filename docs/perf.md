# Performance

Measured 2026-11-05 with ApacheBench on an Apple Silicon Mac, 12 cores, 32 GB, `kio` built in release. Server: `kio run examples/serve/app.kio --listen`, the kio-web example (route match and JSON, or a 2.7 KB file through a `Dir` grant). 20,000 requests per run, keep-alive off, zero failed requests in every run.

| Concurrency | Route + JSON (req/s) | Mean | p99 |
|---|---|---|---|
| 1 | 1,590 | 0.63 ms | 1 ms |
| 12 | 6,210 | 1.9 ms | 2 ms |
| 64 | 6,070 | 11 ms | 19 ms |

With keep-alive (`ab -k`) at concurrency 12: 24,100 req/s, p99 1 ms. So without keep-alive the ceiling is opening a TCP connection per request, not kio.

File serving at concurrency 12 (before W13b): 5,880 req/s, p99 2 ms. Host memory stayed at about 14 MB.

Each request gets a fresh sandbox. The program is checked once at startup (W13b): the guest is pre-initialized with the checked program, so `Server::handle` takes a median of 175 µs on the serve example (500 µs when each request re-checked), at the cost of about 150 ms more startup. Throughput levels off at the core count, as expected for synchronous handlers run on a worker pool; extra concurrency only queues.

What this implies:
- The per-request sandbox is not the bottleneck at these rates; a fresh, isolated instance per request costs well under a millisecond.
- The next win is per-request work inside the guest: the tree-walking interpreter (P2 copy-on-write, P3 lowering, then a bytecode VM).
