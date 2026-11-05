# Performance

Measured 2026-11-05 with ApacheBench on an Apple Silicon Mac, 12 cores, 32 GB, `kio` built in release. Server: `kio run examples/serve/app.kio --listen`, the kio-web example (route match and JSON, or a 2.7 KB file through a `Dir` grant). 20,000 requests per run, keep-alive off, zero failed requests in every run.

| Concurrency | Route + JSON (req/s) | Mean | p99 |
|---|---|---|---|
| 1 | 980 | 1.0 ms | 1 ms |
| 12 | 5,960 | 2.0 ms | 2 ms |
| 64 | 5,820 | 11 ms | 17 ms |

File serving at concurrency 12: 5,880 req/s, p99 2 ms. Host memory stayed at about 14 MB.

Each request gets a fresh sandbox: about 0.5 ms of the 1 ms single-request latency is instantiation and the guest's check of the program (W7a). Throughput levels off at the core count, as expected for synchronous handlers run on a worker pool; extra concurrency only queues.

What this implies:
- The per-request sandbox is not the bottleneck at these rates; a fresh, isolated instance per request costs well under a millisecond.
- The next win is per-request work inside the guest: re-checking the program (fix: snapshot a checked guest, web decision 5) and the tree-walking interpreter (P2 copy-on-write, P3 lowering, then a bytecode VM).
