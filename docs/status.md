# Status

Update at the end of every task. Detail lives in the task entries; this page points to them.

## Now

**Checkpoint reached (2026-11-06): compiled kio runs, proven against the interpreter.** P3 (IR, lowering, interpreter on the IR), C0 (design, [codegen.md](codegen.md)), C1 (runtime), C2a-c (pipeline, Int/Bool/control/calls, differential harness) are done. `host/tests/differential.rs` runs every corpus program both ways; any disagreement fails CI. A deliberate codegen bug (`<` as `<=`) was confirmed to fail it.

## Next

1. **C3: heap values in codegen** (Str, Bytes, List, Map, structs, enums, `match`, `for` over a list), following codegen.md "Ownership" and "Values". Split before starting; each part grows the harness's "ran both ways" count (46 today) and must keep live objects at 0. Wire the runtime's `rt_live` into the harness here.
2. C4 (capabilities, host calls, display of every type), C5 (`kio run` and `--listen` compiled by default; the interpreter behind `--interp`; drop wizer, re-measure W13d), C6 (program generator), C7 (adversarial review, including `trap`), C8 (measure, then Perceus-style refcount elision).
3. `Dir` gains `list`, `write`, `remove`, `exists` (caps.wit; agreed in principle, pin first). Unblocks the notes app's PUT and DELETE (web W9a report).
4. `Option::and_then`/`map`; a bare `return` as a match arm; kio-web helpers (`json_value`, `error`, `body_json`), from the W9a report.

## Waiting on the maintainer

- **`Kv` capability:** deferred until an app needs shared state across requests (design.md, Where state lives).
- **WebSockets:** deferred; plan is a fresh sandbox per message plus a store, or one per connection when state is in memory.
- open_questions #34: constants as check-time literals.

## Speed (2026-11-06, release, run time; details in [roadmap.md](roadmap.md))

| | fib(35) | 30 M loop |
|---|---|---|
| Rust | 24 ms | 20 ms |
| kio compiled (plus ~10 ms compile) | 117 ms | 46 ms |
| Python 3.11 | 890 ms | 1.76 s |
| kio interpreter (`kio run` today) | 3.8 s | 5.9 s |
