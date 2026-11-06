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

## Decisions

Approved by the owner 2026-11-06: 2 (a Rust runtime library), 3 (values: refcounted, copy on write; not WasmGC) and emitting Wasm directly from the IR with no optimizing IR of our own yet. C0 writes up the value layout and runtime API for review before codegen.

1. **Where codegen runs (decided): in the host**, after the host's check. The host emits the program's Wasm and compiles it with Cranelift. Adversarial source is parsed outside the sandbox, so the front end's depth limits and fuzzing (C6) matter: a parser bug is a host bug. Alt: codegen in the guest, returning Wasm bytes.
2. **Crates:** `lang` stays dependency-free and gains the IR. A new crate `codegen` depends on `lang` and `wasm-encoder` (Bytecode Alliance). A new crate `runtime` (Rust, `no_std` + `alloc`, built to `wasm32-unknown-unknown`) holds value operations; codegen'd code calls it.
3. **Values:** `Int` and `Bool` as `i64`/`i32`; `()` erased; everything else a pointer to a reference-counted heap object, copied on write (the P2 model, now in the runtime). No cycles exist, so refcounts never leak.
4. **Runtime errors:** the runtime traps with a code and the span is looked up from a table emitted with the module, so messages match the interpreter's.
5. **Fuel:** Wasmtime fuel as today; compiled code uses far less fuel per operation, so default limits are re-measured.
6. **Depth limit (decided 2026-11-06): the Rust model.** Deep recursion is bounded by stack size, not a count: compiled code relies on Wasmtime's stack limit (`--stack`), whose trap the host reports as "stack overflow (raise it with --stack)". The interpreter, which recurses on the native stack, keeps a call-depth counter instead of today's expression counter, checked at each call with the call's span; nesting within one function is already bounded by the parser. Spec: "deep recursion stops the run; `--stack` sets the limit". The depth at which a program stops may differ between the interpreter and compiled code, as it does between Rust builds; differential tests (C2) compare results only for programs that stay within both limits.

## Correctness

- **Differential tests:** every test program and example runs interpreted and compiled; results, errors and host-call traces must match. Added in C2 and kept for every later task.
- **Generated programs:** a generator of random well-typed kio programs (C6), run through both paths, many thousands per CI run.
- **IR validation:** the interpreter runs the lowered IR (P3c) before any codegen exists, so lowering is proven against all existing tests first.
- **Adversarial review** of the runtime and codegen (C7), as for every boundary so far.

## Measuring

Every task from P3c on re-runs the benchmark set and records it in its result line: `fib(32)` and the 3 M-iteration loop from docs/roadmap.md, JSON parse of 100 KB, and the serve example's `Server::handle` median. Baseline (2026-11-06, release, interpreter): fib(32) 1.07 s native and 2.6 s in Wasm; loop 0.33 s and 0.85 s; Python 0.4 s and 0.18 s; Rust 0.013 s and 0.003 s. Profile with macOS `sample` on the native interpreter. The baseline profile of fib: most time in per-call name hashing (SipHash on the function name), `rsplit_once` on the name for its module, cloning parameter names into the locals, string compares on lookup and on operators, and allocating argument vectors. P3 removes all of these.

## Tasks

- [x] **P3a. IR types.** `lang/src/ir.rs`: a `Module` of functions, structs, enums and capabilities addressed by index; locals as slots with names kept for errors; operators, built-in methods and mutating methods as enums; every expression carrying its type. Reviewed for both consumers (interpreter and Wasm codegen); review fixes applied: mutation is an expression (`rows.push(r)` is a match arm), struct literals keep source order (evaluation order), index places carry the `xs[i]` span, `+=` names its operator so `Str` and `Int` differ, `main`'s parameter names survive for grant errors, capability error enums come first with fixed ids.
- [x] **P3b. Lowering.** `lang/src/lower.rs`: `pub fn lower(p: &Program, caps: &CapTable) -> ir::Module`, from a checked program to IR, for the whole language. It may assume the program checked: an unexpected shape is `unreachable!`, never a diagnostic. It reuses the checker's resolution (keys, `builtin`, `method`) rather than re-deriving it; where the checker's types are not kept per expression, lowering recomputes them with the checker's own functions, or the checker is extended to record them, whichever is smaller (say which). Slots: one per `let`, parameter, `for` variable and pattern binding; shadowing gets a new slot. No evaluation change: `eval.rs` is untouched.
  Done: every test program in lang/tests and every example under examples/ lowers without panicking (a test walks them); snapshot tests for about ten small programs (the `Debug` output of the lowered function, checked in as expected strings) covering locals and shadowing, calls across modules, each `BinOp`, `+=` on `Str` and `Int`, a mutation inside a match arm, struct literals out of declaration order, nested places `a.b[i].c = v`, `?`, `match` on a user enum, `Option` and a literal, `for` over a list and a range, `loop`, and capability calls; a test that every `Expr::ty` equals the checker's type on a sample of programs.
  Result: the checker records each expression's type by span (`Program::types`) and lowering reads it, re-deriving nothing; 385 test programs and every example lower, with types consistent across calls, fields, variants and places.
