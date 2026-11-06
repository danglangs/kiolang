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

**Cost of fresh instances.** Wasmtime's pooling allocator plus copy-on-write memory cut SpiderMonkey's instantiation from 2 ms to 5 µs ([Wasmtime 1.0](https://bytecodealliance.org/articles/wasmtime-10-performance)). `wasmtime serve` on a hello-world went from about 5k to 35k-43k requests/s with pooling on 4- and 16-core Linux machines, and 236k on a maintainer's; the remaining cost is resetting memory (`memset`, `madvise`) ([wasmtime#8034](https://github.com/bytecodealliance/wasmtime/issues/8034)). The kio guest uses 1.1-1.5 MB of linear memory (G4), so reset is small. Measured today: `Kio::new` takes 52 ms, almost all of it compiling the guest, which `kio run --listen` does once at startup; checking `summarize.kio` in the guest takes 40 µs. Measured after W7a: a full request through the kio-web sketch app, from fresh instance to response, takes 475 µs in release (instantiation, the guest's own check, routing and JSON). W10 measures it end to end over HTTP.

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
       target: func() -> string;                 // "/path?query" as sent, percent-encoding kept; no scheme, host or fragment
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
5. **Checking.** `kio run --listen` runs `check` once at startup, prints the report and refuses to start on a diag. The guest is pre-initialized with the checked `Program` (W13b, [`wasmtime-wizer`](https://docs.wasmtime.dev/api/wasmtime_wizer/index.html)), so instances start with it in copy-on-write memory and requests do not re-check.
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
- [x] **W4b. Modules in the checker and interpreter.** `[parallel]` After W4a. v0 is flat: a module is one file, only the root file may declare `mod`, so there are no nested modules and no cycles. Items in a module are visible outside it only if `pub`; the root's own items need no `pub`. Function, struct and enum names are resolved per module; `web::text` calls `web.kio`'s `text`. `Span` gains `file: u32` (0 is the root). The report names module functions `web::text`.
  Done: a two-file program runs natively; calling a non-`pub` function, `mod` outside the root, an unknown module, and a duplicate `mod` are each rejected with a hint; a diag in `web.kio` carries its file.
- [x] **W4c. Files through the API and host.** After W4b.
  - caps.wit: `record file { name: string, source: string }`; `check` and `run` take `files: list<file>` (root first) in place of `source: string`. `diag` gains `file: u32`. Security-review event.
  - Guest: calls `lang::check_files`.
  - Host: reads the root file, then for each `mod name;` found by a cheap scan, `name.kio` in the same directory, through the host's own filesystem access (this is operator input, like the root file, not a guest capability). A missing module file is a clean error naming it, exit 2. Diags print `web.kio:3: error: ..` using the diag's `file`.
  - `examples/web/app.kio` and `examples/web/web.kio`: a two-file pure example (no capabilities) that `kio run` executes.
  Done: `kio run examples/web/app.kio` runs; a diag inside `web.kio` prints `web.kio:LINE`; a `mod` naming a missing file is a clean error; `kio check` works on both; capgen drift passes.

- [x] **W5. `Incoming` and `Outgoing` capabilities.** Delivered by W1: both are checked like `Dir` (parameters only), a second `send` is `already-sent`, and the report lists their methods.
- [x] **W6. Guest serve path.** Delivered by W1: the host grants `incoming` and `outgoing` for one request through the existing `run` export (`Kio::run_web`), tested in host/tests/web.rs.
- [x] **W7a. One request per instance, without a socket.** Host library only. Build the Engine, `Component` and linker once (`Kio::new` today builds them per run), then `Server::handle(files, grants, request) -> response` makes a fresh `Store` and instance per request with per-request limits (decision 9) and the pooling allocator. Check the program once at `Server::new` and refuse to start on a diag; build the authority report there too, so the always-on report check (H8) does not re-check per request.
  Done: 1,000 sequential requests to the sketch app through `Server::handle` each get a correct response; instantiation per request is measured and recorded (target: under 1 ms); a request that runs out of fuel gets a 500 and the next request still works.
- [x] **W7b. `--listen`.** After W7a. `kio run app.kio --listen addr ..`: tokio + hyper 1 (http1 only) accept loop; each request is read fully (decision 3), handed to `Server::handle` on `spawn_blocking` behind a semaphore (decision 4), and its response written back. `--listen` must name an address; nothing listens without it. `main` taking `Incoming`/`Outgoing` without `--listen`, or `--listen` with a `main` that does not, is an error with a hint. Ctrl-C stops the server.
  Done: an integration test starts `kio run --listen 127.0.0.1:0`, sends three requests with a real client and gets the right bodies; 100 concurrent requests all succeed; the README Try it gains a serve example. Also: the host drops guest-set framing headers (`content-length`, `transfer-encoding`, `connection`, `keep-alive`, `upgrade`), so a handler cannot desync a keep-alive connection.

- [x] **W8. Request limits and slow clients.** After W7b. Host only; all values are constants in `listen.rs` (flags later if needed).
  - hyper http1 builder: `header_read_timeout` 10 s (with `hyper_util::rt::TokioTimer`), `max_headers` 100, `max_buf_size` 64 KB (bounds header bytes).
  - Body read deadline: the whole body must arrive within 10 s of the headers, else 408.
  - Connection cap: at most 1,000 open connections (a semaphore taken at accept, before spawning); past it, the accept loop drops the new connection. Idle keep-alive: a connection with no request for 30 s is closed.
  - Queue cap: at most 100 requests waiting for a worker (decision 4); past it, 503 at once instead of waiting 1 s.
  - Host-call workers: `State::within` (H5) leaves a detached thread per timed-out call. Count live detached workers in a process-wide atomic; at 64, `within` refuses new host calls with `timeout` until some finish, so a hostile program cannot pile up threads.
  Done: one test per limit, each with the smallest values that show it: headers stalled past the timeout get no response and the connection closes; a body sent too slowly gets 408; the 101st queued request gets 503 at once; blocked FIFO reads in many requests stop at the worker cap; a response header with CR LF is 500 (already from W1); fuel and memory per request give 500 and the next request succeeds.

- [ ] **W9. kio-web v0 repo.** Started: [danglangs/kio-web](https://github.com/danglangs/kio-web) has the library, tests and example, and the example serves through `kio run --listen`. `web.kio`: `respond`, `text`, `json`, `html`, `redirect`, `not_found`, `route`, `query` (percent-decoded), `header`, `json_str`, `json_obj`, `file`. Tests are kio programs whose `main` builds a `Request`, calls `handle` and checks the `Response`.
  Done: tests pass under `kio run`; an example app serves a page, a JSON route with a path parameter and files from a granted `Dir`, and its README shows the report.
  - [x] **W9a. Notes app.** kio-web repo, `examples/notes/`: a JSON API over a granted `Dir`, written as a real app would be (handlers split into helpers, not one `main`), with today's capabilities only; no kiolang change. `Dir` has `read` and `create` only (no list, delete or overwrite), so: `POST /notes` with `{"title": .., "body": ..}` creates `notes/<id>.json` with the next id from a counter scheme that needs no overwrite (for example one file per id: probe `read` for 1, 2, 3.. until not-found, or document why another scheme); `GET /notes/:id` returns it; `GET /notes` lists them (probe ids); `PUT` and `DELETE` answer 501 with a body naming the missing `Dir` method. Bad JSON or a missing field is 400 with `json.kio`'s message. `examples/notes/README.md` shows the authority report and curl commands; a test program or script exercises every route (`kio run --listen` on a free port plus curl is fine, as test.sh style).
    Report, the point of the task: (1) every function that takes a capability parameter only to forward it, and the deepest forwarding chain; (2) each place the code was awkward, with a two-line example, grouped as capability forwarding, missing `Dir` methods, missing language features, missing kio-web helpers; (3) a sketch of what each fix would look like in the program (no implementation). Record the report in the task notes here.
    Report (done; `examples/notes/{app.kio,README.md,test.sh}` in kio-web, `test.sh` runs it; 13 curl checks pass, 8 concurrent POSTs got distinct ids 1..8):
    1. Capability forwarding. 9 functions take `notes: Dir` (`main` also `req`, `res`). Pure forwarders, which never touch the Dir themselves: `main`, `handle`, `create` (it parses and validates, then hands `notes` to `store`). The rest use it: `store` (create+read), `next_id`, `exists`, `load`, `show`, `list` (read). Deepest chain: `main -> handle -> create -> store -> next_id -> exists`, 6 deep, `notes` passed 5 times. Report: `main : ... notes create read`; every function on the chain lists `notes create read` or `notes read`, so the report cannot tell `create` (validation) from `exists` (probe) by authority.
    2. Awkward places.
       Capability forwarding: authority leaks into pure logic by position, not need.
         `fn create(r: web::Request, notes: Dir) -> web::Response { ... store(notes, title, body) }`  // validation only needs Dir to forward it
         `fn next_id(notes: Dir, from: Int) -> Int` / `fn exists(notes: Dir, id: Int) -> Bool`  // a one-line probe needs the whole Dir, and a read-only probe is not distinguishable from create-capable code in `handle`
       Missing Dir methods:
         no list: `while exists(notes, id) { id += 1; }`  // O(n) full-file reads per POST and per GET /notes
         no overwrite: no counter file, no PUT; no delete: no DELETE (both answer 501, `{"error":"cannot update a note: Dir has no `overwrite` method"}`)
         `create` Err is one value, so `Err(_) => { id = next_id(notes, id + 1); }` treats Denied/TooLarge like "taken" (bounded by id < 100000)
         `exists` has to read the whole file: no `stat`/`has`
       Missing language features:
         `Option` has only `unwrap_or`: `json::get(j, "title").and_then(json::as_str)` is rejected, so a helper `fn str_field(j, key) -> Option<Str> { json::as_str(json::get(j, key)?) }`
         a match arm cannot be a bare `return`/`break`: `Some(_) => return create(r, notes),` is a parse error; must be `Some(_) => { return create(r, notes); }` and `None => { break; }`
         no early-exit match: every route in `handle` is `match web::route(..) { Some(_) => { return ..; } None => {} }`, 5 times
         no first-class functions, so no route table: `[("POST", "/notes", create)]` is impossible
       Missing kio-web helpers:
         JSON error body: `error(status, msg)` builds `{"error": msg}` by hand with a Map + encode (6 lines)
         JSON object building: `note(id, title, body)` is `let mut o = Map::new(); o.insert(..); ..` x3
         no `web::json_value(status, Json)` (encode + respond), so `web::json(200, json::encode(j))` repeats; web.kio and json.kio do not know each other
         no router: the 5-block `match web::route` chain above; route with a method list or a `web::dispatch` would remove it
         no body-as-json: `json::parse(r.body.to_str())` with `Err(e) => return error(400, e)` per handler
    3. Sketches (no implementation).
       Forwarding: attenuated or derived caps, e.g. `Dir::read_only()` / `Dir::sub("x")`, would let `create` take no Dir (return a validated `Note` struct) and `exists` take a read-only view; alternatively split `handle` into `route(r) -> Action` (pure, no caps) and `run(action, notes)`, which needs only an enum and works today and I would do it in a second pass (cuts `handle` and `create` out of the forwarding set; `main -> run -> store -> next_id -> exists`).
       Dir: `list() -> Result<List<Str>, IoErr>` makes ids `max+1` and GET /notes one list; `write(path, data)` (overwrite, or atomic rename) gives a counter file and PUT; `remove(path)` gives DELETE; `exists(path)` avoids reading; a distinct `IoErr::Exists` from `create` replaces the blanket retry.
       Language: `Option::and_then`/`map` (or `?` in an `Option` fn already works, as `str_field` shows); `return`/`break` as arm expressions (`Some(_) => return create(r, notes),`) or `if let Some(p) = ... { return ..; }`.
       kio-web: `web::json_value(status, j)`, `web::error(status, msg)`, `web::body_json(r) -> Result<Json, Response>`, and `json::obj([("k", v), ..])` once tuples or a pair type exist; a `web::route_any(r, [("POST", "/notes"), ...]) -> Int` index to `match` on would flatten `handle`.
    Also: the test relies on python3 for a free port and `sleep 0.3` polling; the listener starts quickly (1.5 s sleep was ample).

- [x] **W10. Benchmark.** `ab` (ApacheBench, ships with macOS) against `kio run examples/serve/app.kio --listen` in release, on `/hello/kio` (route and JSON) and a small file. Concurrency 1, 12 (cores) and 64; 20,000 requests each. Record requests/s, mean and p99 latency, and the host's peak RSS, with `kio` built `--release`. Add a short `docs/perf.md` with the numbers, the machine, and what they imply for P2, P3 and the bytecode VM.
  Done: docs/perf.md exists with the table and a one-paragraph conclusion; README links it.

- [x] **W11. Adversarial serve review.** 47 attacks: no smuggling or desync, no response splitting, no cross-request leaks, no host panics, no per-request limit bypass. Fixed in W11b.
- [x] **W11b. Fix the review's findings.** Host only.
  1. Connection lifetime: a hard 60 s cap per connection (`ListenLimits::lifetime`), enforced by dropping the connection future, so a client that never reads its responses cannot hold a slot or buffered response forever.
  2. `--audit` with `--listen`: refused at startup, before the log file is created, with hint "per-request audit logging is not supported yet; drop --audit".
  3. `--http` with `--listen`: refused at startup the same way (hint "Http grants under --listen are not supported yet").
  4. `Outgoing.send` accepts status 200..=599 only; 1xx is `invalid`.
  5. `Dir.read` and `Dir.create` refuse anything that is not a regular file: open with `O_NONBLOCK` (cap-std `OpenOptionsExt::custom_flags`), then check `metadata().is_file()`, else `denied`. A FIFO in a granted dir then cannot block a worker, which closes the worse-than-stated part of #19.
  Done: one test per item (a non-reading client's connection is closed at the lifetime cap with a small test value; both flag combinations exit 2 with their hint and leave no file; status 101 is `invalid`; reading a FIFO is `denied` at once); all existing tests pass.

- [x] **W12. JSON decoding in kio-web.** kio-web repo only; no kiolang change. A new module `json.kio` (an app writes `mod json;`; modules are flat, so it does not use `web.kio`).
  - `pub enum Json { Null, Bool(Bool), Num(Int), Str(Str), Arr(List<Json>), Obj(Map<Str, Json>) }`, recursive through `List` and `Map` (L1). If the checker refuses a variant named like a type, rename it (`Text`, `Number`) and log it in the task notes.
  - `pub fn parse(s: Str) -> Result<Json, Str>`: recursive descent over `s.to_bytes()`, with a `struct Parsed { value: Json, pos: Int }` threaded through helpers. Whitespace, `null`, `true`, `false`, integers (an optional `-`, digits; a fraction or exponent is `Err("floats are not supported")`), strings with every JSON escape (`\uXXXX` to UTF-8, surrogate pairs joined, a lone surrogate an error), arrays, objects (a repeated key keeps the last value). Trailing input after the value is an error. Nesting deeper than 64 is an error, so a hostile body cannot reach kio's evaluation depth limit. Error messages name the byte offset: `"expected ',' or '}' at byte 17"`.
  - `pub fn encode(j: Json) -> Str`: compact, keys in `Map` order, strings escaped (`"` `\` and control bytes as `\n`, `\t`, `\u00XX`). `parse(encode(j)) == j` for any `j`.
  - Accessors returning `Option`: `get(j, key)`, `at(j, i)`, `as_str(j)`, `as_int(j)`, `as_bool(j)`, `as_list(j)`.
  - `tests/json_test.kio` in the style of `web_test.kio` (returns the number of the first failing check); `test.sh` runs it too; README gains a JSON section.
  - Measure: parse time of a 10 KB and a 100 KB array of small objects under `kio run`. Values are copied, so passing the input `Bytes` down each call may be quadratic; record the numbers in the task notes, as they inform P2 (copy-on-write).
  Done: tests pass under `kio run`, covering every grammar rule and each error, a round trip, and depth 65 refused; timings recorded.
  Result: parsing is quadratic, since the input `Bytes` is copied on every call: 10 KB in 0.13 s (about 1e9 fuel, the default limit), 100 KB in 1.7 s (about 64e9 fuel). This makes P2 (copy-on-write) the next performance task. The round trip compares encoded text, as `==` does not work on enums.

- [x] **W13. Check once under `--listen` (spike).** Each request re-parses and re-checks the program in its fresh instance (decision 5); natively that check costs about 200 µs for the serve example, against about 1 ms per request. Spike, no committed code: measure the check's share of `Server::handle` in the release host, then try the decision 5 alternative, pre-initializing the guest component with the program already checked (`wasmtime-wizer` or an equivalent for components in Wasmtime 50), and report which works, its per-request saving and its cost (build steps, memory, startup). If snapshots do not work for components, list the next option with an estimate.
  Done: a short report with numbers and a recommendation; the owner decides before code lands.
  Result (release, 3,000 `Server::handle` calls on the serve example, median): 500 µs per request: instantiate 16, guest `run` 476 (of which the check is about 350, measured by checking twice), host 8. A wizened guest (`wasmtime-wizer` 50.0.1, `Wizer::run_component` on a new `init(files)` export that checks and keeps the `Program`, then `precompile_component`) handles the same request in 197 µs (p99 380 vs 900). Costs: one dependency (with `wasm-wave`, `logos`), an `init` export in caps.wit, about 92 ms more at `--listen` startup, and about 180 KB more precompiled image, shared copy-on-write across instances. Each request still gets a fresh instance from the snapshot. Rejected: serializing `Program` (more code in `lang`, likely slower), and a host flag telling the guest to skip its check (the guest would no longer check what it runs). Recommendation: adopt wizer; awaiting the owner's decision.

- [x] **W13b. Check once under `--listen`.** Owner approved the W13 recommendation.
  - `caps.wit`, guest world: `export init: func(files: list<file>) -> result<_, list<diag>>;` with the comment "Check `files` and keep the checked program, for a snapshot taken before any request." `check` and `run` keep their signatures. Regenerate with capgen if it covers the world; the capability table does not change.
  - Guest: `init` checks and stores the `Program` in a `thread_local` (`RefCell<Option<(Vec<File>, Program)>>`). `run` takes the stored program only when its `files` equal the stored ones, else checks as today; taking it means a second `run` on one instance re-checks. One-shot `kio run` never calls `init`, so it is unchanged.
  - Host, `Server::new`: after the host-side check, build the snapshot in memory with `wasmtime-wizer` 50 (`Wizer::run_component`, calling `init` with the program's files) on the guest Wasm, then `precompile_component` and `deserialize` it for this engine. The guest Wasm (not only the precompiled form) must be embedded for this: `build.rs` writes `guest.wasm` beside `guest.cwasm`. Wizer is async: use a current-thread tokio runtime for this one step. A failure here is a startup error, never a fallback.
  - The snapshot lives only in host memory: never written to disk or cached across runs. A cached precompiled snapshot is native code that `deserialize` trusts, so it would need an operator-only directory and a keyed integrity check; out of scope.
  - `program_hash` becomes SHA-256 (`sha2` crate, host only; `lang` stays dependency-free) over each file's name and source, length-prefixed so file boundaries count; printed as 64 hex digits in the audit header, and `--listen` prints `program sha256:<hex>` before `listening on`. Update open_questions #27.
  - Done: host tests pass; a test shows a `Server` request does not re-check (for example, a test-hooks counter in the guest, or timing in a release test, whichever is simpler and reliable); `kio run` one-shot unchanged; the audit header hash is 64 hex digits and equal across two runs of the same files and different when a module changes; `Server::handle` median on the serve example re-measured and recorded in docs/perf.md, with an `ab` rerun at concurrency 12.
  Result: `Server::handle` median 175 µs (was 500), startup about 150 ms; `ab -c 12` 6,210 req/s (was 5,960). New dependencies: `wasmtime-wizer` (with `wasm-wave`, `logos`) and `sha2`.

- [x] **W13c. Adversarial review of W13b.** An independent agent attacks the pre-initialized guest: the hand-written WAVE escaping in `checked_guest` (any source bytes: quotes, backslashes, `\u{..}`, control and non-ASCII characters, `}`/`]`/`,` in names, huge files) must either start the server with exactly those files checked or refuse to start; state left in the snapshot leaking into requests (the stored program, `thread_local`s, allocator state, anything a request could observe or change for the next one); `init` callable by a request; the files-equality fallback; fuel and limits during `init`.
  Done: every finding fixed with a test or logged in open_questions.
  Result: WAVE escaping held for every tricky input (tests in `host/tests/snapshot.rs`); one bug fixed (a guest failure during `init` panicked the host inside Tokio, now a startup error); size limits logged as open question 33.

## Later, driven by kio-web

- **Function values.** Route tables and middleware as wrappers (`with_auth(handler)`). Today middleware is a call at the top of `handle`; timeouts, body limits and logging are host concerns.
- **Typed JSON decoding** (into a struct) needs generics or derive; W12 decodes into a `Json` tree.
- **`Result<Response, Response>` from `handle`,** so `?` works in handlers.
- **Nested patterns** (`Err(IoErr::NotFound)`); today a nested `match`.
- **Friction from W12 (json.kio).** `==` on enums and structs; `is_some`/`is_none`, and `unwrap_or` on `Result`; string escapes `\r`, `\0`, `\u{..}`; `loop` (today `while true` needs an unreachable tail expression); constants (today a function returning the value).
- **Outbound calls.** The `Http` capability, allowlisted per route; moves execution to Wasmtime async (decision 4).
- **Streaming bodies, HTTP/2, `wasi:http` export.** Decision 2's alternative.
- **Persistent connections and WebSockets (owner to decide).** HTTP keep-alive already works: hyper keeps the connection and each request gets a fresh sandbox; W8 adds the idle timeout. WebSockets are a long-lived two-way stream, which conflicts with bounded runs and synchronous handlers. Options:
  1. **A fresh sandbox per message (recommended).** The host owns the socket; each message runs `main(msg: WsMessage, conn: WsSend, ..)`, bounded, isolated and logged like a request. State between messages goes through a capability such as `Kv`. A connection is just another grant. Needs `WsMessage`/`WsSend` in caps.wit, the upgrade and a connection table in the host, and `Kv`. Cloudflare Durable Objects and Spin work this way.
  2. **One sandbox per connection.** `main(ws: WebSocket)` loops on a blocking `ws.recv()`; fuel and timeout apply per message, with an idle cap. Simpler programs, but a hung connection holds an instance.
  3. **Async in the language.** `await` in kio: a large change and a one-way door, not needed for 1 or 2.
- **Databases.** A `Kv` or `Sql` capability in caps.wit.
- **Cookies, forms, multipart, websockets.**
