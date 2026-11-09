# Status

Update at the end of every task. Detail lives in the task entries; this page points to them.

## Now

Compiled-only kio covers the whole language and every capability; the active stream is speed (C8): code that stays in compiled Wasm is 1.1-2.1x Rust, runtime-heavy code 2.3-8x (table below). The differential harness runs 440 programs both ways (0 mismatches) and the generator 250 more per CI run.

| Done | Result |
|---|---|
| P3, C0-C2 | IR and lowering; codegen design; the runtime; Int/Bool/control/calls compiled; `host/tests/differential.rs` runs every corpus program both ways and any disagreement fails CI (a deliberate `<` as `<=` bug was caught) |
| L3a, L3b, L4a, L4b | match arms `return`/`break`/`continue`; modules use each other; nested patterns and `if let` (Maranget exhaustiveness); Str and List built-ins |
| C3 (a-f) | runtime exports, descriptors, literals, leak check; Str, Bytes, List, Map, structs, enums, `Option`, `Result`, nested `match`, `?`, every built-in, `main` returning any type. Harness 46 to 240 ran both ways, 0 unsupported, 0 mismatches, 0 leaked; list and ADT code 4-6x faster than the interpreter |
| C6a, C6b | generator (Int, Bool, control, calls, lists, structs/enums/`Option`/`match`/`?`): 250 programs both ways per CI run; front-end fuzzing, 210,000 inputs, no panic |
| D1, K1 | `Dir` `list`/`write`/`remove`/`exists`; `Kv` (`--kv name=:memory:` or a file). Both adversarially reviewed, fixes merged |
| WS1 | WebSockets under `--listen`, a fresh sandbox per event; 7,800 echo events/s on one connection. Adversarially reviewed, fixes in (Origin check and `--ws-origin`; `close` always follows `open`) |
| WS1b | `WsMessage.target()` and `header(name)`: every event of a connection sees its upgrade request (caps.wit, owner-approved); kio-web `examples/ws/` routes by path and checks a token |
| kio-web | `api.kio`; notes on Dir and on Kv, 22 checks each |
| C10, C11 | `Float` and `slice` compiled; harness 267 ran both ways, 0 mismatches (left: `fn` values, C9, and capabilities, C4); kiogen generates floats; float loops compiled run at Rust's speed past ~20 ms of compiling (10 M multiply-adds 39 ms vs 1.49 s interpreted, Rust 20 ms) |
| C4 | capabilities in compiled code: every caps.wit method, host-checked grants shared with the interpreter path, same host functions, quotas, report check and audit log. Harness 252 to 278 ran both ways (26 capability programs, compared with their audit logs); a forged handle is refused by the handle table |
| L6a | `fn(T) -> R` types and function items as values, called as `f(x)` or `(e)(x)`; no capability inside a `fn` type; compiled in C9a |
| L6b | closures, capturing by copy, lifted to functions in lowering; a closure cannot capture a capability; compiled in C9a |
| L6c | `map`, `and_then`, `map_err`, `filter`, `any`, `all` on `Option`/`Result`/`List`, typed from the function; a closure argument takes its parameter type from the receiver; compiled in C9b |
| C9a | `fn` values and closures compiled: harness 300 ran both ways, 0 unsupported, 0 mismatches (left: capabilities, C4); a call through a value costs ~10 ns over a direct call (10 M: 144 ms vs 44 ms, incl. compiling) |
| C5 | compiled only: `kio run` and `--listen` compile; the guest crate, its world, wizer and WASI are gone; the interpreter is the native test oracle (`Interp`). On-disk compile cache; fuel defaults a tenth. `Server::handle` 170 to 32 µs, `--listen` startup 0.95 to 0.17 s, idle RSS 66 to 28 MB. |
| C9b | L6c's built-ins taking functions compiled inline: harness 351 ran both ways, 0 unsupported, 0 mismatches, 0 leaked; kiogen generates them; 1 M `map` then `filter` 51 ms compiled vs 233 ms interpreted (a hand-written `for` 39 ms) |
| C7a | C7 review follow-ups: hidden locals reused (17,000 calls in one function compile), more than 999 parameters a checker error, `--memory` below the initial memory reported as out of memory, a high-bits differential test over runtime slots, `Statics::desc` hashed (4,000 structs 263 to 226 ms) |
| L7a | generic functions `fn f<T>(..)`: inferred from arguments, then the expected type, or `f::<Int>`; `T` opaque and never a capability; monomorphized in lowering (`first<Int>`), so compiled with no codegen change: harness 363 ran both ways, 0 mismatches. Adversarially reviewed (rule 1 holds) |
| L7b | generic structs and enums (`struct Page<T>`, `enum Tree<T>`): inferred like calls, never holding a capability, one IR type per instance (displayed without arguments); no codegen change: harness 380 ran both ways, 0 mismatches. Adversarially reviewed (rule 1 holds) |
| L7d | generic types usability: a literal, variant or call its values leave open waits for the expected type (`set(Page { items: [], next: None }, true)`); "expected Tree<Int>, found Other<..>"; the 65th instance through a type that does not grow names that type; `Box::<Int> { .. }` points to the `let`. Harness 398 ran both ways, 0 mismatches |
| L7e | `< <= > >=` on `Str` by UTF-8 bytes (Rust's `str` order, as `sort()`); compiled through one runtime export `rt_str_cmp` over `text::bytes_cmp`; kiogen generates Str comparisons. Harness 400 ran both ways, 0 mismatches |
| L7c | bounds `T: Eq`, `T: Ord` on functions, structs and enums: `==`/`contains` and `<`/`sort()` on bounded parameters; type arguments checked at calls, function values, literals, variants and every written type (`Ord`: Int and Str only); bounds pass through callers. Lowering and codegen unchanged. Harness 404 ran both ways, 0 mismatches |
| C8a | bench/ (`bench/run.sh`): 9 benchmarks and HTTP against Rust, Python and the interpreter, with profiles. Compiled-only code is 1.1-2.3x Rust; runtime-heavy code 3-271x (quadratic `s = s + x`, zero-filled buffers, allocation, refcount and runtime calls, fuel); ranked C8b list in roadmap.md |
| C8b1 | in-place `Str` append (`s = s + x`, `s += x`) when the count is 1, no zero-filled byte buffers, inline `inc`/`dec`/`unique` and list `len`/`get`/`push` (a runtime-owned `ListBody` layout). Strings 1.37 s to 8.5 ms (2.4x Rust), JSON 554 to 144 ms, closures 36 to 13, list 52 to 28, tree 438 to 284 |
| C8b2 | last-use moves and borrowed local operands (no inc/dec), word-at-a-time `Str` compare, one descriptor lookup per free, inline `Bytes` paths; kiogen generates strings. Tree 284 to 244 ms, JSON 143 to 101; word count unchanged (83 ms) |
| C8b3 | size-class free lists (8-64 bytes) over dlmalloc, handed back past 256 KB or when memory runs out. Tree 243 to 179 ms, JSON 100 to 82, word count 83 to 75, strings 7.9 to 5.2 |
| C8b4 | borrowed parameters (a pointer parameter only read in place is passed without an inc; optimistic fixpoint, `fn`-value wrappers adapt) and `Str` map lookups without a key copy. Word count 74 to 70 ms, JSON 83 to 80, tree 179 to 185 (same count, code moved) |
| L9, C12 | built-in JSON: a reserved `Json` enum, `Str.parse_json() -> Result<Json, Str>`, `Json.to_json()`, accessors (`get`, `at`, `as_*`), json.kio's rules and messages, depth 128; the interpreter (lang/src/json.rs) and the runtime (runtime/src/json.rs, building kio objects directly) agree on the harness, a generated-JSON fuzz test and byte mutations. 1 MB parse + encode 17 ms compiled (`serde_json` 7.8 ms, json.kio 82 ms). kio-web still declares its own `Json` and is refused until it moves to the built-in |
| C8b5 | less work per operation (fuel stays): generated code pops the size-class free lists, fresh blocks carved 32 at a time from 64 KB chunks, freeing without descriptor copies, `Str` map keys compared by an 8-byte prefix. Tree 185 to 98 ms (fuel 3.78 to 2.21 G), word count 70 to 45, JSON 81 to 66 |
| L10, C13 | `HashMap<K, V>` (owner-approved): `Map`'s keys and methods, insertion-ordered iteration, order-insensitive `==`, a fixed hash with buckets sorted past 8 entries (100,000 colliding keys in under a second either way; with chains only, out of fuel). One table source (lang/src/hmap.rs) for the interpreter and the runtime; codegen reuses `Map`'s calls. Word count 45 ms on `Map`, 32 on `HashMap` (Rust 5.7); kiogen generates maps; harness 440 ran both ways, 0 mismatches |

## Next

C8 (roadmap.md): freeing (54% of tree); word count's double search for `insert(k, get(k) + 1)` (open_questions #110, #115; on `HashMap` too) and `split`'s allocations; borrowed match bindings of a borrowed scrutinee (tree).

## Waiting on the maintainer

Nothing.

## Speed (C8b5, 2026-11-09, release, run time; all in [roadmap.md](roadmap.md))

| | fib(35) | 30 M loop | tree | JSON 1 MB | 100 k concatenations |
|---|---|---|---|---|---|
| Rust | 23 ms | 97 ms | 33 ms | 8.0 ms | 4.6 ms |
| kio compiled (plus 20-25 ms compile, cached) | 53-57 ms | 106 ms | 98 ms | 66 ms | 5.2 ms |
| Python 3.13 | 704 ms | 933 ms | 2.87 s | 17 ms | 7.2 ms |
| kio interpreter (the test oracle) | 1.54 s | 1.27 s | 1.03 s | 812 ms | 1.57 s |
