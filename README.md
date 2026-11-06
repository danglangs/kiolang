<p align="center"><img src="docs/assets/kio-unicorn.svg" width="180" alt="kio mascot: a unicorn whose horn is a key"></p>

# kio-lang

A language for code that AI agents write: the code names what it needs (`input: Dir`), the operator decides what that is (`--dir input=/data`), and nothing else is reachable.

Most languages today are not AI-safe: any line can read files, open sockets or start processes. Rust gives strong spatial and temporal memory safety, but any Rust function can still reach the network. Research such as [TACIT](https://arxiv.org/abs/2603.00991) and [Scala capture checking](https://docs.scala-lang.org/scala3/reference/experimental/cc.html) makes capabilities first-class.

kio is a fresh language built around that idea, using Rust's ecosystem for speed and tooling. The goal is a general-purpose systems language: near-native speed, no garbage collector, memory and capability safe.

**Status:** v0 works end to end: checking, the authority report, a Wasm sandbox with enforced limits, files, HTTP and serving. Progress: [docs/guest-host-todo.md](docs/guest-host-todo.md), [docs/web-support-todo.md](docs/web-support-todo.md).

## Quickstart

```
rustup target add wasm32-wasip2
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
error: IoErr::InvalidPath
```

**A socket is a grant too.** A web server is the same model, one fresh sandbox per request:

```
$ kio run examples/serve/app.kio --dir public=examples/tour/in --listen 127.0.0.1:8080
$ curl http://127.0.0.1:8080/hello/kio
{"hello": "kio"}
```

### More

- `--audit log.jsonl` records every host call: capability, method, path, bytes and result. A call the report did not predict stops the run as a checker bug.
- Every run is bounded: `--fuel`, `--timeout`, `--memory`, `--stack`, `--calls`, `--io`. Each limit's error names the flag that raises it.
- Errors teach: `did you mean count?`, `declare it with let mut`.
- Performance: about 24,000 requests/s with keep-alive, 6,200 without, with a fresh sandbox per request ([docs/perf.md](docs/perf.md)).
- The language: [docs/language.md](docs/language.md). A web framework in kio: [kio-web](https://github.com/danglangs/kio-web).
- A guided demo: `docs/hack/demo.sh` walks through the report, a rejected helper, a refused escape and a fuel stop.
- From source: `cargo test` runs every suite; `cargo run -p kio -- run file.kio` runs without installing.

## Use from an agent

`kio mcp` is an MCP server on stdio with two tools: `kio_primer` (the language guide) and `kio_check` (check source in the sandbox; errors come back as `{file, line, message, hint}`). There is no run tool: grants are the operator's decision. Add it to an MCP client config:

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

A reviewer reads the signature of `main` and knows everything the program can ever touch. Because there is no ambient clock or randomness, a run is also deterministic given its inputs, so a replay reproduces a bug exactly.

## Why a new language

Why not add a checker to Python or Rust, or run agent code in a container? kio is a small language where authority is explicit, the analysis is simple enough to understand, and execution is confined and audited by construction.

| Approach | Where it falls short |
|---|---|
| Static analysis on an existing language | Ambient authority is built into the standard library, and `eval`, reflection, FFI, `unsafe`, macros, build scripts and dependencies all reach around the analysis. Making the analysis sound means banning most of the language, which is a new language without the enforcement. Nothing makes the code that runs be the code that was checked. |
| Containers, seccomp, VMs | They confine the whole process, all or nothing. They cannot say which function may send data where, and nothing tells a reviewer what the code can do before it runs. |
| kio | Authority is in every signature, so the checker's rule is one sentence and its report is exact. The checker runs inside the sandbox on the code it executes, the host enforces every grant again, and the host logs every call. |

The cost is the ecosystem: kio has few libraries and no package manager yet. Two things soften it. Agents, not people, write most of the code, and Rust syntax means models already write it well. Heavy lifting such as HTTP, TLS and filesystems lives in the host, in Rust, behind capabilities.

## Threat model

- **Adversary:** the agent writing the program. It may be buggy, prompt-injected or malicious.
- **Defended:** the host machine, and every file and network host not granted to this run.
- **Not defended:** misuse of what was granted. A program allowed to read `customers/` and POST to `partner.example` can send the customers to the partner. kio does not track where data came from.

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

Three crates:

- `lang`: parser, checker and interpreter. No I/O, no Wasm dependency, tested natively.
- `guest`: `lang` plus a thin adapter, built once to `wasm32`. Exports `check(files)` and `run(files, grants)`; see [caps.wit](caps.wit).
- `host`: the native binary. Embeds Wasmtime and the guest, mints handles, sets limits, implements the host functions.

Steps:

1. Done: parser, checker and authority report natively, with accept/reject tests (G1, G2).
2. Done: interpreter natively, with stub host functions.
3. Done: capability table generated from `caps.wit`, real `Dir` and `Http` host functions (G3, G4).
4. Done: guest built for `wasm32`, running in Wasmtime with limits (G5).
5. Done: adversarial reviews of the capability rules and the host (path escapes, forged handles, runaway loops, blocked calls). `Http` refuses other hosts, off-origin redirects and private addresses.
6. Done: an audit log of every host call, and `kio mcp`, which serves the primer and `check` to agents.
7. Done: `kio run --listen`, one fresh sandbox per request (about 0.2 ms each), with slow-client limits, and [kio-web](https://github.com/danglangs/kio-web), a web framework written in kio ([docs/web-support-todo.md](docs/web-support-todo.md)).
8. Later: copy-on-write values and a lowering step, then attenuation and a compiler to Wasm.

v0 has no closures, globals, reflection or `unsafe`. Capabilities are parameters only. See [docs/language.md](docs/language.md).

## Docs

- [docs/language.md](docs/language.md): one-screen primer and the rules the checker enforces.
- [docs/design.md](docs/design.md): components, trust boundary, one run step by step, open questions.
- [docs/lang-todo.md](docs/lang-todo.md): decisions and milestones for the native language crate (done for v0).
- [docs/guest-host-todo.md](docs/guest-host-todo.md): tasks for the Wasm sandbox and the `kio` binary.
- [docs/web-support-todo.md](docs/web-support-todo.md): kio-web, a web framework in kio, and the language features it drives.
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
