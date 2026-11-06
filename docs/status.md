# Status

Update at the end of every task. Detail lives in the task entries; this page points to them.

## Now

The compiler ([compiler-todo.md](compiler-todo.md)): P3a IR, P3b lowering and P3c, the interpreter on the IR, done (fib 2.7x faster natively, 3x in Wasm).

## Next

1. C1 runtime (C1a done: objects and refcounts; C1b-d next), then C2-C4 codegen with differential tests against the interpreter. Design: [codegen.md](codegen.md), approved.
2. `Dir` gains `list`, `write`, `remove`, `exists` (caps.wit; agreed in principle, pin first). Unblocks the notes app's PUT and DELETE (web W9a report).
3. `Option::and_then`/`map`; a bare `return` as a match arm; kio-web helpers (`json_value`, `error`, `body_json`), all from the W9a report.

## Waiting on the maintainer

- **`Kv` capability:** deferred until an app needs shared state across requests (design.md, Where state lives).
- **WebSockets:** deferred; plan is a fresh sandbox per message plus a store, or one per connection when state is in memory.
- open_questions #34: constants as check-time literals.

## Baseline (2026-11-06, release)

| | fib(32) | 3 M loop |
|---|---|---|
| kio, in Wasm | 2.6 s | 0.85 s |
| kio interpreter, native | 1.07 s | 0.33 s |
| Python 3.11 | 0.4 s | 0.18 s |
| Rust | 0.013 s | 0.003 s |
