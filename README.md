<p align="center"><img src="docs/assets/kio-unicorn.svg" width="180" alt="kio mascot: a unicorn whose horn is a key"></p>

# kio-lang

A language for code that AI agents write: the code names what it needs (`input: Dir`), the operator decides what that is (`--dir input=/data`), and nothing else is reachable.

Most languages today are not AI-safe: any line can read files, open sockets or start processes. Rust gives strong spatial and temporal memory safety, but any Rust function can still reach the network. Research such as [TACIT](https://arxiv.org/abs/2603.00991) and [Scala capture checking](https://docs.scala-lang.org/scala3/reference/experimental/cc.html) makes capabilities first-class.

kio is a fresh language built around that idea, using Rust's ecosystem for speed and tooling. The goal is a general-purpose systems language: near-native speed, no garbage collector, memory and capability safe.

**Status:** v0, working end to end.

| | |
|---|---|
| Language | structs, enums, generics, closures, `match` with nested and `\|` patterns, `let`-else, constants, `Float`, `HashMap`, built-in JSON, SHA-256 |
| Resources, each a grant | files (`Dir`), a key-value store (`Kv`), HTTP GET (`Http`), `Rng`, `Clock`, `Log`; serving HTTP and WebSockets |
| Safety | the authority report before a run; a Wasm sandbox with fuel, memory, time, call and I/O limits; an audit log |
| Speed | compiled to native code through Wasm: 1.1-2.3x Rust on compute, 2.3-8x where it allocates; faster than Python on most benchmarks ([table](docs/roadmap.md#speed-today)) |

Next: HTTP beyond GET, SQLite, parallel HTTP calls ([status](docs/status.md)).

## Quickstart

```
rustup target add wasm32-unknown-unknown
cargo install --git https://github.com/danglangs/kiolang kio
git clone https://github.com/danglangs/kiolang && cd kiolang      # for the examples
```

```
$ kio run examples/tour/hello.kio
hello, kio
```

### What kio adds

**The report shows what each function can reach.** This `summarize` makes a network request. In Python or Rust that's one line hidden in the body; in kio it needs `log: Http` in the signature, so `kio check` lists it:

```rust
fn summarize(data: Bytes, log: Http) -> Bytes {
    let ignored = log.get("/collect");
    data
}
```

```
$ kio check examples/tour/leaky.kio
main      : input read, out create, log get
summarize : log get
```

**No ambient authority.** In Python or Rust a helper can import `urllib` or `std::net` and reach the network without its signature saying so. kio has no such import; the only way in is a parameter:

```
$ kio check examples/tour/sneaky.kio
examples/tour/sneaky.kio:5: error: no ambient Http access: `Http::get` does not exist
  hint: capabilities come only as parameters of `main`
```

**Capabilities are granted per run and confined by the host.** The operator decides what `input` is; paths outside it, `..` and symlinks out are refused even though the program type-checks:

```
$ kio run examples/tour/escape.kio --dir input=examples/tour/in
main : input read
error: IoErr::InvalidPath
```

**A socket is a grant too.** A web server is the same model, one fresh sandbox per request:

```
$ kio run examples/serve/app.kio --dir public=examples/tour/in --listen 127.0.0.1:8080
main             : req body header_names header_values method target, res send, public read
...
listening on http://127.0.0.1:8080
$ curl http://127.0.0.1:8080/hello/kio
{"hello": "kio"}
```

A WebSocket server takes `msg: WsMessage, conn: WsSend` instead, and gets one fresh sandbox per event (open, each message, close), run in order per connection; state between events goes in a `Kv`:

```
$ kio run examples/ws/echo.kio --listen 127.0.0.1:8080
main : msg data kind, conn binary text
program sha256:…
listening on ws://127.0.0.1:8080
$ websocat ws://127.0.0.1:8080/
hi
hi
```

`msg.target()` and `msg.header("authorization")` show the upgrade request's path and headers, the same on every event of the connection, so a server can route by path or check a token. A browser page may connect only from the server's own host; `--ws-origin https://app.example` allows another origin (clients that send no `Origin`, such as websocat, are not browsers and are accepted).

On a loopback `--listen`, a request's `Host` must be `localhost`, `127.0.0.1` or `[::1]` (against DNS rebinding); `--host app.example` allows another name, and is the only restriction on a non-loopback bind.

### More

- `--http api=api.example` grants `Http` to one host over HTTPS, GET only; `--http-methods api=POST,PUT,DELETE` adds methods for `api.send(method, path, headers, body)`, and `api.send_all([..])` issues several requests at once. `--http api=http://127.0.0.1:9000` allows plain HTTP for local testing. The audit log records each request's method, path, status and header names, never a header value.
- `--kv store=data.kv` grants a key-value store (`store: Kv`) that outlives runs and is shared by every `--listen` request; `--kv store=:memory:` keeps it for the process only. `swap` updates a key atomically, `add` counts in one, `scan` pages through a prefix's entries. `--kv store=data.kv:ro` is read-only: a program that can call `set`, `delete`, `swap` or `add` on it is refused, no lock is taken, and each `get`, `keys` and `scan` sees what another kio writing the file stored (a server reading tokens a separate job rotates). [examples/ledger.kio](examples/ledger.kio) keeps running totals per customer:

  ```
  $ kio run examples/ledger.kio --dir input=examples/in --kv totals=/tmp/ledger.kv
  load  : totals scan
  main  : input read, totals add scan
  parse : none
  Ok(["initech 300", "acme 120", "globex 45"])
  $ kio run examples/ledger.kio --dir input=examples/in --kv totals=/tmp/ledger.kv:ro
  kio: `totals` is granted read-only but `main` can call `totals add`
    hint: drop `:ro` or remove the writes
  ```
- `--rng r` grants randomness (`r: Rng`: `bytes(n)`, `int(lo, hi)`) from the OS; `--rng r=seed:1` a stream that is the same every run (under `--listen`, keyed by the request number too), for replay and tests: anyone who knows the seed can predict every value, so never serve clients with one. `--clock now` grants the system clock (`now.now_ms()`); `--clock now=fixed:2026-11-09T12:00:00Z` (or `fixed:<ms>`) always that time. `--log events=app.log` grants a log the program appends one line to per `write` (control characters escaped, 64 KB a line, 1 MB a run, `req N: ` first under `--listen`); `--log events=-` is stderr. None of the three is reachable without its grant, so a run with fixed values is replayable.
- `--audit log.jsonl` records every host call: capability, method, path, bytes and result. A call the report did not predict stops the run as a checker bug. Under `--listen` it is one log for the server: the header once, then each request's calls and trailer, every line starting with `"req":N` (and `"conn":N` for a WebSocket event); a trailer counts the calls made after the response was sent (`"after_send":N`). The log is appended to, never truncated, and refused at startup if it is a `--kv` store's file, inside a granted `--dir` or inside the compile cache.
- Every run is bounded: `--fuel`, `--timeout`, `--memory`, `--stack`, `--calls`, `--io`. Each limit's error names the flag that raises it.
- Errors teach: `did you mean count?`, `declare it with let mut`.
- Performance: about 74,000 requests/s with keep-alive and 18,000-20,000 without, with a fresh sandbox per request ([docs/roadmap.md](docs/roadmap.md), [docs/perf.md](docs/perf.md)). `kio run` keeps compiled programs in `~/.cache/kio`, so a second run of the same program skips the compile; a run granted a `--dir` that contains, or is inside, the cache does not use it, and a `--kv` store there is refused.
- Built in: `Json` (`s.parse_json()` gives `Result<Json, Str>`, `j.to_json()`, accessors `get`, `at`, `as_str`, ..), `HashMap<K, V>` (insertion-ordered, beside the key-ordered `Map`), generic functions and types with `T: Eq` and `T: Ord` bounds, `<` on `Str` by bytes, `sort_by`, `join`, `sha256`, `to_hex` and `ct_eq` (constant-time compare).
- The language: [docs/language.md](docs/language.md). A web framework in kio: [kio-web](https://github.com/danglangs/kio-web).
- A guided demo: `examples/tour/demo.sh` walks through the report, a rejected helper, a refused escape and a fuel stop.
- From source: `cargo test` runs every suite; `cargo run -p kio -- run file.kio` runs without installing.

## Use from an agent

`kio mcp` is an MCP server on stdio with two tools: `kio_primer` (the language guide) and `kio_check` (check up to 1 MB of source; errors come back as `{file, line, message, hint}`). There is no run tool: grants are the operator's decision. Add it to an MCP client config:

```json
{ "mcpServers": { "kio": { "command": "kio", "args": ["mcp"] } } }
```

## Why

Agents write code faster than people can review it. In most languages any line can open a file or a socket, so reviewing for safety means reading every line.

In kio every file handle, socket and network host is just another capability: named in `main`'s signature, granted by the operator, enforced by the host. A web server is `kio run app.kio --listen 127.0.0.1:8080`, because a listening socket is a grant like `--dir`.

kio-lang makes authority declarative:

- There is no ambient I/O. Files and network exist only as **capability values** the host hands to `main`.
- A function with no capability parameter cannot do I/O. The checker rejects the attempt at compile time, including through helpers.
- The program names roles. The operator binds them to real directories and hosts, per run.
- Programs run inside a Wasm sandbox whose only exits are a short list of host functions. If the checker has a bug, the sandbox still holds.

A reviewer reads the signature of `main` and knows everything the program can ever touch. Because there is no ambient clock or randomness (a program reaches time or random values only through a `Clock` or `Rng` parameter, which the operator can fix), a run is also deterministic given its inputs, so a replay reproduces a bug exactly.

## Why a new language

Why not add a checker to Python or Rust, or run agent code in a container? kio is a small language where authority is explicit, the analysis is simple enough to understand, and execution is confined and audited by construction.

| Approach | Where it falls short |
|---|---|
| Static analysis on an existing language | Ambient authority is built into the standard library, and `eval`, reflection, FFI, `unsafe`, macros, build scripts and dependencies all reach around the analysis. Making the analysis sound means banning most of the language, which is a new language without the enforcement. Nothing makes the code that runs be the code that was checked. |
| Containers, seccomp, VMs | They confine the whole process, all or nothing. They cannot say which function may send data where, and nothing tells a reviewer what the code can do before it runs. |
| kio | Authority is in every signature, so the checker's rule is one sentence and its report is exact. The checker runs in the host on the code it compiles, the sandbox runs only that code, the host enforces every grant again, and the host logs every call. |

The cost is the ecosystem: kio has few libraries and no package manager yet. Two things soften it. Agents, not people, write most of the code, and Rust syntax means models already write it well. Heavy lifting such as HTTP, TLS and filesystems lives in the host, in Rust, behind capabilities.

## Threat model

- **Adversary:** the agent writing the program. It may be buggy, prompt-injected or malicious.
- **Defended:** the host machine, and every file and network host not granted to this run.
- **Not defended:** misuse of what was granted. A program allowed to read `customers/` and call `partner.example` can send the customers to the partner. kio does not track where data came from.

## v0 guarantees

| # | Guarantee | Enforced by |
|---|---|---|
| G1 | No ambient authority: no I/O except through capability values | Checker, and Wasm links only the kio imports |
| G2 | Unforgeable capabilities: obtainable only as `main` parameters granted by the host | Checker, and the host handle table |
| G3 | Filesystem confinement: a `Dir` reaches only paths beneath its root | Host, via cap-std (`openat2` with `RESOLVE_BENEATH` on Linux) |
| G4 | Network allowlist: an `Http` reaches only granted hosts and methods | Host. Rechecked on every redirect, IP literals refused, DNS pinned per run |
| G5 | Bounded execution: fuel, memory, host-call count, I/O size and wall-clock | Wasmtime fuel and epochs, plus per-call timeouts in the host |

The checker is for early, readable rejection and the authority report. The host enforces G1 to G5 whether or not the checker was right.

## Non-goals

- **Business correctness.** A well-typed program can still compute the wrong answer.
- **Information-flow control.** Data a program may read can leave through any output it may use.
- **Policy authoring.** kio enforces the grants it is given. It does not decide what an agent should be granted.
- **Making untrusted host code safe.** The host functions are the security boundary and need careful review.

## Plan

```
source ─► check + report ─► lower ─► IR ─┬─► codegen ─► Wasm + runtime ─► Cranelift ─► native code (`kio run`)
                                         └─► interpreter (native, tests only: the reference)
```

- `lang`: parser, checker, lowering, interpreter. No I/O, no dependencies, tested natively.
- `runtime` and `codegen`: the compiler's runtime library and code generator ([docs/codegen.md](docs/codegen.md)); the sandbox interface is [caps.wit](caps.wit).
- `host`: the `kio` binary. Wasmtime, grants, limits, host functions.

Steps:

1. Done: parser, checker and authority report natively, with accept/reject tests (G1, G2).
2. Done: interpreter natively, with stub host functions.
3. Done: capability table generated from `caps.wit`, real `Dir` and `Http` host functions (G3, G4).
4. Done: programs running in Wasmtime with limits (G5).
5. Done: adversarial reviews of the capability rules and the host (path escapes, forged handles, runaway loops, blocked calls). `Http` refuses other hosts, off-origin redirects and private addresses.
6. Done: an audit log of every host call, and `kio mcp`, which serves the primer and `check` to agents.
7. Done: `kio run --listen`, one fresh sandbox per request (about 35 µs each, compiled), with slow-client limits, and [kio-web](https://github.com/danglangs/kio-web), a web framework written in kio ([docs/web-support-todo.md](docs/web-support-todo.md)).
8. Done: copy-on-write values (passing a value is cheap; semantics unchanged) and the program checked once per server, not per request.
9. Done: lowering to a typed IR (the interpreter runs it, 3x faster), and a compiler to Wasm for `Int`, `Bool`, control flow and calls, with a differential harness against the interpreter ([docs/compiler-todo.md](docs/compiler-todo.md)).
10. Done: the compiler for strings, collections, structs and capabilities; `kio run` and `--listen` compile (C5), and the interpreter is the test oracle only.
11. Done: `Dir` list/write/remove/exists, `Kv`, WebSockets, grants bound by type and DNS-rebinding defence (D1, K1, WS1, H10, H11); `Float`, slices, function values, closures, generic functions, structs and enums with `Eq`/`Ord` bounds, `<` on `Str`, all compiled (L6-L8, C9-C11); an adversarial review of the runtime and codegen (C7); built-in `Json` and `HashMap`, one source for the interpreter and the runtime (L9, L10, C12, C13).
12. Done: a differential harness and program generator (any compiled/interpreted disagreement fails CI), and speed work on strings, refcounts, allocation and borrowed parameters (C6, C8a-C8b6).
13. Done: what the links target app (kio-web) needed: built-ins (`sort_by`, `slice`, `join`, `sha256`, `ct_eq`, ..), errors that teach for Rust forms kio lacks, read-only `Kv` grants, `--audit` under `--listen`, `Kv` `scan` and `add`, kio-web's bearer, wildcard, HEAD, paging and protected routes; `|` patterns, `let`-else, struct update, constants, `Bool.to_str`; `Rng`, `Clock` and `Log` grants (L11-L17, H12-H15, K2, W14).
14. Next: more speed (C8); later attenuation.

There are no globals, reflection, `unsafe`, traits or tuples. Capabilities are parameters only. See [docs/language.md](docs/language.md).

## Docs

- [CLAUDE.md](CLAUDE.md): how to work on kio, for people and agents; start with [docs/status.md](docs/status.md).
- [docs/language.md](docs/language.md): one-screen primer and the rules the checker enforces.
- [docs/design.md](docs/design.md): components, trust boundary, one run step by step, open questions.
- [docs/codegen.md](docs/codegen.md): how a checked program becomes Wasm, and the runtime it links.
- [docs/perf.md](docs/perf.md): server latency and throughput, startup and memory.
- [docs/lang-todo.md](docs/lang-todo.md): tasks for the `lang` crate: parser, checker, lowering, interpreter, and every language feature since.
- [docs/guest-host-todo.md](docs/guest-host-todo.md): tasks for the Wasm sandbox and the `kio` binary.
- [docs/web-support-todo.md](docs/web-support-todo.md): kio-web, a web framework in kio, and the language features it drives.
- [docs/compiler-todo.md](docs/compiler-todo.md): compiling kio to Wasm, task by task, with the interpreter as the reference.
- [docs/roadmap.md](docs/roadmap.md): speed today against native, the path to near-native, and what real apps need that kio lacks.
- [bench/](bench/README.md): the benchmarks behind those numbers, against Rust and Python; `bench/run.sh` reruns them.
- [docs/open_questions.md](docs/open_questions.md): decisions taken without full review, and how to undo each.
- [caps.wit](caps.wit): the interface. The only hand-written source of truth for what crosses the sandbox wall.

## Related work

- [Wyvern](https://wyvernlang.github.io/): capability-based module system for authority control
- [TACIT](https://arxiv.org/abs/2603.00991): tracked capabilities for safer agents, in Scala 3
- [Scala capture checking](https://docs.scala-lang.org/scala3/reference/experimental/cc.html)
- [Austral](https://austral-lang.org/): linear types and capability-based security
- [Second-class values](https://dl.acm.org/doi/10.1145/2983990.2984009) (Osvald et al., OOPSLA 2016): the restriction v0 uses for capabilities
- [WASI](https://wasi.dev/) and [Wasmtime](https://wasmtime.dev/)
- [cap-std](https://github.com/bytecodealliance/cap-std): capability-based filesystem APIs for Rust

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
