# guest and host TODO

Run kio inside the Wasm sandbox. The interface is fixed in [caps.wit](../caps.wit); limits and the run walkthrough are in [design.md](design.md). Tasks run in order. Each is pinned before handoff, like [lang-todo.md](lang-todo.md).

## Rules for every task

- The rules in lang-todo.md apply: minimal code, errors that teach, tests assert hints, no unpinned design.
- Toolchain: rustup stable, target `wasm32-wasip2`. Versions are pinned in `Cargo.toml`: `wit-bindgen` 0.63, `wasmtime` 50.
- `lang` stays dependency-free and unchanged in behavior. The guest adapts it; it does not fork it.
- The host is the security boundary. Every host function validates its handle, kind and quotas itself and never trusts the guest or the checker.
- CI builds the guest and runs the host tests.

## Layout

```
guest/   lang plus the WIT adapter, built to wasm32-wasip2 as a component
host/    the kio binary: Wasmtime, cap-std, limits, host functions
```

The host embeds the guest component bytes at build time.

## Tasks

- [ ] **G1. Guest component with `check`.**
  - `guest` crate, `crate-type = ["cdylib"]`, `wit_bindgen::generate!` on `caps.wit` world `guest`. Implements `check` by calling `lang::check(src, &CapTable::v0())` and `lang::report`, mapping each `Diag` to `diag { start, end, message, hint }` and the report to `report { lines }`. `run` returns a single diag "not implemented" until G2.
  - Build: `cargo build -p guest --target wasm32-wasip2 --release` produces a component (wasip2 emits components directly).
  - A test crate or `host/tests` file loads the component with `wasmtime` (component model, `bindgen!` on the same world), stubs the `fs` and `net` imports so that any call fails the test, and calls `check`.
  Done: `check` on examples/summarize.kio returns its two report lines; on a program with an unknown name it returns one diag whose span, message and hint equal `lang::check`'s; no import is called; CI builds the component and runs the test.

- [ ] **G2. `run` through the sandbox.** Pinned after G1.
  Done: a test host with an in-memory `Dir` resource runs examples/invoices.kio through the component and receives the report and `main`'s value; a missing grant returns the no-grant diag and calls no import.

- [ ] **G3. Measure the guest.** Stack and memory at the depth budget and on the primer, in release. Sets the `--stack` default and decides bytecode VM timing.
  Done: numbers recorded in design.md's limits table and open_questions #11.

- [ ] **H1. `kio` with real directories.** `kio check prog.kio` and `kio run prog.kio --dir name=path`, cap-std for every path.
  Done: the README example runs on real files; `..`, absolute paths and outward symlinks are refused.

- [ ] **H2. Limits.** Fuel, memory, stack, wall clock, host calls and I/O, with the defaults and flags in design.md.
  Done: one test per limit, each stopping a program that exceeds it with an error naming the limit.

- [ ] **H3. Adversarial host suite.** Path escapes, symlink races, forged and wrong-kind handles, runaway loops, huge reads, slow host calls.
  Done: every case refused or bounded; an independent review attacks the host like the capability-rule review did.

- [ ] **H4. `Http` with an allowlist.** Grant names host and methods; recheck every redirect, refuse IP literals unless granted, pin DNS per run, cap response size.
  Done: tests for each rule against a local test server.

- [ ] **M1. MCP tool.** Serves `primer` (language.md) and `check` (the guest export), so agents can learn kio and iterate.
  Done: an MCP client gets the primer and a structured diagnostics list for a bad program.