- [x] **P3c. Interpreter on the IR.** `lang/src/eval.rs` runs `ir::Module` (from `lower`) instead of the AST; `lang::run` keeps its signature (it lowers, then runs) so `guest` and `host` are unchanged. Values: `Value::Struct` and `Value::Enum` carry `StructId` and `EnumRef` instead of names; `Program::display` and the guest's host-value conversion adapt (display needs the `Module`, or the names it holds). Locals: one `Vec<Value>` frame per call, indexed by `LocalId`, so no name is compared; a frame is sized from `Func::locals`. Calls by `FuncId`; operators and methods by their enums. Depth: decision 6, a call-depth counter (`MAX_CALLS`, chosen so a release guest stays inside `--stack`: measure, as G4 did) with "evaluation is too deep" at the call's span; the expression counter, `walk`'s emulation and the `chain` budget go. Runtime errors keep their messages and spans. `fits` (host values) moves to IR types.
  Done: every existing test passes; `evaluation_depth` asserts the call limit instead of the expression budget (the 58-deep `-` case is no longer special) and language.md, open_questions #1 and design.md state the new rule; the benchmark set re-measured and recorded here, natively and in Wasm, with a profile of fib after the change.
  Result: fib(32) 2.7x faster natively and 3x in Wasm, with no name compared or hashed at run time; `MAX_CALLS` is 1_000. `check` lowers once and keeps the `Module` in `Program`, so `run` and `display` use it and a `--listen` snapshot holds it. All locals of all calls live in one `Vec<Value>`, each frame at a base offset and sized from `Func::locals`; arguments are evaluated straight into the callee's frame. A block's `let` slots are reset to `()` when it ends, so a dropped local does not keep a list shared. Places are read in place where the old `walk` did, with evaluated steps kept on one reused `Vec<i64>`.
  Depth, release guest on the default 2 MB `--stack` with no limit: 3_271 plain recursive calls fit, 1_894 through `?`, 1_357 through `match`, 1_141 through loops, 205 under the parser's maximum nesting (58 `-` per call), so 1_000 calls end in "evaluation is too deep" for every common shape and the pathological one traps as "stack overflow", as decision 6 allows. Natively at 1_000 calls: up to 2 MB in release and 9 MB in debug (test threads have 16 MB); before smaller helpers, debug needed 16 MB.
  Benchmarks (release, wall time, best of 3; before is HEAD d68331d):

  | | before | after |
  |---|---|---|
  | fib(32) native | 1.17 s | 0.44 s |
  | fib(32) Wasm (`kio run`) | 2.77 s | 0.93 s |
  | loop native | 0.34 s | 0.22 s |
  | loop Wasm | 0.91 s | 0.60 s |
  | JSON parse 100 KB, Wasm (whole `kio run`) | 0.59 s | 0.16 s |
  | serve `Server::handle` median (3,000 requests) | 164 µs | 164 µs |

  Profile of native fib after (`sample`, top of stack): `Interp::expr` 37%, `binary` 21%, `Value` drop 18%, `call` 6%, `block` 6%, `Value::clone` 6%, `Vec::resize` (the frame) 3%, `returned` 2%. Left: each read clones a `Value` and drops it after; `Stop` is boxed per `return`. `Server::handle` is unchanged because instantiation dominates it.
- [ ] **C0. Design review.** [codegen.md](codegen.md): one core module (runtime plus program), a `compiled` world with a `trap` import, value layout with type descriptors, ownership rules, error sites, testing. Revised after an adversarial review (8 must-fix: owned temporaries, early-exit leaks, double frees, descriptors instead of a scalar/pointer kind, iterative free, dynamic error messages and Wasm `rem_s`, the WIT world, linking). Waiting on the owner: decides 1-3.
- [ ] **C1. Runtime.** Allocator, refcounts, `Str`, `Bytes`, `List`, `Map` operations; unit-tested natively and on wasm32.
- [ ] **C2. Codegen: Int, Bool, control flow, functions.** `fib` and `loop` compile and match the interpreter; differential test harness lands here.
- [ ] **C3. Codegen: Str, Bytes, List, Map, structs, enums, `match`.**
- [ ] **C4. Codegen: capabilities, host calls, runtime errors with spans, depth limit.**
- [ ] **C5. Integration.** `kio run` and `--listen` run compiled code (the `--listen` snapshot compiles once); the interpreter remains behind a flag for comparison.
- [ ] **C6. Program generator** for differential testing.
- [ ] **C7. Adversarial review** of runtime and codegen.
- [ ] **C8. Measure** against Rust and Python (roadmap table); update docs/perf.md and docs/roadmap.md.
