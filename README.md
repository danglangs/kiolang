# kio-lang

A small capability-typed language for AI agents. A function can only touch the files and network named in its signature.

**Status: design stage. Nothing is implemented yet.**

## Why

Agents write code faster than people can review it. In most languages any line can open a file or a socket, so reviewing for safety means reading every line.

kio-lang makes authority declarative:

- There is no ambient I/O. Files and network exist only as **capability values** the host hands to `main`.
- A function with no capability parameter cannot do I/O. The checker rejects the attempt at compile time, including through helpers.
- The program names roles. The operator binds them to real directories and hosts, per run.
- Programs run inside a Wasm sandbox whose only exits are a short list of host functions. If the checker has a bug, the sandbox still holds.

A reviewer reads the signature of `main` and knows everything the program can ever touch. Because there is no ambient clock or randomness, a run is also deterministic given its inputs, so a replay reproduces a bug exactly.

## Example

The agent writes:

```
fn summarize(data: Bytes) -> Bytes {
  // pure computation: no Dir in the signature, so no I/O is possible
}

fn main(input: Dir, out: Dir) -> Result<(), IoErr> {
  let data = input.read("invoices.csv")?;
  out.create("summary.csv", summarize(data))
}
```

The operator runs it:

```
kio run prog.kio --dir input=/srv/jobs/42/in --dir out=/srv/jobs/42/out
```

The checker reports:

```
main       : input read, out create
summarize  : none
```

Every path in the program is relative to one of the two granted directories. `..`, absolute paths and symlinks out are refused by the host.

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
- `guest`: `lang` plus a thin adapter, built once to `wasm32`. Exports `check(source)` and `run(source, caps)`.
- `host`: the native binary. Embeds Wasmtime and the guest, mints handles, sets limits, implements the host functions.

Steps:

1. Parser, checker and authority report natively, with accept/reject tests (G1, G2).
2. Interpreter natively, with stub host functions.
3. Capability table generated from `caps.wit`, real host functions (G3, G4).
4. Guest built for `wasm32`, running in Wasmtime with limits (G5).
5. Adversarial suite: path escapes, disallowed hosts, redirects, forged handles, runaway loops, slow servers.
6. MCP tool that serves the primer and runs `check`, so agents can learn and iterate.
7. Later: attenuation, a compiler to Wasm.

v0 has no closures, globals, reflection or `unsafe`. Capabilities are parameters only. See [docs/language.md](docs/language.md).

## Docs

- [docs/language.md](docs/language.md): one-screen primer and the rules the checker enforces.
- [docs/design.md](docs/design.md): components, trust boundary, one run step by step, open questions.
- [docs/lang-todo.md](docs/lang-todo.md): decisions and milestones for the native language crate.
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
