# Web support TODO

Goal: `kio-web`, a small but complete web framework written in kio, in its own repo. It is the first real program to drive the language: each feature below is added because kio-web needs it.

## How servers usually work, and what kio does

| | Rust (axum on tokio and hyper) | kio |
|---|---|---|
| Accepting connections | An event loop (epoll or kqueue) in the async runtime | The host: Rust, tokio and hyper, outside the sandbox |
| Concurrency | Async tasks on a work-stealing thread pool | Many guest instances at once, one per request, run by the host |
| Handler code | `async fn`, futures, `.await` | Plain synchronous kio. No async in the language |
| Shared state | `Arc<Mutex<..>>`, or state passed through extractors | None between requests; state lives behind capabilities (a `Dir` now, a database later) |
| Limits | Middleware, timeouts | Per-request fuel, memory, time and quotas, enforced by Wasmtime |

This is the model of WASI HTTP and of Wasm platforms such as Spin and Cloudflare Workers. It fits kio for three reasons:

- **No async, no shared mutable state.** The language stays as it is. Async would be a one-way door; per-request instances are a host choice that can change later.
- **Isolation per request.** A bug or attack in one request cannot see another's data.
- **Authority per route.** The handler's signature names its capabilities, so the report shows what each route can touch.

```rust
fn handle(req: Request, files: Dir) -> Response {
    match req.path.as_str() {
        "/" => text(200, "hello"),
        _ => match files.read(req.path) {
            Ok(body) => Response { status: 200, headers: [], body },
            Err(_) => text(404, "not found"),
        },
    }
}
```

```
handle : files read
```

## Decisions to make with the owner first

These change `caps.wit` or the language, so each is pinned with the owner before any task starts.

1. **Entry point.** A server program exports `handle(req, ..caps) -> Response` instead of `main`. Rule 3 changes from "`main` is the only entry point" to "`main` or `handle`".
2. **Request and response shape.** WIT records `request { method, path, query, headers, body }` and `response { status, headers, body }`, mapped to kio structs. Headers are `list<tuple<string, string>>`, a `List` of a two-field struct in kio. Alternative: adopt `wasi:http` types directly, which are richer but use resources and streams kio does not have.
3. **Instance per request.** A fresh guest instance per request, using Wasmtime's pooling allocator so creating one takes microseconds. Alternative: reuse instances. That is faster, but state could leak between requests.
4. **Modules.** kio-web is a library, so programs need more than one file: `mod` and `use`, as sketched in design.md's open questions.

## Tasks

Each task is pinned and split further before handoff, as in lang-todo.md.

- [ ] **W1. Records in caps.wit.** capgen maps WIT records to kio structs. Prerequisite for `request` and `response`.
- [ ] **W2. Server world.** `world server` in caps.wit exports `handle`; the checker accepts `handle` as an entry point and the report covers it (decisions 1 and 2).
- [ ] **W3. `kio serve`.** The host accepts HTTP with tokio and hyper and runs one guest instance per request with per-request limits (decision 3). `kio serve app.kio --port 8080 --dir files=./public`.
- [ ] **W4. Modules.** Multi-file programs with `mod` and `use` (decision 4). The host reads the files and passes them to the guest.
- [ ] **W5. String building and parsing.** What kio-web needs to parse queries and write JSON: `Str` slicing, characters or bytes by index, `Int.parse` with errors, string escapes. Scoped by what the kio-web code actually uses.
- [ ] **W6. kio-web repo.** A router (`match` on method and path), request helpers (query, headers), response helpers (text, JSON, status codes), a JSON encoder, and an example app with a route that serves files from a granted `Dir`.
- [ ] **W7. Benchmark.** Requests per second and latency for the example app. Decides when the bytecode VM and later a compiler move up, together with G4's stack measurement.

## Later, driven by kio-web

- **Function values.** Routes as data (`router.get("/", home)`) need first-class function names. `match` routing works without them.
- **Outbound calls from handlers.** The existing `Http` capability, allowlisted per route.
- **Streaming bodies.** Needs a stream type in the language and WIT streams.
- **Databases.** A new capability type in caps.wit, such as `Kv` or `Sql`.
