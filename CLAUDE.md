# Working on kio

kio is a capability-typed language for code AI agents write: the code names what it needs (`input: Dir`), the operator grants it (`--dir input=/data`), nothing else is reachable. Goal: a general-purpose systems language, near-native, memory and capability safe.

**Start here:** [docs/status.md](docs/status.md) (where work stands, what is next, what waits on the maintainer).

## Where things are

| Question | File |
|---|---|
| The language | docs/language.md |
| Every flag and grant, by example | docs/guide.md |
| Architecture, trust boundary, state, memory model | docs/design.md |
| The sandbox interface (the one hand-written contract) | caps.wit |
| Tasks: language, host, web, compiler | docs/{lang,guest-host,web-support,compiler}-todo.md |
| Decisions taken without review, and how to undo each | docs/open_questions.md |
| Speed, gaps, direction | docs/roadmap.md, docs/perf.md |

```
lang/    parser, checker, lowering to IR, interpreter (the test oracle). No dependencies.
host/    the `kio` binary: check, compile, run in Wasmtime; grants, limits, audit log, --listen, mcp
runtime/ compiled kio's runtime library (no_std), built to runtime.wasm
codegen/ IR to Wasm, appended to runtime.wasm, wrapped as a `compiled` component
tools/capgen  generates lang/src/caps_gen.rs from caps.wit
```

## Principles

- **Every resource is a grant.** A directory, host, socket or store is a capability: a `main` parameter, granted on the command line, enforced and logged by the host. For a new feature, first ask "is this a grant?"; prefer a capability and a `kio run` flag over a new command, mode or construct.
- **The host is the security boundary.** It enforces every rule itself and never trusts compiled code or the checker.
- **Errors teach.** Every diagnostic says what is wrong, at the exact span, with a hint naming the fix, as rustc does. Tests assert hints.
- **The interpreter is the reference** for the compiler and its test oracle (native only, never shipped from C5): compiled code must match it. A language feature lands in the interpreter first, and its compiler task is pinned before the language task counts as done.

## How to work

1. **Pin before building.** A task in a `*-todo.md` states signatures, rules, error cases and "Done:" tests before any code. Split anything that spans two passes or grows large.
2. **Delegate scoped work.** Well-pinned, mechanical tasks go to a cheaper model; shape-setting or subtle work (IR, codegen, security reviews) to a stronger one. Workers report ambiguities; they do not settle design.
3. **Review every diff** before it is committed or pushed. Iterate on code later tasks inherit. Security-relevant changes get an independent adversarial review.
4. **Decide two-way doors yourself.** Pick one, continue, and log it in docs/open_questions.md: question, choice, how to undo.
5. **Ask the maintainer only about one-way doors:** changes to caps.wit, language semantics programs would rely on, a new dependency in `lang`, anything published. Ask with a small diagram or example and a recommendation.
6. **Commit per task** with the task id in the message. Record a one-line result (with numbers, for performance work) in the task entry, and update docs/status.md.
7. **Docs before every push:** re-read README and docs for anything the change made stale.

## Style

- **Code:** minimal and easy to review line by line. Straightforward control flow; only the abstractions the task needs. Comments only where they add information.
- **Docs:** shorter still. Concrete decisions and acceptance criteria; no filler, no restating other docs.
- **Talking to the maintainer:** short. Tables, small ASCII diagrams and examples over prose. A decision request gives the options, the tradeoff and a recommendation.

## Rules

- Commits use the repo's configured git identity; never change `user.email`. No `Co-Authored-By` or AI-attribution lines in commits or PRs.
- Stage named files; never `git commit -a` or `git add .` while other work is in the tree.
- Never hand-edit `lang/src/caps_gen.rs`: `cargo run -p capgen` (CI checks it).
- Parallel agents use separate `CARGO_TARGET_DIR`s, and never build a crate another agent is editing.

## Build and test

```
rustup target add wasm32-unknown-unknown
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo clippy -p runtime --target wasm32-unknown-unknown --features test-heap -- -D warnings
cargo build --release -p kio            # target/release/kio
KIO_TEST_HEAP=1 cargo test ...          # compiled code on the test-heap runtime (poisoned frees)
```

kio-web (github.com/danglangs/kio-web) has its own tests: `KIO=path/to/kio ./test.sh`.

Gotchas: on macOS `sed -i ''` and there is no `timeout`; after a `sed` edit run `cargo fmt`; a shell `EXIT` trap under `set -e` needs `wait $pid || true`.
