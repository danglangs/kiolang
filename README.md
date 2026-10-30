# kio-lang

A small capability-typed language for AI agents. Code can only touch the files and network it is explicitly granted.

**Status: design stage. Nothing is implemented yet.**

## Why

Agents write code faster than people can review it. Instead of trusting the generator or a second model reviewing it, kio-lang makes the dangerous things impossible to express unless authority was granted:

- There is no ambient I/O. Files, network and clock exist only as **capability values** handed in by the host.
- A small type checker rejects code that uses a capability it wasn't given, including through helper functions.
- Programs run inside a Wasm sandbox whose only way out is a short list of host functions. If the checker has a bug, the sandbox still holds.

The language stays small. Real-world power comes from the host: trusted, capability-typed functions you can audit in one place.

## Architecture

```
 Agent writes                 Toolchain                         Runtime
┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────────────────┐
│ .kio source: │──▶│ type checker │──▶│ interpreter  │──▶│ Wasmtime sandbox         │
│ records, fns,│   │ accept/reject│   │ (later: a    │   │                          │
│ cap params   │   │ + capability │   │  compiler)   │   │ guest can only call ──┐  │
└──────────────┘   │   tracking   │   └──────────────┘   │ imported host fns     │  │
       ▲           └──────┬───────┘                      └───────────────────────┼──┘
       └──────────────────┘ errors back                                          ▼
                            to agent                     ┌──────────────────────────┐
        ┌──────────────────────────────┐                 │ HOST (Rust)              │
        │ interface file               │                 │ trusted, hand-written:   │
        │ (caps.wit + prelude.kio)     │────implements──▶│ dir.read / dir.create    │
        │ read by checker and runtime  │                 │ http.get (allowlisted)   │
        └──────────────────────────────┘                 │ validates every handle   │
                                                         └──────────────────────────┘
```

| Component | Trusted | Role |
|---|---|---|
| `.kio` source | No | Business logic. Capabilities are opaque values (`Dir`, `Http`) that can be passed and called, never created. |
| Type checker | Yes | Accepts or rejects the program against the language rules and the interface file, and reports which capabilities each function can reach. Generates no code. |
| Interpreter | Yes | Evaluates the checked program. Written in Rust, compiled to `wasm32`, and run inside Wasmtime. |
| Interface file | Yes | Declares which host functions exist and their types. Changing it is a security-review event. |
| Wasmtime | Yes | In-process sandbox. The guest has no ambient file, network, env or clock access, and runs under fuel and memory limits. |
| Host functions | Yes | The only code that touches real resources. Each call validates the handle, its scope and quotas. |

Everything runs in one Rust process. Wasmtime is embedded as a library, and its sandbox is a memory wall inside that process. For production, run the process under OS-level isolation as well (a container or VM, plus seccomp).

## Glossary

- **Ambient authority:** power code has just by running, such as `open()`, sockets or env vars in most languages. kio-lang has none.
- **Capability:** a value that *is* the permission, such as a `Dir` or an `Http`. If you hold it you can use it; if you don't, you can't.
- **Unforgeable:** code can't create or cast a capability. It can only receive one from the host.
- **Handle:** what a capability is at run time, a number the host looks up in its table for that run. An unknown or wrong-kind handle is denied.
- **WIT (WebAssembly Interface Type):** the interface definition language of the Wasm Component Model. `caps.wit` lists every function that crosses the sandbox wall, and bindings for the guest and host are generated from it.
- **Host as kernel:** the guest is like a user process, host functions are its syscalls, and handles are its file descriptors.

## Example

```
// prelude.kio (trusted interface)
cap Dir { fn read(path: Str) -> Result<Bytes, IoErr>
          fn create(path: Str, data: Bytes) -> Result<(), IoErr> }

// agent-written program
fn main(input: Dir, out: Dir) -> Result<(), IoErr> {
  let data = input.read("invoices.csv")?
  out.create("summary.csv", summarize(data))
}
```

`summarize` cannot touch the filesystem, because it was never given a `Dir`. `main` can use only the two directories the host granted.

## v0 guarantees

| # | Guarantee | Enforced by |
|---|---|---|
| G1 | No ambient authority: no I/O except through capability values | Checker + Wasm (only kio imports are linked) |
| G2 | Unforgeable capabilities: obtainable only as `main` parameters granted by the host | Checker + host handle table |
| G3 | Filesystem confinement: a `Dir` reaches only paths beneath its root, with no `..` or symlink escape | Host |
| G4 | Network allowlist: an `Http` reaches only granted hosts and methods | Host |
| G5 | Bounded execution: fuel, memory, host-call count and I/O size limits | Wasmtime + host |

## Non-goals

- **Business correctness.** A well-typed program can still compute the wrong answer.
- **Information-flow control.** Data a program may read can still leave through an output it is allowed to use.
- **Policy authoring.** kio-lang enforces the grants it is given. It doesn't decide what an agent *should* be granted.
- **Making untrusted host code safe.** The host functions are the security boundary and need careful review.

## Plan

Crates:

- `lang`: parser, type checker and interpreter. It has no I/O and no Wasm dependency, so it can be tested natively.
- `guest`: a thin adapter that packages `lang` as a Wasm component. It exports `run(source)` and forwards capability calls to the `caps.wit` imports.
- `host`: the native "kernel". It runs the checker, embeds Wasmtime, mints handles, sets limits and implements the host functions.

v0 has no closures, globals, reflection or `unsafe`.

1. Parser and checker natively, with accept/reject tests (G1, G2).
2. Interpreter natively, with stub host functions.
3. `caps.wit` and real host functions (G3, G4).
4. Guest built for `wasm32`, running in Wasmtime with limits (G5).
5. Adversarial test suite passing: path escapes, disallowed hosts, forged handles, runaway loops.
6. Later: an MCP tool for agents, an authority-diff report for reviewers, and possibly a compiler to Wasm.

## Related work

- [Wyvern](https://wyvernlang.github.io/): capability-based module system for authority control
- [TACIT](https://arxiv.org/abs/2603.00991): tracked capabilities for safer agents, in Scala 3
- [Scala capture checking](https://docs.scala-lang.org/scala3/reference/experimental/cc.html)
- [Austral](https://austral-lang.org/): linear types and capability-based security
- [WASI](https://wasi.dev/) and [Wasmtime](https://wasmtime.dev/)
- [cap-std](https://github.com/bytecodealliance/cap-std): capability-based filesystem APIs for Rust

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
