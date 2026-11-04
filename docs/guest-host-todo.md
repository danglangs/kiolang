# guest and host TODO

Run kio inside the Wasm sandbox. The interface is fixed in [caps.wit](../caps.wit); limits and the run walkthrough are in [design.md](design.md). Tasks run in order. Each is pinned before handoff, like [lang-todo.md](lang-todo.md).

## Rules for every task

- The rules in lang-todo.md apply: minimal code, errors that teach, tests assert hints, no unpinned design.
- Toolchain: rustup stable, target `wasm32-wasip2`. Versions pinned in `Cargo.toml`: `wit-bindgen` 0.63, `wasmtime` 50.
- `lang` stays dependency-free and unchanged in behavior. The guest adapts it; it does not fork it.
- The host is the security boundary. Every host function validates its handle, kind and quotas itself and never trusts the guest or the checker.
- CI builds the guest and runs the host tests.

## Layout

```
guest/   lang plus the WIT adapter, built to wasm32-wasip2 as a component
host/    the kio binary: Wasmtime, cap-std, limits, host functions
```

## Tasks

- [x] **G1. Guest component exports `check`.** `guest` crate, `crate-type = ["cdylib"]`, `wit_bindgen::generate!` on `caps.wit` world `guest`. `check` calls `lang::check(src, &CapTable::v0())` and `lang::report`, mapping each `Diag` to `diag { start, end, message, hint }` and the report to `report { lines }`. `run` returns one diag "`run` is not implemented yet". No other code.
  Done: `cargo build -p guest --target wasm32-wasip2 --release` produces the component; CI builds it. Behavior is tested in G2.

- [x] **G2. `kio check` through Wasmtime.** `host` crate, binary `kio`. `host/build.rs` runs `cargo build -p guest --target wasm32-wasip2 --release --target-dir target/guest` (its own target dir, so it does not wait on the outer build's lock) with `rerun-if-changed` on `guest/`, `lang/src/` and `caps.wit`; the host embeds the result with `include_bytes!`. `bindgen!` on world `guest`; `fs` and `net` import implementations return an error and record that they were called. `kio check prog.kio` prints the report, or each diag as `path:line: error: msg` and `  hint: ..`, exit 1.
  Done: tests call the export on examples/summarize.kio (two report lines) and on a program with an unknown name (diag identical to `lang::check`'s); no import is called.

- [x] **G3. Guest `run` with an in-memory host.**
  - Guest: `run(source, dirs, https)` checks, then calls `lang::run` with a `Caps` that holds each grant by name. `has(name, ty)` is true when a grant of that kind has that name. `call` converts `Value` arguments to WIT types (`Str` to `string`, `Bytes` to `list<u8>`), calls the import on that grant's handle, and converts the result back to `Value::Enum("Result", ..)`, with `IoErr` and `HttpErr` cases by declaration index (`Status(u16)` as `Int`).
  - Grant errors before any code runs, each a diag with a hint: a duplicate grant name; a grant that matches no `main` parameter ("grant `x` is not a parameter of `main`"). A missing grant is `lang::run`'s existing no-grant error.
  - Ok result: the report lines and `main`'s value printed with `{:?}`. A `RunErr` becomes one diag with its span and no hint.
  - Host: `dir` resources backed by an in-memory `BTreeMap<String, Vec<u8>>` in the store (H1 replaces this with cap-std). `read` of a missing file is `not-found`; `create` stores the bytes. Every call is counted. `Kio::run(source, dirs)` for tests.
  Done: examples/invoices.kio runs through the component with an in-memory `input` and `out` and `total.txt` holds `42`; a missing, duplicate or extra grant returns its diag and no import is called; a runtime error comes back as a diag with its span.

- [x] **G4. Measure the guest.** Wasm stack and memory at the depth budget and on the primer, release build. Sets the `--stack` default; updates design.md limits and open_questions #11; decides bytecode VM timing.
  Done: numbers recorded. Wasm stack: under 32 KB for the primer, about 900 KB at the depth budget in a release host and over 1 MB in a debug one, so `--stack` is 2 MB. Linear memory: 1.1 MB for a trivial program, 1.3 MB at the depth budget, 1.5 MB for the primer on 1,000 rows.

- [x] **H1. `kio run` with real directories.**
  - `kio run prog.kio --dir name=path ..`: each `--dir` opens `path` once with `cap_std::fs::Dir::open_ambient_dir` (the only ambient call in the host), granted as `name`. A path that does not exist or is not a directory is an error before the guest starts.
  - `dir.read` and `dir.create` go through that `cap_std::fs::Dir`, which refuses `..`, absolute paths and symlinks leading outside it. Empty paths and NUL bytes are `invalid-path` before cap-std is called. Escapes are `invalid-path`, missing files `not-found`, permission errors `denied`.
  - `create` fails with `denied` if the file exists: no overwrites. A missing parent directory is `not-found`.
  - Output: the report, then `main`'s value; diags as in `kio check`, exit 1. A stack-overflow trap prints "stack overflow", exit 1.
  - Replace G3's in-memory backend with cap-std; host tests use temp directories (`tempfile` as a dev-dependency).
  - README: the Example and Try it sections merge into one walkthrough using `kio run --dir` on examples/summarize.kio with a real `invoices.csv`.
  Done: summarize.kio and invoices.kio run on files in a temp dir and write their output; `../x`, `/etc/passwd`, a symlink to outside, an empty path and an existing target for `create` each return the right error and leave the filesystem unchanged; the README commands work as written.

- [x] **H2. Fuel and wall clock.**
  - `Config::consume_fuel(true)` and `epoch_interruption(true)`. Each run sets its fuel (`--fuel`, default 1 billion) and an epoch deadline of `--timeout` seconds (default 10). One background thread per process increments the engine epoch every 100 ms; the deadline is `timeout * 10` ticks and traps when reached.
  - Out of fuel prints "out of fuel (limit 1000000000; raise it with --fuel)"; the deadline prints "timed out after 10 s (raise it with --timeout)". Both exit 1. Flags take positive integers; anything else is a usage error, exit 2.
  - `Kio::new` takes a `Limits { fuel, timeout_secs, stack }` struct with `Default` matching design.md; `with_stack` becomes a field.
  Done: `while true {}` stops with the fuel error; a host function that sleeps past a 1 s timeout (a test-only `Caps` hook, or a test dir on a slow path if simpler) stops with the timeout error; examples still run within the defaults; both flags work from the CLI.

- [x] **H3. Memory and stack.** Wasmtime memory limit and `max_wasm_stack`.
  Done: a program allocating without bound and one recursing past the stack each stop with an error naming the limit.

- [ ] **H4. Host-call and I/O quotas.** Call count, total bytes and per-read size, enforced in the host functions.
  Done: one test per quota.

- [ ] **H5. Adversarial host review.** An independent agent attacks the host: path escapes, symlink races, forged and wrong-kind handles, quota bypasses, panics in host functions.
  Done: every finding fixed or logged in open_questions.

- [ ] **N1. `Http` grant and allowlist.** `--http name=host[:methods]`; refuse any other host and IP literals unless granted.
  Done: tests against a local server for an allowed and a refused host.

- [ ] **N2. Redirects and DNS.** Recheck the allowlist on every redirect; pin DNS per run; cap response size.
  Done: a redirect to an ungranted host is refused; tests for the pin and the cap.

- [ ] **M1. MCP tool.** Serves `primer` (language.md) and `check` (the guest export).
  Done: an MCP client gets the primer and structured diagnostics for a bad program.
