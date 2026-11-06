# Compiler TODO

Compile kio to Wasm instead of interpreting it, for near-native speed (docs/roadmap.md). Owner decision 2026-11-05: go from P3 lowering straight to a Wasm backend; no bytecode VM. Tasks run in order, each pinned before handoff, as in lang-todo.md.

## Shape

```
source ─► parse ─► check ─► lower (P3) ─► IR ─┬─► interpreter (reference, kept)
                                             └─► codegen ─► program.wasm ─┐
                         runtime.wasm (Rust: allocator, refcounts, Str, List, Map) ┤
                                                    Wasmtime links, compiles, runs ┘
```

- **The interpreter stays** as the reference. Every compiled program must behave exactly as the interpreter: same value, same runtime error and span, same host calls in the same order.
- **The sandbox is unchanged.** Compiled code calls the same host functions under the same limits (fuel, memory, time). A compiler bug can give a wrong answer, but cannot escape: Wasmtime validates every module, and the host enforces every grant (layer 2).
- **The checker still decides what runs.** Codegen accepts only checked, lowered programs.
- **The host checks and compiles; the guest only runs** (owner decision 2026-11-06). The host is already trusted with every grant, so a second check inside the sandbox adds no safety. Today the guest re-checks the source (W13b snapshots that); with the compiler, the sandbox receives compiled code and nothing parses source inside it. This also resolves open_questions #33: no checking within the sandbox's memory or fuel, and no source copied in per request. Layer 1 (static) moves to the host; layers 2 and 3 are unchanged.

## Decisions, proposed (owner reviews C0)

1. **Where codegen runs (decided): in the host**, after the host's check. The host emits the program's Wasm and compiles it with Cranelift. Adversarial source is parsed outside the sandbox, so the front end's depth limits and fuzzing (C6) matter: a parser bug is a host bug. Alt: codegen in the guest, returning Wasm bytes.
2. **Crates:** `lang` stays dependency-free and gains the IR. A new crate `codegen` depends on `lang` and `wasm-encoder` (Bytecode Alliance). A new crate `runtime` (Rust, `no_std` + `alloc`, built to `wasm32-unknown-unknown`) holds value operations; codegen'd code calls it.
3. **Values:** `Int` and `Bool` as `i64`/`i32`; `()` erased; everything else a pointer to a reference-counted heap object, copied on write (the P2 model, now in the runtime). No cycles exist, so refcounts never leak.
4. **Runtime errors:** the runtime traps with a code and the span is looked up from a table emitted with the module, so messages match the interpreter's.
5. **Fuel:** Wasmtime fuel as today; compiled code uses far less fuel per operation, so default limits are re-measured.
6. **Depth limit (owner decision, before P3c):** today "evaluation nested deeper than 3_000" counts nested expressions, which mirrors the AST node for node and cannot be reproduced by compiled code. Proposed: a call-depth limit, checked at each call with the call's span; nesting within one function is already bounded by the parser. The error message stays "evaluation is too deep"; docs/language.md changes from "nested deeper than 3_000 (about 1_400 recursive calls)" to a call depth.

## Correctness

- **Differential tests:** every test program and example runs interpreted and compiled; results, errors and host-call traces must match. Added in C2 and kept for every later task.
- **Generated programs:** a generator of random well-typed kio programs (C6), run through both paths, many thousands per CI run.
- **IR validation:** the interpreter runs the lowered IR (P3c) before any codegen exists, so lowering is proven against all existing tests first.
- **Adversarial review** of the runtime and codegen (C7), as for every boundary so far.

## Measuring

Every task from P3c on re-runs the benchmark set and records it in its result line: `fib(32)` and the 3 M-iteration loop from docs/roadmap.md, JSON parse of 100 KB, and the serve example's `Server::handle` median. Baseline (2026-11-06, release, interpreter): fib(32) 1.07 s native and 2.6 s in Wasm; loop 0.33 s and 0.85 s; Python 0.4 s and 0.18 s; Rust 0.013 s and 0.003 s. Profile with macOS `sample` on the native interpreter. The baseline profile of fib: most time in per-call name hashing (SipHash on the function name), `rsplit_once` on the name for its module, cloning parameter names into the locals, string compares on lookup and on operators, and allocating argument vectors. P3 removes all of these.

## Tasks

- [x] **P3a. IR types.** `lang/src/ir.rs`: a `Module` of functions, structs, enums and capabilities addressed by index; locals as slots with names kept for errors; operators, built-in methods and mutating methods as enums; every expression carrying its type. Reviewed for both consumers (interpreter and Wasm codegen); review fixes applied: mutation is an expression (`rows.push(r)` is a match arm), struct literals keep source order (evaluation order), index places carry the `xs[i]` span, `+=` names its operator so `Str` and `Int` differ, `main`'s parameter names survive for grant errors, capability error enums come first with fixed ids.
- [ ] **P3b. Lowering.** Checked AST to IR, for the whole language. Tests: IR snapshots for small programs.
- [ ] **P3c. Interpreter on the IR.** The interpreter runs the IR instead of the AST; all tests pass unchanged; measure (expect 2-5x from slot lookups alone).
- [ ] **C0. Design review.** The decisions above, a value layout, and the runtime API, written up for the owner. One-way door: wait for approval.
- [ ] **C1. Runtime.** Allocator, refcounts, `Str`, `Bytes`, `List`, `Map` operations; unit-tested natively and on wasm32.
- [ ] **C2. Codegen: Int, Bool, control flow, functions.** `fib` and `loop` compile and match the interpreter; differential test harness lands here.
- [ ] **C3. Codegen: Str, Bytes, List, Map, structs, enums, `match`.**
- [ ] **C4. Codegen: capabilities, host calls, runtime errors with spans, depth limit.**
- [ ] **C5. Integration.** `kio run` and `--listen` run compiled code (the `--listen` snapshot compiles once); the interpreter remains behind a flag for comparison.
- [ ] **C6. Program generator** for differential testing.
- [ ] **C7. Adversarial review** of runtime and codegen.
- [ ] **C8. Measure** against Rust and Python (roadmap table); update docs/perf.md and docs/roadmap.md.
