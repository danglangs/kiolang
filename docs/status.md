# Status

Update at the end of every task. Detail lives in the task entries; this page points to them.

## Now

**Checkpoint reached (2026-11-06): compiled kio runs, proven against the interpreter.** P3 (IR, lowering, interpreter on the IR), C0 (design, [codegen.md](codegen.md)), C1 (runtime), C2a-c (pipeline, Int/Bool/control/calls, differential harness) are done. `host/tests/differential.rs` runs every corpus program both ways; any disagreement fails CI. A deliberate codegen bug (`<` as `<=`) was confirmed to fail it.

**2026-11-08: new primary agent; work runs in parallel streams** (Opus for bulk code, Fable-level review; see CLAUDE.md "How to work"). Done: L3a (`return`/`break`/`continue` as a match arm), L3b (modules use each other's `pub` items). Also done: C3a (runtime exports, descriptor table, literals, leak check), kio-web `api.kio`. In progress: C6a-b, D1 (Dir methods), K1 (Kv).

## Next

1. **C3b-e: heap values in codegen** (C3a, runtime exports, descriptor table and literals, done), pinned in compiler-todo.md. Each part grows the harness's "ran both ways" count (46 at the checkpoint) and keeps live objects at 0.
2. C4 (capabilities, grants, host calls), C5 (`kio run` and `--listen` compiled by default; the interpreter behind `--interp`; drop wizer, re-measure W13d), C6 (program generator), C7 (adversarial review, including `trap`), C8 (measure, then Perceus-style refcount elision).
3. D1 (`Dir` `list`/`write`/`remove`/`exists`) and K1 (`Kv`), approved 2026-11-08 and in progress, then WS1 (WebSockets, web-support-todo.md). C4 covers their calls generically.
4. `Option::and_then`/`map` (needs function values; not yet pinned). kio-web helpers move to `api.kio` (in progress), which L3b made possible.

## Waiting on the maintainer

- open_questions #34: whether to add constants (proposed: check-time literals only). Not decided; nothing depends on it.

## Speed (2026-11-06, release, run time; details in [roadmap.md](roadmap.md))

| | fib(35) | 30 M loop |
|---|---|---|
| Rust | 24 ms | 20 ms |
| kio compiled (plus ~10 ms compile) | 117 ms | 46 ms |
| Python 3.11 | 890 ms | 1.76 s |
| kio interpreter (`kio run` today) | 3.8 s | 5.9 s |
