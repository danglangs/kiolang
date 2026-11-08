# Status

Update at the end of every task. Detail lives in the task entries; this page points to them.

## Now

**Checkpoint reached (2026-11-06): compiled kio runs, proven against the interpreter.** P3 (IR, lowering, interpreter on the IR), C0 (design, [codegen.md](codegen.md)), C1 (runtime), C2a-c (pipeline, Int/Bool/control/calls, differential harness) are done. `host/tests/differential.rs` runs every corpus program both ways; any disagreement fails CI. A deliberate codegen bug (`<` as `<=`) was confirmed to fail it.

## Next

1. **C3a-e: heap values in codegen**, pinned in compiler-todo.md. Each part grows the harness's "ran both ways" count (46 at the checkpoint) and keeps live objects at 0.
2. C4 (capabilities, grants, host calls), C5 (`kio run` and `--listen` compiled by default; the interpreter behind `--interp`; drop wizer, re-measure W13d), C6 (program generator), C7 (adversarial review, including `trap`), C8 (measure, then Perceus-style refcount elision).
3. After C5, in this order (not before: they change what the compiler must cover). `Dir` gains `list`, `write`, `remove`, `exists` (caps.wit; agreed in principle, pin first). Unblocks the notes app's PUT and DELETE (report in docs/web-support-todo.md, W9a; the app is in the kio-web repo, github.com/danglangs/kio-web).
4. `Option::and_then`/`map`; a bare `return` as a match arm; kio-web helpers (`json_value`, `error`, `body_json`), from the same W9a report.

## Waiting on the maintainer

- **`Kv` capability:** deferred until an app needs shared state across requests (design.md, Where state lives).
- **WebSockets:** deferred; plan is a fresh sandbox per message plus a store, or one per connection when state is in memory.
- open_questions #34: whether to add constants (proposed: check-time literals only). Not decided; nothing depends on it.

## Speed (2026-11-06, release, run time; details in [roadmap.md](roadmap.md))

| | fib(35) | 30 M loop |
|---|---|---|
| Rust | 24 ms | 20 ms |
| kio compiled (plus ~10 ms compile) | 117 ms | 46 ms |
| Python 3.11 | 890 ms | 1.76 s |
| kio interpreter (`kio run` today) | 3.8 s | 5.9 s |
