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

## Decisions, proposed (owner reviews C0)

1. **Where codegen runs:** in the guest, beside the checker, so the code that checks a program is the code that compiles it. The guest returns Wasm bytes; the host compiles them with Cranelift. Alt: codegen in the host (faster startup, but a second copy of the front end outside the sandbox).
2. **Crates:** `lang` stays dependency-free and gains the IR. A new crate `codegen` depends on `lang` and `wasm-encoder` (Bytecode Alliance). A new crate `runtime` (Rust, `no_std` + `alloc`, built to `wasm32-unknown-unknown`) holds value operations; codegen'd code calls it.
3. **Values:** `Int` and `Bool` as `i64`/`i32`; `()` erased; everything else a pointer to a reference-counted heap object, copied on write (the P2 model, now in the runtime). No cycles exist, so refcounts never leak.
4. **Runtime errors:** the runtime traps with a code and the span is looked up from a table emitted with the module, so messages match the interpreter's.
5. **Fuel:** Wasmtime fuel as today; compiled code uses far less fuel per operation, so default limits are re-measured.

## Correctness

- **Differential tests:** every test program and example runs interpreted and compiled; results, errors and host-call traces must match. Added in C2 and kept for every later task.
- **Generated programs:** a generator of random well-typed kio programs (C6), run through both paths, many thousands per CI run.
- **IR validation:** the interpreter runs the lowered IR (P3c) before any codegen exists, so lowering is proven against all existing tests first.
- **Adversarial review** of the runtime and codegen (C7), as for every boundary so far.

## Tasks

- [ ] **P3a. IR types.** A typed IR in `lang`: locals as slot indexes, fields and variants as indexes, calls by function index, every expression carrying its type. No evaluation yet.
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
