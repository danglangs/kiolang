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

## Baseline (2026-11-06, release)

| | fib(32) | 3 M loop |
|---|---|---|
| kio compiled (C2b; incl. ~10 ms compile) | 0.04 s | 0.016 s |
| kio, in Wasm | 2.6 s | 0.85 s |
| kio interpreter, native | 1.07 s | 0.33 s |
| Python 3.11 | 0.4 s | 0.18 s |
| Rust | 0.013 s | 0.003 s |

The Wasm interpreter row is the original baseline; on the C2b run it took 0.90 s and 0.58 s (`Kio::run`).
