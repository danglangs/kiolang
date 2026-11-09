# Status

Update at the end of every task. Detail lives in the task entries; this page points to them.

## Now

**Checkpoint reached (2026-11-06): compiled kio runs, proven against the interpreter.** P3 (IR, lowering, interpreter on the IR), C0 (design, [codegen.md](codegen.md)), C1 (runtime), C2a-c (pipeline, Int/Bool/control/calls, differential harness) are done. `host/tests/differential.rs` runs every corpus program both ways; any disagreement fails CI. A deliberate codegen bug (`<` as `<=`) was confirmed to fail it.

**2026-11-08: new primary agent; work runs in parallel streams** (Opus for bulk code, a stronger model for design and security review; CLAUDE.md "How to work").

| Done | Result |
|---|---|
| L3a, L3b, L4a, L4b | match arms `return`/`break`/`continue`; modules use each other; nested patterns and `if let` (Maranget exhaustiveness); Str and List built-ins |
| C3a-d | runtime exports, descriptors, literals, leak check; Str, Bytes, List, Map, structs, enums, `Option`, `Result`, nested `match`, `?` compiled. Harness 46 to 173 ran both ways, 0 mismatches, 0 leaked; list and ADT code 4-6x faster than the interpreter |
| C6a, C6b | generator (Int, Bool, control, calls, lists, structs/enums/`Option`/`match`/`?`): 250 programs both ways per CI run; front-end fuzzing, 210,000 inputs, no panic |
| D1, K1 | `Dir` `list`/`write`/`remove`/`exists`; `Kv` (`--kv name=:memory:` or a file). Both adversarially reviewed, fixes merged |
| WS1 | WebSockets under `--listen`, a fresh sandbox per event; 7,800 echo events/s on one connection. Adversarially reviewed, fixes in (Origin check and `--ws-origin`; `close` always follows `open`) |
| kio-web | `api.kio`; notes on Dir and on Kv, 22 checks each |
| L6a | `fn(T) -> R` types and function items as values, called as `f(x)` or `(e)(x)`; no capability inside a `fn` type; interpreter only |

## Next

1. **C3e, C3f** (`main` returning anything; built-ins added after C3 began), then C4 (capabilities), C5 (compiled only), C7 (review), C8 (measure, refcount elision).
2. The checker rejects `o = None;` for an `Option` local ("cannot tell the type of `None`"; open_questions #61).
3. L6b closures, L6c built-ins taking functions, then L7 generics; compiler support for `fn` values (C9) and `Float` (C10).

## Waiting on the maintainer

- **WebSocket upgrade headers:** a program cannot see the upgrade's path or headers; proposed `WsMessage.target()` and `header(name)` on the `open` event (caps.wit).

- open_questions #34: whether to add constants (proposed: check-time literals only). Not decided; nothing depends on it.

## Speed (2026-11-06, release, run time; details in [roadmap.md](roadmap.md))

| | fib(35) | 30 M loop |
|---|---|---|
| Rust | 24 ms | 20 ms |
| kio compiled (plus ~10 ms compile) | 117 ms | 46 ms |
| Python 3.11 | 890 ms | 1.76 s |
| kio interpreter (`kio run` today) | 3.8 s | 5.9 s |
