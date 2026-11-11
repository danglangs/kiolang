# Allocation-heavy code within 2x Rust (C8c plan)

Measurements of the five rows the maintainer named, where the time goes, and the structural changes ranked by predicted gain. Tasks are pinned in [compiler-todo.md](../compiler-todo.md) under C8c. Measured 2026-11-10 on the C8b7 build (99bc847), Apple M3 Max, release, rustc 1.99.1, Wasmtime 50.0.1; bench/ method (best of 5, startup subtracted; other agents' builds ran on the machine, load 3-13, so treat differences under 5% as noise). Fuel is Wasmtime's count, exact and load-free. "Objects" are allocations the runtime frees (a scratch build counted frees; every run leaves 0 live), so they include a `Str` regrown by `push` and exclude a `List`'s separate buffer.

## Measurements

| Row | Rust | kio, fuel on | fuel off | metering | fuel | objects kio / Rust mallocs | vs Rust on / off |
|---|---|---|---|---|---|---|---|
| `fib(35)` | 24 ms | 54 ms | 51 ms | 5% | 1.08 G | 1 / 0 | 2.3x / 2.1x |
| word count, `Map` | 4.9 ms (`HashMap<&str>`); 15.7 ms with `BTreeMap<&str>` | 27 ms | 20 ms | 27% | 0.40 G | 363 k / ~10 | 5.5x / 4.1x (1.7x / 1.3x of `BTreeMap`) |
| word count, `HashMap` | 4.6 ms | 22 ms | 16 ms | 25% | 0.31 G | 363 k / ~10 | 4.7x / 3.5x |
| tree 2^20 | 33 ms | 86 ms | 65 ms | 25% | 1.80 G | 5.24 M / 1.05 M | 2.6x / 2.0x |
| json.kio 1 MB | 7.5 ms (`serde_json`) | 62 ms | 37 ms | 40% | 0.80 G | 1.49 M / ~0.13 M | 8.3x / 5.0x |

"Fuel off" is a scratch host with `consume_fuel(false)` and `epoch_interruption(false)` (bench/README.md), the two binaries run alternately. Metering is a sandbox cost, reported apart; it is not proposed for removal. Its share is large because Wasmtime charges at every basic block (a load, subtract and store of the fuel counter), and generated code is branchy: an inline `inc` is three nested `if`s, a `dec` three, an inline allocation one, an overflow check one.

Micro-benchmarks (3 M passes of one operation over an empty loop of 2.5 ms / 2.1 ms, fuel on / off) give the unit costs the predictions below use:

| One pass | fuel on | fuel off | fuel per pass |
|---|---|---|---|
| a small kio function call (`byte(b, i)`: two bounds checks, a load) | 2.3 ns | 1.1 ns | 23 |
| a pointer-free struct allocated, one field read, freed | 9.0 ns | 6.1 ns | 162 |
| `Some(i)` built by a callee, `unwrap_or`, freed | 14.8 ns | 9.7 ns | 329 |
| a 6-byte `Str` from `slice`, `len`, freed | 17.9 ns | 13.2 ns | 327 |

Data: words.txt has 19,832 lines and 159,354 words over 4,770 distinct; data.json has 89,618 values (28,541 strings, 67,198 object keys, 15,281 objects, 9,160 arrays, 9,159 floats), 9,159 records.

## Attribution

Milliseconds of the fuel-on time, from `bench/profile.py` sample shares (2^22 tree, x10 word count and JSON; 140-430 samples each after the attach stall is dropped, so +-10%) and the object counts; metering is inside every line, so it is given once more as its own line. "Not in Rust" is work the Rust version never does.

| Category | fib | `Map` | `HashMap` | tree | json.kio |
|---|---|---|---|---|---|
| allocation (inline pops, `rt_alloc`, `rt_list_new`, regrows, dlmalloc) | - | 1.8 | 1.1 | 15 | 6 |
| freeing (`obj::dec`, `rt_dec`, free-list pushes) | - | 3.5 | 3.2 | 46 (fuel says 38%: ~33) | 15 |
| reference counts and `unique` checks in generated code | - | 0.5 | 0.5 | 6 | 4 |
| bounds and overflow checks | ~6 (Rust pays 23% for them on fib) | 0.3 | 0.3 | 2 | 3 |
| runtime calls: container search | - | 14.6 (`BTreeMap`) | 11.4 (hash table) | - | 5 (map inserts) |
| runtime calls: strings (`split`, `lines`, copies, `append`, UTF-8 checks) | - | 4.3 | 4.3 | - | 8 |
| program code that is also in Rust | 27 (the recursion) | 1.5 | 1.5 | 12 | 10 |
| not in Rust: a call per byte (`byte`, 2.2 M calls), `Option`/`Result` and struct wrappers, `to_str`/`to_bytes` copies | ~21 (Wasm call: prologue stack check, no inlining) | 0.5 | 0.5 | 5 | 11 |
| metering (within the above) | 3 | 7 | 5 | 21 | 25 |
| total | 54 | 27 | 22 | 86 | 62 |

What the generated code does per unit of work that Rust does not (from the Wasm text of the hot functions, `wasm-tools print` on `codegen::compile`'s output):

- **tree.** `build`: a `Leaf` is a 24-byte object (Rust: 32 bytes inline in the parent's `Vec`); a `Node` is three allocations, the variant, the `List` object and its buffer (Rust: one, the `Vec` buffer). `fold`: every `Stats` is a heap object (2.1 M of them, 40% of the row's objects; Rust: three registers); `s.sum += c.sum` is a `unique` check (load `rc`, compare), a load, an overflow check and a store, three times per child; each child is `inc`'d by the loop's `list_get` and `dec`'d after `fold(k)`, though `fold` borrows it. Freeing the tree walks 3.1 M objects through `free` (one plan read, a free-list push each) against Rust's 1.05 M `free` calls.
- **word count.** Per word: `split` allocates and copies the word (Rust: a `&str` into the text); `get` allocates a `Some` that `unwrap_or` reads and frees (Rust: `entry().or_insert`, no allocation); the inner loop makes four runtime calls (`rt_map_get`, `rt_option_unwrap_or`, `rt_map_insert`, `split` amortized). The search itself: a `BTreeMap` over 4,770 keys is 54% of the `Map` row (Rust's own `BTreeMap<&str>` takes 15.7 ms for this row, 3.5x its `HashMap`); `hmap.rs` is 53% of the `HashMap` row (`Key::find` compares bytes along a chain; the entry's hash is not stored).
- **json.kio.** Every byte goes through `byte(b, i)`: a call with its own bounds check and the inline index's second one, 2.2 M calls (`skip_ws` calls it up to four times per position; `string` twice per byte); `-1` is `0 - 1` with an overflow check (no constant folding). Every value returns `Ok(Parsed { value, pos })`: two objects (Rust: an inline `Value`); every string is `"".to_bytes()` (a copy of the empty literal with capacity 0), a regrow to 16 on the first `push`, then `out.to_str()` (a copy and a UTF-8 check: 95.7 k strings, ~3 wasted objects each); `quote` copies `s.to_bytes()` and `out.to_str()` again; `literal` copies `word.to_bytes()` per literal (27.5 k); encode allocates a `Str` per value and appends it (89.6 k, the program's shape), and `m.get(k).unwrap_or(JsonKio::Null)` a `Some` per key (67 k). 1.49 M objects for 157 k values and keys: 9.5 each, against `serde_json`'s ~1.

Wasmtime's own inliner, measured (`Config::compiler_inlining(Inlining::Yes)`, scratch host): json.kio 62 to 56 ms (-9%), `HashMap` -3%, tree and `Map` within noise, fib 53 to 66 ms (+23%: the recursion is inlined into itself and the body doubles), compile time 30 to 87-93 ms per program. Not adopted as is; its thresholds (`inlining_small_callee_size`, 50 bytes) are not settable in Wasmtime 50. A kio-level inliner of small leaf functions in codegen is proposed instead (C8c3).

## Structural changes

Each with the rows it moves (predicted ms, from the counts above and the unit costs; fuel on), its size, and its risk to compiled == interpreted. The differential harness (506 programs), `differential_gen` (2,000 from a fresh seed), `json_fuzz`, Miri on the runtime and a test-heap runtime run (poisoned frees, no reuse) are the oracle for every one; a change here never touches lowering or the interpreter, so a disagreement is a codegen or runtime bug by construction.

**1. Unboxed one-slot enums in locals, parameters and results** (`Option<T>`, `Result<T, E>`, any enum whose variants carry at most one slot: tag `i32` + payload `i64` as two Wasm values; a function returning one returns two values, Wasm multi-value). Boxed, as today, the moment it enters a slot (a field, element, map value, capture) or leaves one; `rt_map_get`, `parse_int`, `pop`, `find`, `ok`/`err` get variants returning the payload through a runtime scratch word and the tag as the result, so a hit allocates nothing; `match`, `?`, `unwrap_or*`, `map`/`and_then` on the pair test the tag local. `None`'s payload local is 0, so the scope-end `dec` of the payload needs no tag test.
Moves: `Map` -2.4, `HashMap` -2.4 (159 k `Some`), json.kio -4.0 (270 k `Ok`/`Some`), the micro-benchmark's 14.8 ns to ~1 ns. Size: large, 600-900 lines of codegen (every expression position of these types, the moves/borrows analysis per payload local, table wrappers with multi-value signatures) plus ~100 of runtime. Risk: representation only; a value is boxed with the same descriptor and variant wherever `==`, display, freeing or the interpreter's model could observe it. `kiogen` already generates `Option`/`Result` heavily, and the high-bits slot test (C7a) covers `slot_in`/`slot_out`.

**2. Unboxed small structs in locals, parameters and results** (at most 4 slots, each field its own Wasm local; a pointer field is an owned local like a `let`; a struct result is a multi-value return; boxed into a slot as in 1). Field read: `local.get`; `s.f += e`: no `unique` check, no load/store of the object; a struct literal allocates nothing until it is stored.
Moves: tree -24 (2.1 M `Stats` at 9 ns, and 6.3 M `unique` checks and field loads/stores), json.kio -1.7 (185 k `Parsed`/`Scanned`). Size: large, ~500 lines of codegen after 1 (they share the multi-value and boxing machinery; 1 first because its boxing is one slot). Risk: as 1; struct update `..p` and generic instances are lowering's and unchanged. Interaction: a struct with an `Option` field holds it boxed (one slot), as today.

**3. Inlining small kio functions in codegen** (a non-recursive function whose body is under a size threshold and has no loop, inlined at its call sites with the arguments bound to fresh locals; the error sites keep the callee's spans; the moves analysis runs on the inlined body. Experiment within the task: inlining a self-recursive function into itself once, which is what LLVM does for Rust's `fib`, with fib's target as the gate).
Moves: json.kio -5 (2.2 M `byte` calls at 2.3 ns, `is_digit`, `hex_digit`), fib -8 to -10 if the self-inline pays (halves the calls; Wasmtime's inliner lost 23% here, so it is an experiment, not a promise). Size: 300-400 lines. Risk: `--stack` depth changes (a program that overflowed may not), which the harness excludes as a limit; spans unchanged.

**4. Conversions adopt a moved operand; `Bytes` start with room** (`to_str()` and `to_bytes()` on a last-use operand take the reference and rewrite the descriptor word in place after the UTF-8 check, no copy; `"".to_bytes()` and `to_bytes` of a `Str` under 16 bytes get capacity 16, so the first `push` does not regrow). Later in the same task if the measurements still point at it: `split`, `lines`, `slice` returning views (`[rc][desc][parent][ptr][len]`, a +1 on the parent, copied before any in-place append), which removes the copy but not the object; see the maintainer flag on memory.
Moves: json.kio -9 (330 k conversion copies, ~200 k regrows), `Map`/`HashMap` -1.3 with views. Size: ~150 lines runtime, ~50 codegen (a moved operand is what C8b2 gives `x = x + ..`); views ~300 more. Risk: none to semantics (the moved value had one reference and is dead); views change peak memory, not values.

**5. Lists with the buffer inline** (`[rc][desc][len][cap][slots..]` as `Str`/`Bytes` are; `push` past `cap` returns the moved object, stored back as `b.push` already is; `rt_list_new` from a literal sizes it exactly).
Moves: tree -6 (1.05 M buffer allocations and frees, one less indirection per element access), json.kio -0.3, word count -0.3. Size: ~400 lines runtime (`ListBody` users: `coll.rs`, sort, `elems`/`elems_mut`, freeing plans), ~100 codegen (inline `len`/`get`/`push`, `for`). Risk: none to semantics; a moved list object is the `Bytes` discipline, already tested.

**6. Branchless reference counts** (`inc`: `rc += (rc - 1) <u 0x7fffffff` and store, no branch for live or pinned objects, one `if` only for the test-heap path; `dec`: one subtract-and-store, one `if` for "reached 1: free"). Three basic blocks become one, so the fuel bookkeeping per operation drops with them.
Moves: every row with pointers: tree -2 to -3, json.kio -2, word count -0.5 (fuel -3 to -5%). Size: ~60 lines. Risk: none; the test-heap runtime's use-after-free checks must still fire (keep a `rc == 0 or poisoned` call in the slow path).

**7. Faster map searches** (`Map`: a direct-mapped cache in front of the `BTreeMap`, key hash to (key, value) pointers, valid until a structural change, which clears it: `last` from C8b6 generalized; `HashMap`: the hash stored per entry and compared before the bytes, the 8-byte prefix as `StrKey` does). Runtime only; `hmap.rs` is one source with the interpreter, so the interpreter's table changes alike and the model test covers it.
Moves: `Map` -7 to -8 (search 14.6 ms to ~5; a `BTreeMap` lookup becomes a hash, one compare and a tree search only on a miss), `HashMap` -2.5, json.kio -1 (object inserts). Size: ~250 lines. Risk: none to values; order is still the tree's.

**8. Borrowed loop variables** (`for x in xs`: `x` is read in place without its own reference when the body only uses it where C8b4 borrows a parameter; the loop's hidden reference to `xs` keeps the element alive, and a `push` on `xs` in the body copies as today).
Moves: tree -2 (2.1 M `inc`/`dec` pairs), json.kio -0.5. Size: ~150 lines (the C8b4 analysis over a loop body). Risk: none; the same rules as borrowed parameters.

**9. Fuel charged per loop and per call** (Wasmtime's `consume_fuel` off; generated code decrements a counter at every loop back-edge and function entry; the runtime charges its own exports a fixed amount plus a per-byte amount for copies, scans and searches; epochs stay for `--timeout`). Measured ceiling: the "fuel off" column, 25-40% of the runtime-heavy rows, less the counter's own cost (~5%). **Maintainer decision** (#98 kept fuel): it changes what `--fuel` counts and makes the runtime's self-charging part of the security boundary. Pinned last, as C8c9, to be done only if the maintainer takes it.

Not proposed, with the measured reason:

- **Escape analysis to a stack or region.** After 1 and 2, what still allocates without escaping is small: json.kio's `w = word.to_bytes()` (27 k, under 0.5 ms), `split`'s lists (20 k per word-count row, 0.3 ms); everything else escapes into a container or a result. A region needs a sound escape analysis and a second allocation path in every runtime operation; the gain is under 1 ms per row.
- **Unboxed enums and structs inside lists and fields.** A `Leaf(Int)` inline in `List<Tree>` as Rust has it needs 16-byte slots or per-type element layouts: a change to the memory representation every runtime operation, `==`, display and the descriptor table rest on. Tree would lose 1.05 M objects (-10 ms). Flagged below for the maintainer; not pinned.
- **Fewer overflow checks.** They are the language (`Int` overflow is an error in both paths); Rust pays 23% for them on `fib` when enabled. Only constant folding of `-1` and `a - 1` where the operand is a literal is free (part of 3's cleanup).

## Predicted result per row

| Row | today | after C8c1-C8c8 | vs Rust | with C8c9 (fuel per loop) | 2x reachable? |
|---|---|---|---|---|---|
| tree | 86 ms | ~53 ms (2: -24, 5: -6, 6+8: -3) | 1.6x | ~42 | yes |
| `fib` | 54 ms | 54, or ~45 if 3's self-inline pays | 2.3x, or 1.9x | 43 | borderline: the gap is Wasmtime's per-call prologue (stack check for `--stack`) and LLVM's partial inlining of the recursion, not allocation |
| word count, `HashMap` | 22 ms | ~15 (1: -2.4, 4: -1.3, 7: -2.5, 6: -0.5) | 3.2x | ~11.5 (2.5x) | no, not against hashbrown with `&str` keys: zero allocations per word still leaves `hmap.rs`'s search (two per word) and `split`'s scan at ~10 ms; 2x needs C8c9 and a table as fast as hashbrown |
| word count, `Map` | 27 ms | ~15 (1: -2.4, 4: -1.3, 7: -7.5, 6: -0.5) | 3.1x of Rust's `HashMap`, 1.0x of Rust's `BTreeMap<&str>` | ~11.5 | against the idiomatic Rust row no (an ordered map against a hash map); against Rust's own ordered map it is at parity after 7 |
| json.kio | 62 ms | ~40 (1: -4, 2: -1.7, 3: -5, 4: -9, 5+6+7: -3) | 5.3x | ~28 (3.7x) | no: the remaining 28-40 ms is the program's shape against `serde_json`'s (a `Str` built and appended per encoded value, a call per byte, 95 k parsed strings each copied into the tree); the built-in `Json` row, the same work with the runtime building the objects, is already 1.5x Rust (13 ms). Rewriting json.kio to slice strings instead of pushing bytes and to append into one buffer is a program change, outside C8c |

So: tree reaches 2x; `fib` reaches it only if one-level self-inlining works; the word counts reach 2.5-3x without the fuel decision and ~2.3-2.5x with it; json.kio reaches ~4-5x. Where a row cannot reach 2x the table says why; every other change in the list has a measured count behind its prediction.

## For the maintainer

- **Memory model (design.md): unchanged.** Changes 1, 2, 4, 5 and 8 change how a value is represented in Wasm locals and objects, never what a program observes: values still copy on write, counts still live in the runtime, every differential test still checks 0 live objects. Nothing to decide, flagged for the record.
- **Views (4, second half) change peak memory**, not values: a 6-byte word that is a view keeps its 1 MB line's text alive until the last view dies. `--memory` can refuse a program that passed before. Recommend: do the adoption half first; views only if word count still needs them, and then report peak memory before and after.
- **Unboxed values inside lists and fields** (not proposed) would change the object representation every runtime operation rests on; raise it only if tree's 1.6x is not enough.
- **C8c9, fuel per loop and per call:** changes what `--fuel` counts (iterations and calls plus the runtime's self-charging, not instructions) and puts the runtime's charging on the security boundary. Recommend: decide after C8c1-C8c8 land, with the measured gap then in hand; the ceiling is 25-40% on runtime-heavy rows and 5% on compute.
- **Wasmtime inlining** stays off (measured: fib +23%, compile 3x); revisit when its thresholds become configurable.


**Correction (C8c7, 2026-11-10).** Change 7 does not pay: the bench words are near-uniform, so a 64-slot `Map` cache hits 1.4% and costs fuel (+5.7%), and `HashMap` already stores each entry's hash and compares prefixes. Dropped; see compiler-todo.md C8c7.

**After C8c1's review.** `Json` accessors (`get`, `at`, `as_*`) still box an unboxed receiver for each call (24 bytes allocated and freed), so the `_u` exports save the result's allocation only. A `_u` taking the receiver as `(tag, payload)` removes it; take it with C8c8 or when json.kio is profiled again. `Result<Str, ()>`-shaped enums test the tag before each count change though the payload is always a pointer or 0: one compare, for C8c2.

**After C8c2.** Change 2 as predicted on tree (-19 ms, 90 to 71, fuel -22%: `Stats` are registers) but not on json.kio (fuel -1.6%, time flat): `Parsed` and `Scanned` are the payloads of a `Result`, so they are one object each still; the -1.7 ms predicted for them needs the payload's struct unboxed into the enum's locals (two levels) or its fields stolen from a count-1 object (open_questions #187). Tree's remaining gap to 2x (71 vs 33 ms, 1.43 G) is the build's three allocations per node (C8c5) and the `inc`/`dec` pair with a tag test per child in `fold` (C8c8).

**After C8c2's review.** `s.name = s.name + e` copies (the place's root is live before the value), so only `+=` appends in place; lower `s.f = s.f + e` to `s.f += e` as locals already are. A boxed field read at the struct local's last use (`s.p`, `s.o`) takes a reference instead of moving; #187's "steal when the count is 1" covers it.
