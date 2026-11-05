# Web support TODO

Goal: `kio-web`, a small but complete web framework written in kio, in its own repo. It is the first real program to drive the language: each feature below is added because kio-web needs it.

## How servers usually work, and what kio does

| | axum / rocket | Spin, wasmCloud | Cloudflare Workers | kio |
|---|---|---|---|---|
| Accept | tokio + hyper in the app | Host (Wasmtime) | Host (V8 runtime) | Host: tokio + hyper, outside the sandbox |
| Concurrency | Async tasks, work-stealing pool | Fresh instance per request ([Spin](https://dev.to/matei_radu/spin-10-the-developer-tool-for-serverless-webassembly-2bhk), [wasmCloud](https://wasmcloud.com/blog/wasmcloud-2-6-release/) by default; warm pools opt-in) | Isolates reused across requests, globals leak between them ([docs](https://developers.cloudflare.com/workers/about/how-it-works)) | Fresh guest instance per request |
| Handler | `async fn` + extractors | `wasi:http` handler export | `async fetch(req, env)` | Synchronous kio `fn handle` |
| Routing | `Router` / `#[get]` + `mount` | Manifest routes, most specific wins ([Spin](https://github.com/spinframework/spin-docs/blob/main/content/v3/http-trigger.md)) | In code | In kio code |
| State | `State<T>`, `Arc<Mutex<..>>` | KV/SQL host APIs | Bindings (`env`), KV, D1 | Capabilities only (`Dir` now, `Kv` later) |
| Limits | tower middleware | Fuel, memory, epochs | CPU time, memory | Fuel, memory, epoch, quotas per request |

**Decided with the owner:** one fresh guest instance per request; synchronous kio handlers, no async in the language; the host does accept and concurrency. Consequences: no state leaks between requests, and each handler's authority is its signature, so the report shows what each route can touch.

**Cost of fresh instances.** Wasmtime's pooling allocator plus copy-on-write memory cut SpiderMonkey's instantiation from 2 ms to 5 µs ([Wasmtime 1.0](https://bytecodealliance.org/articles/wasmtime-10-performance)). `wasmtime serve` on a hello-world went from about 5k to 35k-43k requests/s with pooling on 4- and 16-core Linux machines, and 236k on a maintainer's; the remaining cost is resetting memory (`memset`, `madvise`) ([wasmtime#8034](https://github.com/bytecodealliance/wasmtime/issues/8034)). The kio guest uses 1.1-1.5 MB of linear memory (G4), so reset is small. The real per-request cost is the guest re-parsing and re-checking the program; W10 measures it.

## Sketch: kio-web v0 core

Checked with today's `kio check` (record structs declared inline, `pub`, `mod`, `web::` stripped). It compiles except the lines marked with a task number.

```rust
// web.kio: the library, vendored into the app. Header, Request, Response come from caps.wit.  // W1
pub fn respond(status: Int, ctype: Str, body: Bytes) -> Response {                            // W4: pub
  let mut hs: List<Header> = [];
  hs.push(Header { name: "content-type", value: ctype });
  Response { status: status, headers: hs, body: body }
}
pub fn text(status: Int, s: Str) -> Response { respond(status, "text/plain; charset=utf-8", s.to_bytes()) }
pub fn json(status: Int, s: Str) -> Response { respond(status, "application/json", s.to_bytes()) }
pub fn not_found() -> Response { text(404, "not found") }

// Match "/hello/:name"; Some(params) on a hit.
pub fn route(req: Request, method: Str, pattern: Str) -> Option<Map<Str, Str>> {
  if req.method != method { return None; }
  let want = pattern.split("/");
  let got = req.path.split("/");
  if want.len() != got.len() { return None; }
  let mut params: Map<Str, Str> = Map::new();
  for i in 0..want.len() {
    if want[i].starts_with(":") { params.insert(want[i], got[i]); }
    else if want[i] != got[i] { return None; }
  }
  Some(params)
}

pub fn query(req: Request) -> Map<Str, Str> {
  let mut m: Map<Str, Str> = Map::new();
  for pair in req.query.split("&") {
    let kv = pair.split("=");
    if kv.len() == 2 { m.insert(kv[0], kv[1]); }
  }
  m
}

pub fn json_str(s: Str) -> Str {
  let b = s.to_bytes();
  let mut out = "\"".to_bytes();
  for i in 0..b.len() {
    let c = b[i];                                                                              // W2: index
    if c == 34 || c == 92 { out.push(92); }                                                    // W2: push
    out.push(c);
  }
  out.push(34);
  out.to_str()
}

pub fn file(files: Dir, name: Str) -> Response {
  let parts = name.split(".");
  let ctype = match parts[parts.len() - 1] { "css" => "text/css", "html" => "text/html", _ => "application/octet-stream" };
  match files.read(name) {
    Ok(b) => respond(200, ctype, b),
    Err(e) => match e { IoErr::NotFound => not_found(), _ => text(500, "read failed") },
  }
}

// app.kio: `kio serve app.kio --dir public=./public`
mod web;                                                                                       // W4

fn handle(req: Request, public: Dir) -> Response {                                            // W5
  match web::route(req, "GET", "/hello/:name") {                                               // W4: paths
    Some(p) => { return web::json(200, "{\"hello\": " + web::json_str(p.get(":name").unwrap_or("")) + "}"); } // W3
    None => {}
  }
  match web::query(req).get("file") { Some(f) => web::file(public, f), None => web::not_found() }
}
```

```
file   : files read
handle : public read
```

Found while writing it: routing, path params, query maps, status helpers and file serving need nothing new. JSON needs byte access. A `Json` value enum is rejected ("enum `Json` contains itself"), so v0 encodes by building strings and does not decode.

## Decisions to pin with the owner

Each changes `caps.wit` or the language. Recommendation first, then the alternative.

1. **Entry point.** `fn handle(req: Request, ..caps) -> Response`; rule 3 becomes "`main` or `handle`, exactly one". Caps are bound by name with `--dir`, as for `main`. Alt: `main` returns a `Response` and reads the request from a cap; that hides the request's type from the checker.
2. **Request and response: plain WIT records, not `wasi:http`.** In a new `web` interface:
   `record header { name: string, value: string }`,
   `record request { method: string, path: string, query: string, headers: list<header>, body: list<u8> }`,
   `record response { status: u16, headers: list<header>, body: list<u8> }`.
   Host-side rules: `method` uppercase as sent; `path` and `query` raw (percent-encoded, no `?`); header names lowercased, duplicates kept in order; a request with non-UTF-8 headers or a body over the cap is answered 400/413 without starting a guest. Responses: status outside 100-599, or a header hyper's `HeaderValue` rejects (CR, LF), becomes 500; the host sets `content-length`.
   Why not `wasi:http`: 0.3 (final [June 2026](https://wasi.dev/releases/wasi-p3), Wasmtime 46+) has `handle: async func(request) -> result<response, error-code>` with `request`/`response` resources, `stream<u8>` bodies and `future` trailers; 0.2 has nine resources, `wasi:io` streams and a `response-outparam` ([types.wit](https://github.com/WebAssembly/wasi-http/blob/main/wit/types.wit)). Adopting either needs non-capability resources in kio values, a stream type, and the program baked into the component, since `handle` takes no source. Records need only W1. Alt, later: the guest crate also exports `wasi:http/handler@0.3`, adapting to the same records, so kio apps run under `wasmtime serve`, Spin or wasmCloud unchanged.
3. **Buffered bodies.** The host reads the whole request body (cap 1 MB) before starting the guest and writes the response after the instance is dropped. A slow client never holds an instance. Alt: streaming, which needs decision 2's alternative; see Later.
4. **Execution.** Sync Wasmtime (as `kio run` today) on `tokio::task::spawn_blocking`, bounded by a semaphore (default: 2 × CPUs); when it is full, queue up to 100 more for 1 s, then 503. Engine, `Component` and `InstancePre` are built once at startup; today `Kio::with_stack` compiles the component on every call. Alt: Wasmtime async with epoch yields; needed once handlers call `Http`, so a slow upstream does not pin a thread.
5. **Checking.** `kio serve` runs `check` once at startup, prints the report and refuses to start on a diag. Each request re-parses and re-checks inside its fresh instance. Alt, if W10 shows it dominates: snapshot a checked guest per program with [`wasmtime wizer`](https://docs.wasmtime.dev/api/wasmtime_wizer/index.html) so instances start with `Program` already in copy-on-write memory.
6. **Modules.** Rust's own syntax: `mod web;` loads `web.kio` beside the file; `pub fn` / `pub struct`; `web::f`, `web::T`. No `use` in v0. `check`, `run` and `serve` take `list<record file { path: string, source: string }>`; `Span` gains a file id. kio-web is vendored as one file: no package manager. Alt: concatenate files on the host; no language change, but diags and names collide.
7. **Routing.** A `route` helper and `match`/early return, as in the sketch. Alt: route tables (`router.get("/", home)`), which need function values; see Later.
8. **JSON.** Encode only: `json_str`, plus builders over `List<Field>` with `Str` values already encoded. Decoding waits for recursive types or an index-based tree.
9. **Per-request limits** (overridable flags): fuel 100 million, memory 64 MB, wall clock 5 s, host calls and I/O as `kio run`; hyper `header_read_timeout` 10 s (default 30 s), `max_headers` 100, header bytes 64 KB, body read deadline 10 s, idle keep-alive 30 s ([hyper http1 Builder](https://docs.rs/hyper/latest/hyper/server/conn/http1/struct.Builder.html)). A trap or limit is 500 with no detail to the client; the diag goes to the log. HTTP/1.1 only in v0.

## Tasks

`[parallel]` tasks touch only `lang` or `tools/capgen` and can run alongside H1-H5, N1-N2 and M1. Each is pinned and split further before handoff, as in lang-todo.md.

- [ ] **W1. Records in caps.wit.** `[parallel]` capgen maps a WIT `record` to a kio struct declared in every program (`header` to `Header`, `u16` to `Int`, `list<u8>` to `Bytes`); add decision 2's `web` interface. A user struct with the same name is an error. Security-review event.
  Done: `Header { name: "a", value: "b" }` checks with no declaration; a duplicate user `struct Request` is rejected with a hint; CI's capgen drift check passes.
- [x] **W2. `Bytes` index and push.** `[parallel]` `b[i] -> Int` (0-255); `push(Int)` on a `let mut` `Bytes`, runtime error outside 0-255. Out-of-bounds index is the existing runtime error.
  Done: `json_str` from the sketch and a percent-decoder run in tests; push of 256 and index past the end each stop with their error.
- [x] **W3. `Option.unwrap_or(T)`.** `[parallel]`
  Done: tests for `Some` and `None`, and a type-mismatch rejection with a hint.
- [ ] **W4. Modules.** `[parallel]` Decision 6. Split by pass: parse (`mod`, `pub`, `a::b`), resolve and check (unknown module, private item, cycles), then the API change to a file list.
  Done: a two-file program runs; a diag in `web.kio` prints `web.kio:line`; calling a non-`pub` function and a `mod` cycle are each rejected with a hint; the report names functions as `web::file`.
- [ ] **W5. `handle` entry point.** After W1. Rule 3 as in decision 1; the report covers `handle` and its helpers; `lang::run_handle(p, req, caps) -> Result<Value, RunErr>`.
  Done: a program with both `main` and `handle`, one with neither, and `handle` with a wrong signature are each rejected with a hint; a native test returns a `Response` value for a `Request`.
- [ ] **W6. Guest `serve` export.** After W4, W5. `serve(files, req, dirs, https) -> result<response, list<diag>>` in world `guest`.
  Done: with G3's in-memory host, the sketch app answers `/hello/kio` with `{"hello": "kio"}`; a runtime error is a diag with its span.
- [ ] **W7. `kio serve`.** After W6, H2, H3. tokio + hyper http1 + Wasmtime pooling allocator; decisions 4 and 5. `kio serve app.kio --port 8080 --dir public=./public`.
  Done: an integration test with a real client gets the right body for three routes, and 100 concurrent requests all succeed; a program with a diag exits 1 before binding the port.
- [ ] **W8. Request limits and slow clients.** After W7. Decisions 3 and 9, and the request/response validation in decision 2.
  Also: cap detached host-call workers (H5 leaves one blocked thread per timed-out call); at the cap, answer 503.
  Done: one test per limit (repeated blocked reads hit the worker cap and get 503, headers stalled past the timeout, oversized body, body sent too slowly, fuel, memory, a full queue answering 503, a response header containing CR LF answering 500); none leaves an instance alive.
- [ ] **W9. kio-web v0 repo.** After W1-W5 for the library; W7 for the example. `web.kio`: `respond`, `text`, `json`, `html`, `redirect`, `not_found`, `route`, `query` (percent-decoded), `header`, `json_str`, `json_obj`, `file`. Tests are kio programs whose `main` builds a `Request`, calls `handle` and checks the `Response`.
  Done: tests pass under `kio run`; an example app serves a page, a JSON route with a path parameter and files from a granted `Dir`, and its README shows the report.
- [ ] **W10. Benchmark.** After W9. Requests/s and p50/p99 for the example app, with time split into instantiate, check and handle. Decides decision 5's snapshot, the bytecode VM, and copy-on-write values: v0 copies `req`, body included, into every helper call.
  Done: numbers and the decisions they force recorded in open_questions.
- [ ] **W11. Adversarial serve review.** After W8. As H5: header and response splitting, oversized and malformed requests, slow clients, path escapes through `file`.
  Done: every finding fixed or logged in open_questions.

## Later, driven by kio-web

- **Function values.** Route tables and middleware as wrappers (`with_auth(handler)`). Today middleware is a call at the top of `handle`; timeouts, body limits and logging are host concerns.
- **JSON decoding.** Needs recursive types or an index-based tree; typed decoding needs generics.
- **`Result<Response, Response>` from `handle`,** so `?` works in handlers.
- **Nested patterns** (`Err(IoErr::NotFound)`); today a nested `match`.
- **Outbound calls.** The `Http` capability, allowlisted per route; moves execution to Wasmtime async (decision 4).
- **Streaming bodies, HTTP/2, `wasi:http` export.** Decision 2's alternative.
- **Databases.** A `Kv` or `Sql` capability in caps.wit.
- **Cookies, forms, multipart, websockets.**
