# Status

Update at the end of every task. Detail lives in the task entries; this page points to them.

## Now

**Checkpoint reached (2026-11-06): compiled kio runs, proven against the interpreter.** P3 (IR, lowering, interpreter on the IR), C0 (design, [codegen.md](codegen.md)), C1 (runtime), C2a-c (pipeline, Int/Bool/control/calls, differential harness) are done. `host/tests/differential.rs` runs every corpus program both ways; any disagreement fails CI. A deliberate codegen bug (`<` as `<=`) was confirmed to fail it.

**2026-11-08: new primary agent; work runs in parallel streams** (Opus for bulk code, a stronger model for design and security review; CLAUDE.md "How to work").

| Done | Result |
|---|---|
| L3a, L3b, L4a, L4b | match arms `return`/`break`/`continue`; modules use each other; nested patterns and `if let` (Maranget exhaustiveness); Str and List built-ins |
| C3 (a-f) | runtime exports, descriptors, literals, leak check; Str, Bytes, List, Map, structs, enums, `Option`, `Result`, nested `match`, `?`, every built-in, `main` returning any type. Harness 46 to 240 ran both ways, 0 unsupported, 0 mismatches, 0 leaked; list and ADT code 4-6x faster than the interpreter |
| C6a, C6b | generator (Int, Bool, control, calls, lists, structs/enums/`Option`/`match`/`?`): 250 programs both ways per CI run; front-end fuzzing, 210,000 inputs, no panic |
| D1, K1 | `Dir` `list`/`write`/`remove`/`exists`; `Kv` (`--kv name=:memory:` or a file). Both adversarially reviewed, fixes merged |
| WS1 | WebSockets under `--listen`, a fresh sandbox per event; 7,800 echo events/s on one connection. Adversarially reviewed, fixes in (Origin check and `--ws-origin`; `close` always follows `open`) |
| kio-web | `api.kio`; notes on Dir and on Kv, 22 checks each |
| C10, C11 | `Float` and `slice` compiled; harness 267 ran both ways, 0 mismatches (left: `fn` values, C9, and capabilities, C4); kiogen generates floats; float loops compiled run at Rust's speed past ~20 ms of compiling (10 M multiply-adds 39 ms vs 1.49 s interpreted, Rust 20 ms) |
| C4 | capabilities in compiled code: every caps.wit method, host-checked grants shared with the interpreter path, same host functions, quotas, report check and audit log. Harness 252 to 278 ran both ways (26 capability programs, compared with their audit logs); a forged handle is refused by the handle table |
| L6a | `fn(T) -> R` types and function items as values, called as `f(x)` or `(e)(x)`; no capability inside a `fn` type; interpreter only |
| L6b | closures, capturing by copy, lifted to functions in lowering; a closure cannot capture a capability; interpreter only |
| L6c | `map`, `and_then`, `map_err`, `filter`, `any`, `all` on `Option`/`Result`/`List`, typed from the function; a closure argument takes its parameter type from the receiver; interpreter only (C9b) |
| C9a | `fn` values and closures compiled: harness 300 ran both ways, 0 unsupported, 0 mismatches (left: capabilities, C4); a call through a value costs ~10 ns over a direct call (10 M: 144 ms vs 44 ms, incl. compiling) |
| C5 | compiled only: `kio run` and `--listen` compile; the guest crate, its world, wizer and WASI are gone; the interpreter is the native test oracle (`Interp`). On-disk compile cache; fuel defaults a tenth. `Server::handle` 170 to 32 µs, `--listen` startup 0.95 to 0.17 s, idle RSS 66 to 28 MB. Programs with `fn` values fail "not compiled yet" until C9a/C9b |
| C9b | L6c's built-ins taking functions compiled inline: harness 351 ran both ways, 0 unsupported, 0 mismatches, 0 leaked; kiogen generates them; 1 M `map` then `filter` 51 ms compiled vs 233 ms interpreted (a hand-written `for` 39 ms) |
| C7a | C7 review follow-ups: hidden locals reused (17,000 calls in one function compile), more than 999 parameters a checker error, `--memory` below the initial memory reported as out of memory, a high-bits differential test over runtime slots, `Statics::desc` hashed (4,000 structs 263 to 226 ms) |
| L7a | generic functions `fn f<T>(..)`: inferred from arguments, then the expected type, or `f::<Int>`; `T` opaque and never a capability; monomorphized in lowering (`first<Int>`), so compiled with no codegen change: harness 363 ran both ways, 0 mismatches. Adversarially reviewed (rule 1 holds) |
| L7b | generic structs and enums (`struct Page<T>`, `enum Tree<T>`): inferred like calls, never holding a capability, one IR type per instance (displayed without arguments); no codegen change: harness 380 ran both ways, 0 mismatches. Needs its adversarial review (rule 1) |

| C8a | bench/ (`bench/run.sh`): 9 benchmarks and HTTP against Rust, Python and the interpreter, with profiles. Compiled-only code is 1.1-2.3x Rust; runtime-heavy code 3-271x (quadratic `s = s + x`, zero-filled buffers, allocation, refcount and runtime calls, fuel); ranked C8b list in roadmap.md |
| C8b1 | in-place `Str` append (`s = s + x`, `s += x`) when the count is 1, no zero-filled byte buffers, inline `inc`/`dec`/`unique` and list `len`/`get`/`push` (a runtime-owned `ListBody` layout). Strings 1.37 s to 8.5 ms (2.4x Rust), JSON 554 to 144 ms, closures 36 to 13, list 52 to 28, tree 438 to 284 |
| C8b2 | last-use moves and borrowed local operands (no inc/dec), word-at-a-time `Str` compare, one descriptor lookup per free, inline `Bytes` paths; kiogen generates strings. Tree 284 to 244 ms, JSON 143 to 101; word count unchanged (83 ms) |

## Next

1. C8 next (roadmap.md): borrowed parameters; fewer allocations (a size-class free list, a list's buffer in its object; malloc/free are ~25% of tree and JSON); word count's map (key copy per lookup, `BTreeMap` node search).
2. L7b adversarial review (running); L7c (bounds `Eq`, `Ord`).

## Waiting on the maintainer

- **WebSocket upgrade headers:** a program cannot see the upgrade's path or headers; proposed `WsMessage.target()` and `header(name)` on the `open` event (caps.wit).

- open_questions #34: whether to add constants (proposed: check-time literals only). Not decided; nothing depends on it.

## Speed (C8b2, 2026-11-09, release, run time; all in [roadmap.md](roadmap.md))

| | fib(35) | 30 M loop | tree | JSON 1 MB | 100 k concatenations |
|---|---|---|---|---|---|
| Rust | 24 ms | 97 ms | 33 ms | 7.5 ms | 3.5 ms |
| kio compiled (plus 20-25 ms compile, cached) | 55 ms | 111 ms | 244 ms | 101 ms | 7.8 ms |
| Python 3.13 | 732 ms | 928 ms | 2.87 s | 16 ms | 7.0 ms |
| kio interpreter (the test oracle) | 1.62 s | 1.32 s | 1.05 s | 868 ms | 1.57 s |
