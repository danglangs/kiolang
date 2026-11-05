# Web support TODO

Goal: `kio-web`, a small but complete web framework written in kio, in its own repo. It is the first real program to drive the language: each feature below is added because kio-web needs it.

## How servers usually work, and what kio does

| | axum / rocket | Spin, wasmCloud | Cloudflare Workers | kio |
|---|---|---|---|---|
| Accept | tokio + hyper in the app | Host (Wasmtime) | Host (V8 runtime) | Host: tokio + hyper, outside the sandbox |
| Concurrency | Async tasks, work-stealing pool | Fresh instance per request ([Spin](https://dev.to/matei_radu/spin-10-the-developer-tool-for-serverless-webassembly-2bhk), [wasmCloud](https://wasmcloud.com/blog/wasmcloud-2-6-release/) by default; warm pools opt-in) | Isolates reused across requests, globals leak between them ([docs](https://developers.cloudflare.com/workers/about/how-it-works)) | Fresh guest instance per request |
| Handler | `async fn` + extractors | `wasi:http` handler export | `async fetch(req, env)` | Synchronous kio `main(req: Incoming, res: Outgoing, ..)` |
| Routing | `Router` / `#[get]` + `mount` | Manifest routes, most specific wins ([Spin](https://github.com/spinframework/spin-docs/blob/main/content/v3/http-trigger.md)) | In code | In kio code |
| State | `State<T>`, `Arc<Mutex<..>>` | KV/SQL host APIs | Bindings (`env`), KV, D1 | Capabilities only (`Dir` now, `Kv` later) |
| Limits | tower middleware | Fuel, memory, epochs | CPU time, memory | Fuel, memory, epoch, quotas per request |

**Decided with the owner:** servers start with `kio run app.kio --listen addr`, not a separate command (a listening socket is a grant, like `--dir`); one fresh guest instance per request; synchronous kio handlers, no async in the language; the host does accept and concurrency; plain WIT records, not `wasi:http` (decision 2); the request and response as capabilities, with `main` the only entry point (decision 1); Rust's module syntax (decision 6). Consequences: no state leaks between requests, and each handler's authority is its signature, so the report shows what each route can touch.

**Cost of fresh instances.** Wasmtime's pooling allocator plus copy-on-write memory cut SpiderMonkey's instantiation from 2 ms to 5 µs ([Wasmtime 1.0](https://bytecodealliance.org/articles/wasmtime-10-performance)). `wasmtime serve` on a hello-world went from about 5k to 35k-43k requests/s with pooling on 4- and 16-core Linux machines, and 236k on a maintainer's; the remaining cost is resetting memory (`memset`, `madvise`) ([wasmtime#8034](https://github.com/bytecodealliance/wasmtime/issues/8034)). The kio guest uses 1.1-1.5 MB of linear memory (G4), so reset is small. Measured today: `Kio::new` takes 52 ms, almost all of it compiling the guest, which `kio run --listen` does once at startup; checking `summarize.kio` in the guest takes 40 µs. So the per-request overhead after startup is instantiation plus check, roughly 50-100 µs. W10 measures it end to end.

## Sketch: kio-web v0 core

kio-web owns the types; only raw parts cross the wall (decision 1). Lines marked `// W4` need modules; everything else checks today.

```rust
// web.kio: the library, vendored into the app.
pub struct Header { name: Str, value: Str }                                                  // W4: pub
pub struct Request { method: Str, path: Str, query: Str, headers: List<Header>, body: Bytes }
pub struct Response { status: Int, headers: List<Header>, body: Bytes }

// Build a Request from the raw parts the host hands over.
pub fn request(req: Incoming) -> Request {
  let target = req.target().split("?");
  let names = req.header_names();
  let values = req.header_values();
  let mut hs: List<Header> = [];
  for i in 0..names.len() { hs.push(Header { name: names[i], value: values[i] }); }
  let query = if target.len() > 1 { target[1] } else { "" };
  Request { method: req.method(), path: target[0], query: query, headers: hs, body: req.body() }
}

// Send a Response back through the host.
pub fn send(res: Outgoing, r: Response) -> Result<(), WebErr> {
  let mut names: List<Str> = [];
  let mut values: List<Str> = [];
  for h in r.headers { names.push(h.name); values.push(h.value); }
  res.send(r.status, names, values, r.body)
}

pub fn respond(status: Int, ctype: Str, body: Bytes) -> Response {
  let mut hs: List<Header> = [];
  hs.push(Header { name: "content-type", value: ctype });
  Response { status: status, headers: hs, body: body }
}
pub fn text(status: Int, s: Str) -> Response { respond(status, "text/plain; charset=utf-8", s.to_bytes()) }
pub fn json(status: Int, s: Str) -> Response { respond(status, "application/json", s.to_bytes()) }
pub fn not_found() -> Response { text(404, "not found") }

// Match "/hello/:name"; Some(params) on a hit.
pub fn route(r: Request, method: Str, pattern: Str) -> Option<Map<Str, Str>> {
  if r.method != method { return None; }
  let want = pattern.split("/");
  let got = r.path.split("/");
  if want.len() != got.len() { return None; }
  let mut params: Map<Str, Str> = Map::new();
  for i in 0..want.len() {
    if want[i].starts_with(":") { params.insert(want[i], got[i]); }
    else if want[i] != got[i] { return None; }
  }
  Some(params)
}

pub fn json_str(s: Str) -> Str {
  let b = s.to_bytes();
  let mut out = "\"".to_bytes();
  for i in 0..b.len() {
    let c = b[i];
    if c == 34 || c == 92 { out.push(92); }
    out.push(c);
  }
  out.push(34);
  out.to_str()
}

pub fn file(files: Dir, name: Str) -> Response {
  match files.read(name) {
    Ok(b) => respond(200, "application/octet-stream", b),
    Err(e) => match e { IoErr::NotFound => not_found(), _ => text(500, "read failed") },
  }
}

// app.kio: `kio run app.kio --dir public=./public --listen 127.0.0.1:8080`
mod web;                                                                                     // W4

fn main(req: Incoming, res: Outgoing, public: Dir) -> Result<(), WebErr> {
  let r = web::request(req);                                                                 // W4: paths
  let out = match web::route(r, "GET", "/hello/:name") {
    Some(p) => web::json(200, "{\"hello\": " + web::json_str(p.get(":name").unwrap_or("")) + "}"),
    None => web::file(public, r.path),
  };
  web::send(res, out)
}
```

```
main    : req body header_names header_values method target, res send, public read
request : req body header_names header_values method target
send    : res send
file    : files read
```

Found while writing it: routing, path params, status helpers and file serving need nothing new. A `Json` value enum is rejected ("enum `Json` contains itself"), so v0 encodes JSON by building strings and does not decode.

## Decisions to pin with the owner

Each changes `caps.wit` or the language. Recommendation first, then the alternative.

1. **How a request reaches the program (decided).** No language change: `main` stays the only entry point, and HTTP is a capability carrying only the raw bytes across the wall, as hyper does for axum. `caps.wit` gains:
   ```wit
   interface web {
     resource incoming {
       method: func() -> string;                 // "GET"
       target: func() -> string;                 // raw "/path?query", as sent
       header-names: func() -> list<string>;     // lowercased, in order
       header-values: func() -> list<string>;
       body: func() -> list<u8>;
     }
     resource outgoing {
       send: func(status: u16, header-names: list<string>, header-values: list<string>, body: list<u8>) -> result<_, web-err>;
     }
     variant web-err { already-sent, invalid }
   }
   ```
   A server program is `fn main(req: Incoming, res: Outgoing, public: Dir) -> Result<(), IoErr>`, and the host calls `main` once per request with fresh grants. `Request`, `Response`, `Header`, routing and helpers are kio-web types built from these parts; changing them never touches `caps.wit`. Headers are two parallel lists because kio has no tuples. The report shows each handler's access: `main : req method target, res send, public read`.
2. **Not `wasi:http` (decided).** `wasi:http` 0.3 (final [June 2026](https://wasi.dev/releases/wasi-p3), Wasmtime 46+) has `handle: async func(request) -> result<response, error-code>` with resources, `stream<u8>` bodies and `future` trailers; 0.2 has nine resources and `wasi:io` streams ([types.wit](https://github.com/WebAssembly/wasi-http/blob/main/wit/types.wit)). Adopting it needs non-capability resources, a stream type and the program baked into the component. Later, the guest can also export `wasi:http/handler`, adapting to decision 1's parts, so kio apps run under `wasmtime serve`, Spin or wasmCloud. Host-side rules for decision 1: `method` uppercase as sent; `target` raw; header names lowercased, duplicates kept in order; non-UTF-8 headers or a body over the cap is answered 400/413 without starting a guest; a response status outside 100-599 or a header with CR or LF becomes 500; the host sets `content-length`.
3. **Buffered bodies.** The host reads the whole request body (cap 1 MB) before starting the guest and writes the response after the instance is dropped. A slow client never holds an instance. Alt: streaming, which needs decision 2's alternative; see Later.
4. **Execution.** Sync Wasmtime (as `kio run` today) on `tokio::task::spawn_blocking`, bounded by a semaphore (default: 2 × CPUs); when it is full, queue up to 100 more for 1 s, then 503. Engine, `Component` and `InstancePre` are built once at startup; today `Kio::with_stack` compiles the component on every call. Alt: Wasmtime async with epoch yields; needed once handlers call `Http`, so a slow upstream does not pin a thread.
5. **Checking.** `kio run --listen` runs `check` once at startup, prints the report and refuses to start on a diag. Each request re-parses and re-checks inside its fresh instance. Alt, if W10 shows it dominates: snapshot a checked guest per program with [`wasmtime wizer`](https://docs.wasmtime.dev/api/wasmtime_wizer/index.html) so instances start with `Program` already in copy-on-write memory.
6. **Modules (decided).** Rust's own syntax: `mod web;` appears once, in the file that owns the module, and loads `web.kio` beside it; other files refer to `web::f` without declaring it again; `pub fn` / `pub struct`; `web::f`, `web::T`. No `use` in v0. `check`, `run` and `serve` take `list<record file { path: string, source: string }>`; `Span` gains a file id. kio-web is vendored as one file: no package manager. Alt: concatenate files on the host; no language change, but diags and names collide.
7. **Routing.** A `route` helper and `match`/early return, as in the sketch. Alt: route tables (`router.get("/", home)`), which need function values; see Later.
8. **JSON.** Encode only: `json_str`, plus builders over `List<Field>` with `Str` values already encoded. Decoding waits for recursive types or an index-based tree.
9. **Per-request limits** (overridable flags): fuel 100 million, memory 64 MB, wall clock 5 s, host calls and I/O as `kio run`; hyper `header_read_timeout` 10 s (default 30 s), `max_headers` 100, header bytes 64 KB, body read deadline 10 s, idle keep-alive 30 s ([hyper http1 Builder](https://docs.rs/hyper/latest/hyper/server/conn/http1/struct.Builder.html)). A trap or limit is 500 with no detail to the client; the diag goes to the log. HTTP/1.1 only in v0.

## Tasks

`[parallel]` tasks touch only `lang` or `tools/capgen` and can run alongside H1-H5, N1-N2 and M1. Each is pinned and split further before handoff, as in lang-todo.md.

- [x] **W1. The `web` interface in caps.wit.** `[parallel]` Add decision 1's `incoming`, `outgoing` and `web-err` exactly as written, import `web` in world `guest`, and add `incoming-grant`/`outgoing-grant` lists to `run`. capgen maps them like `Dir` (resources become capability types, `u16` becomes `Int`, `list<string>` becomes `List<Str>`). Security-review event.
  Done: `fn main(req: Incoming, res: Outgoing)` checks and its report reads `main : req method target, res send` for a program calling those; `Incoming` cannot be stored or returned (rule 1); CI's capgen drift check passes.

- [x] **W2. `Bytes` index and push.** `[parallel]` `b[i] -> Int` (0-255); `push(Int)` on a `let mut` `Bytes`, runtime error outside 0-255. Out-of-bounds index is the existing runtime error.
  Done: `json_str` from the sketch and a percent-decoder run in tests; push of 256 and index past the end each stop with their error.
- [x] **W3. `Option.unwrap_or(T)`.** `[parallel]`
  Done: tests for `Some` and `None`, and a type-mismatch rejection with a hint.
- [x] **W4a. Module syntax.** `[parallel]` Parser only. `mod name;` as an item; `pub` before `fn`, `struct`, `enum`; paths `name::item` in expressions, types and patterns, extending today's `Enum::Variant` path. `mod`, `pub` leave the reserved list; `use` stays reserved.
  Done: parse tests for each form; `use` is rejected with hint "write the full path, like `web::text`".
- [ ] **W4b. Modules in the checker and interpreter.** `[parallel]` After W4a. v0 is flat: a module is one file, only the root file may declare `mod`, so there are no nested modules and no cycles. Items in a module are visible outside it only if `pub`; the root's own items need no `pub`. Function, struct and enum names are resolved per module; `web::text` calls `web.kio`'s `text`. `Span` gains `file: u32` (0 is the root). The report names module functions `web::text`.
  Done: a two-file program runs natively; calling a non-`pub` function, `mod` outside the root, an unknown module, and a duplicate `mod` are each rejected with a hint; a diag in `web.kio` carries its file.
- [ ] **W4c. Files through the API and host.** After W4b. `lang::check_files(&[(name, source)])` with the root first, and `check(src)` as its one-file case. caps.wit's `check` and `run` take `list<record file { name: string, source: string }>` (security-review event). The host reads `name.kio` beside the root file for each `mod name;`, and prints diags as `web.kio:line`.
  Done: `kio run app.kio` with `mod web;` runs; a diag inside `web.kio` prints `web.kio:3`; a `mod` naming a missing file is a clean error.

- [x] **W5. `Incoming` and `Outgoing` capabilities.** Delivered by W1: both are checked like `Dir` (parameters only), a second `send` is `already-sent`, and the report lists their methods.
- [x] **W6. Guest serve path.** Delivered by W1: the host grants `incoming` and `outgoing` for one request through the existing `run` export (`Kio::run_web`), tested in host/tests/web.rs.
- [ ] **W7. `kio run --listen`.** After W4c, H2, H3. `--listen addr` grants `Incoming` and `Outgoing`; `kio run` then serves until stopped, running `main` once per request with per-request limits. `--listen` without an `Incoming` parameter in `main`, or the reverse, is an error with a hint. Defaults to refusing non-loopback addresses unless `--listen 0.0.0.0:..` is written explicitly. tokio + hyper http1 + Wasmtime pooling allocator; decisions 4 and 5. `kio run app.kio --dir public=./public --listen 127.0.0.1:8080`.
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
