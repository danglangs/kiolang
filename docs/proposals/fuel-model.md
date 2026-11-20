# A fuel charging model (C8c9)

What `--fuel` should count, measured against what it costs. Pinned by [compiler-todo.md](../compiler-todo.md) C8c9's 2026-11-19 requirement: a written charging model and its adversarial review before any implementation.

Measured 2026-11-19 on main (73a1ac9), Apple M3 Max, release, rustc 1.100.0, Wasmtime 50.0.1, load 9-11 (other agents' builds). Times are `used.ms` from `kio run --json` (the run alone, no startup), best of 7 rounds with the five binaries alternating inside each round; differences under 5% are noise (fib and mandel show the band). Fuel is exact and load-free. Scratch hosts: `consume_fuel(false)` and/or `epoch_interruption(false)` in host/src/config.rs (with `set_fuel` made optional), and `flat`: Wasmtime's `Config::operator_cost` table with every flat cost 0 except `br`, `br_if`, `br_table` and the calls (1), bulk operations still 1 a byte. Programs: bench/kio and the probes below. Nothing committed but this document.

## What fuel must do

| Purpose | Needs |
|---|---|
| Stop runaway code | every generated cycle and recursive path, and every input-dependent runtime operation, spends fuel; a bound on how long one operation can run uncharged; the same count on every OS and machine (replay, `kio review`'s both-sides comparison) |
| Resource-regression evidence (`kio test` per-test fuel T3, `kio review` 2% threshold, PG1's 2% gate) | fuel proportional to work done, so a 2% move means something; stable across runs; a version, so a baseline from another model is never compared |

Both purposes need fuel to track real work. The yardstick below is **ns of guest time per fuel unit**: a model is good when that ratio stays in a narrow band across programs, and bad where an adversary can make it large (cheap-metered, expensive) or tiny (over-charged work, which only costs users headroom).

## Today

Wasmtime's `consume_fuel`: 1 per Wasm instruction except `nop`, `drop`, `block`, `loop`, `unreachable`, `return`, `else`, `end` (0); bulk operations (`memory.copy`, `memory.fill`, `memory.init`, `memory.grow`) 1 a byte or page besides, charged after the operation succeeds and followed by a check (`VariableOperatorCost`, Wasmtime 50). Instructions are summed per basic block and added to a function-local counter at block ends, saved to the store before every call and `return` and reloaded after every call; the remaining fuel is checked at function entry and every loop header. Epochs: a 100 ms ticker, checked at the same sites (function entry, loop header), `--timeout` x 10 ticks. Instantiation is exempt (#268); `rt_init`'s free plans are charged (#276).

**Stale note.** #110 and #333 say Wasmtime charges `memory.copy` as one operation. Wasmtime 50.0.1 (Cargo.lock) charges a byte each: the `copy` probe below copies 200 MB for 211.9 M fuel (1.00 a byte). Both entries need a correction; C20's "+7.2% since `memory.copy` counted as one" was a mis-attribution (the appends were already charged by length).

## 1. Metering cost, re-measured

| Row | fuel | on (today) | fuel only (epochs off) | epochs only (fuel off) | both off | `flat` | metering, on vs both off |
|---|---|---|---|---|---|---|---|
| fib | 1.060 G | 35.6 ms | 35.9 | 37.5 | 37.1 | 42.4 | ~0 |
| loop | 2.100 G | 102.9 | 103.3 | 103.1 | 104.0 | 103.0 | ~0 |
| mandel | 0.285 G | 12.2 | 11.7 | 11.6 | 11.7 | 11.9 | ~0 |
| strings | 0.060 G | 3.4 | 2.9 | 2.6 | 2.2 | 3.2 | 35% (noisy) |
| list | 0.959 G | 26.7 | 25.5 | 25.0 | 21.8 | 26.2 | 18% |
| tree | 1.307 G | 67.6 | 57.9 | 58.3 | 51.5 | 66.5 | 24% |
| closures | 0.200 G | 6.4 | 6.2 | 5.7 | 5.6 | 6.2 | 13% |
| map | 0.339 G | 23.4 | 21.5 | 20.9 | 17.3 | 23.0 | 26% |
| hashmap | 0.243 G | 17.9 | 16.7 | 15.8 | 13.4 | 17.7 | 25% |
| json (json.kio) | 0.622 G | 34.9 | 33.0 | 29.7 | 24.1 | 34.6 | 31% |
| json_builtin | 0.179 G | 12.9 | 11.5 | 10.9 | 8.8 | 12.7 | 32% |
| csv | 0.185 G | 12.0 | 11.0 | 10.3 | 8.9 | 11.8 | 26% |
| logscan | 0.254 G | 15.7 | 13.8 | 12.2 | 9.3 | 15.4 | 41% |
| records | 0.199 G | 13.5 | 11.8 | 11.3 | 9.1 | 13.1 | 33% |
| `kio run` of an empty program (wall) | | 38.8 | 35.8 | 34.1 | 29.6 | 38.6 | 24% |

Three readings, each new against speed-plan.md:

1. **Epochs are half the cost.** speed-plan.md measured "epochs alone within noise"; with the binaries alternated per round they are not: on the allocating rows fuel alone costs 5-15% and epochs alone 5-22%, roughly additive (tree: 57.9 fuel only, 58.3 epochs only, 67.6 both). Both insert the same thing at the same sites: a load, a compare and a branch at every function entry and loop header, and the runtime's hot functions (`inc`/`dec` slow paths, allocation, `Str` operations) are tiny, so entry checks dominate.
2. **Charging less often saves nothing.** `flat` adds fuel only at branches and calls, so it removes nearly every per-block add, yet runs within noise of today on every row (tree 66.5 vs 67.6; fib slower, 42.4, since its tiny function now spends more on the save/reload around its two calls than on the adds). What costs is the check and spill structure at entries, loop headers and calls, not the number of increments. Weighted or every-N charging inside Wasmtime's scheme cannot pay; a hand-rolled counter at the same sites would cost about what the epoch check costs (its instruction sequence is the same length), that is about the "epochs only" column: tree ~58 ms.
3. **The ceiling C8c9 was pinned against is wrong.** Its "fuel off" column had epochs off too. C8c9 as pinned keeps epochs and adds a hand-rolled check at every loop and call: predicted tree 67.6 to ~65, json.kio 34.9 to ~34, hashmap 17.9 to ~17.5, not the 21-33% the speed plan lists. The two-mechanism structure is the cost; the charging model is almost free either way.

## 2. Gaps in today's model

Probes (scratchpad, 1 MB operands built by doubling; `read` reads bench/data/words.txt 50 times through a `Dir`):

| Probe | Work | fuel | ms | ns per fuel |
|---|---|---|---|---|
| `emptyloop` | 3 M iterations, `a = a + i; a = a % p` | 150.0 M | 9.2 | 0.061 |
| `unrolled` | the same loop, 64 adds a body | 3,930 M | 108.3 | 0.028 |
| `copy` | `slice` of 1 MB, 200 times (`memory.copy`) | 211.9 M | 3.5 | 0.017 |
| `copy_small` | 1 KB slices, 204,800 times | 371.3 M | 9.7 | 0.026 |
| `concat` | `"y" + s`, s 1 MB, 200 times | 211.9 M | 3.2 | 0.015 |
| `eq` | `s == t` on 1 MB, 200 times | 632.3 M | 22.2 | 0.035 |
| `lower` | `to_lowercase` of 1 MB, 50 times | 1,581.6 M | 24.1 | 0.015 |
| `sha` | `sha256` of 1 MB, 50 times | 6,786.4 M | 178.8 | 0.026 |
| `sortrev` | 1 M descending `Int`s sorted | 99.6 M | 3.9 | 0.039 |
| `deepfree` | a 1 M-element `List<Str>` freed, 5 times | 328.7 M | 12.8 | 0.039 |
| `read` | 1 MB file read 50 times (host) | 0.08 M | 6.0 | 74 |

Guest work lands between 0.015 and 0.061 ns a fuel unit, a 4x band, with the cheapest-charged work (`memory.copy` at memmove speed) over-charged, never under: no probe makes expensive guest work cheap. The gaps:

| Gap | Size | Bounded by | Verdict |
|---|---|---|---|
| Host work is outside fuel: `Dir`, `Http`, `Kv`, `Db` (SQLite runs natively), `Clock.sleep_ms` | `read`: 1,000x the guest ratio | `--calls`, `--io`, per-call timeouts, SQLite's progress handler and scan checks, the run's deadline checked at every host call | by design; fuel measures the program, not its services |
| Bulk operations as one instruction (#110, #333) | none: 1 a byte measured | | closed by Wasmtime 50; correct the notes |
| Instantiation exempt (#268) | 82 K on macOS, 0 on Linux | module size, `--timeout` | keep (OS-independence) |
| `rt_init` plans charged (#276) | ~150 a type, ~1,000 per `Http` program | | a test-noise item, not a safety one |
| A long bulk operation runs uncharged until it ends | one `memory.copy` of at most `--memory` bytes (256 MB, ~5 ms) | `--memory` | acceptable |
| Interruption latency for `--timeout` | one epoch tick (100 ms) plus one loop iteration or straight-line function | | acceptable |

Determinism: fuel depends on the Wasm (codegen version), the runtime's Wasm (the pinned Rust, #267) and Wasmtime's cost table (the pinned Wasmtime), none on the OS or machine. The same holds for every model below.

## 3. Candidate models

| | Model | Mechanism |
|---|---|---|
| A | **today**: Wasmtime per instruction | as above; nothing to build |
| A' | A with epochs only when fuel cannot bound the run | `epoch_interruption(fuel * MAX_NS_PER_FUEL > timeout)` per run (each run has its own engine, #72; under `--listen` the per-request limits are known at startup); `--timeout` is already checked by the host at every host call and sleep (H36, `State::within`) |
| B | flat charge per loop back-edge and per call, Wasmtime fuel off, epochs on | codegen: a global counter decremented at every back-edge and function entry, trap at zero; runtime: every export charges a fixed amount plus per byte or element (`rt_fuel(n)`); the C8c9 pin |
| C | weighted B | back-edge by the body's static instruction count; call by the callee's static size; runtime as B |
| D | less frequent B | a per-loop local counts iterations, the global charged and checked every N; runtime as B |
| E | runtime self-charging alone | B's runtime half with Wasmtime fuel kept for generated code: no gain (the runtime's instrumentation is most of the cost) and two units; listed to close it |

B, C and D need the same runtime half: of the 110 non-capability exports about 75 do input-dependent work (`split`, `lines`, `replace`, `find`, `to_lowercase`, `sha256`, `eq`/`cmp`, `concat`, `sort`, `sort_by_key` and the `rt_sort_*` protocol, `join`, `contains`, `slice`, map search, insert and remove, `parse_json`, `to_json`, display, `rt_dec`'s free walk, allocation and growth), each needing a charge formula that is right for every input, including collisions (a `HashMap` bucket sorts past 8 entries: a hostile key set costs log n full-key compares an operation, so the charge must count compares, not entries) and generated code's inline fast paths (`str_append` under 8 bytes, list `push`/`get`, `inc`/`dec`, size-class pops), which the back-edge charge covers only because each is O(1).

## 4. Comparison

| Criterion | A today | A' | B flat | C weighted | D every N |
|---|---|---|---|---|---|
| guest ns per fuel, measured band | 0.015-0.061 (4x) | same | `flat` proxy: 0.016 (copy) to 17.2 (`unrolled`), 1,000x; `unrolled` 11x `emptyloop` at the same fuel | fixes `unrolled` (64 adds counted); runtime formulas still hand-written | as B, plus N iterations uncharged |
| cheap-metered expensive programs | none found; host work is the only one, bounded apart | same | an unrolled body; work moved into any export whose formula is missing or too low; a collision-heavy `HashMap`; `==` on deep values (`rt_eq` walks both) | the runtime cases of B | B's plus a loop ended before N |
| deep recursion | charged per call and instruction | same | per call; `--stack` bounds depth | per call by callee size | per call |
| giant copies and frees | 1 a byte (Wasmtime); frees per instruction | same | per byte and per object, by formula | same | same |
| overflow safety | Wasmtime: i64, bulk charges saturate | same | i64 global; a charge is at most `--memory` bytes x cost, so the sum fits; saturate the multiply | same | same |
| uncharged stretch (interruption latency for `--fuel`) | one basic block, or one bulk op of at most `--memory` bytes | same | one runtime export on one input: `sort` of 30 M `Int`s ~150 ms, `sha256` of 256 MB ~45 s, unless charged up front from the length (possible when the cost is a known function of it) | same | N iterations more |
| `--timeout` latency | 100 ms tick | 100 ms where epochs are on; where off, fuel bounds guest time to `MAX_NS_PER_FUEL x fuel` and the host checks the deadline at every call and sleep | 100 ms | 100 ms | 100 ms |
| determinism | yes, OS-independent | yes | yes; also depends on the runtime's formulas | yes | yes |
| cost to build | 0 | S (host config predicate, the `MAX_NS_PER_FUEL` probe set, tests, docs) | L: codegen at every loop and entry; ~75 export formulas; host, tests that hunt cheap-metered programs, docs | B + M (static counts; inlining changes them) | B + M (per-loop counters) |
| what the runtime's charging is | Wasmtime's, reviewed upstream | same | hand-written, on the security boundary (#98's objection) | same | same |
| time gained on allocating rows | 0 | 5-15% (the "fuel only" column), startup -3 ms | ~0-4% with epochs kept (reading 3); 5-15% only with epochs off, which is A' | as B | as B |
| PG1 baselines, T3 notes, `--fuel` defaults | unchanged | unchanged (epochs spend no fuel) | all regenerated; a new unit 20-35x smaller (fib 1.06 G to 30 M, a request 54 K to ~2 K); `--fuel` and `Limits::web` re-derived (#76); the 2% thresholds sit on coarser steps | same | same |
| what `--fuel` means to a user | Wasm instructions, copies by length | same | iterations and calls plus the runtime's own estimate | same, weighted | same |

Versioning, for any model: a `fuel_model` integer (today 1) in `--json`'s `used`, the audit `start` line, `kio test --json` and bench/baseline.json; `kio review` compares nothing across versions and says so; `gate.py` fails a baseline of another version with "regenerate with `--update`". A Wasmtime or Rust upgrade that changes counts keeps the version and regenerates the baseline, as #267 does today.

## 5. Recommendation

**Keep today's model (A); do not build B, C, D or E.** Its one named gap is already closed by Wasmtime 50; across every probe it keeps guest work inside a 4x band with no cheap-metered program found, while the `flat` proxy shows a per-loop model diverging 11x on one unrolled body and leaving ~75 hand-written charge formulas on the security boundary. The gain B was pinned for is not there: charging less often saves nothing, and with epochs kept the hand-rolled check costs what Wasmtime's does.

**Take the time where it is: the second check. Pin A' as a host task** (the `--timeout` mechanism, not the charging model), with an adversarial review before it lands:

| Step | Content |
|---|---|
| `MAX_NS_PER_FUEL` | measured worst guest time per fuel unit, from a probe set kept in bench/ (`emptyloop`, a first-touch page walk, `memory.grow` to `--memory`, float ops, the rows), times a safety factor for slow machines (start at 1 ns: 16x the worst probe); documented as the bound on how late `--timeout` can fire when epochs are off |
| rule | `epoch_interruption` on iff `fuel x MAX_NS_PER_FUEL > timeout`; off at every default (`kio run`: 10 G x 1 ns = 10 s against 10 s: on the edge, so either lower the factor or set it from the measured 0.07 ns x 16; `--listen`: 10 M against 5 s, off); on for bench (`--fuel 10^15`) |
| host | the deadline is already checked at every host call and `sleep_ms` (H36); confirm by test that a run that only spins at `--fuel 10^15 --timeout 1` still stops in ~1 s (epochs on), and that `--fuel 10^9 --timeout 1` stops by fuel within the bound |
| tests | the "hunt" set: every probe above as a test asserting `ms / fuel` under `MAX_NS_PER_FUEL` on CI (a ratio bound, not a time, so load inflates it only toward failure; set the factor so CI passes with headroom) |
| gain | the "fuel only" column: tree 67.6 to ~58 ms, json.kio to ~33, `HashMap` to ~16.7, logscan to ~13.8, records to ~11.8, startup -3 ms; PG1 unchanged |
| undo | the predicate back to `true` (one line) |

Also, now: correct #110 and #333 (bulk operations are charged a byte), add `fuel_model: 1` to `used`, the audit header and baseline.json so a later change is never compared blind, and close C8c9 as not taken with this document as its result. Revisit a per-loop model only if a profile after A' shows the fuel adds (not the checks) above 5% of a row, which `flat` says they are not.

## For the maintainer

1. **C8c9 (B): recommend not taking it.** Measured: the per-loop model's gain with epochs kept is ~0-4%, its divergence 1,000x on probes, and ~75 charge formulas would join the security boundary.
2. **A', epochs only when fuel cannot bound the run:** host-only, no caps.wit or language change, `--timeout` still fires in guest code wherever it could matter; a two-way door I would take after its review, since it moves what `--timeout` guarantees in principle (late by at most `MAX_NS_PER_FUEL x fuel`) if not in practice. Recommend: yes, pinned with the review and the probe set as its tests.
3. Nothing else here needs a decision.
