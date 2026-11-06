# Codegen design (C0)

How a checked kio program becomes Wasm. Approved (2026-11-06): refcounted values copied on write, a runtime library in Rust, Wasm emitted straight from the IR, the host checks and compiles, recursion bounded by `--stack`. **decide** marks what the maintainer chooses. Revised after an adversarial review.

## Pipeline: one core module

```
build:  runtime/ (Rust, no_std + alloc, wit-bindgen glue, cabi_realloc) ─► runtime.wasm (core module)
host:   source ─► check ─► lower ─► ir::Module ─► codegen: parse runtime.wasm, append the
        program's functions, data and type descriptors (no new imports, so no index shifts)
        ─► wit-component wraps the one module ─► precompile ─► InstancePre ─► run
```

Runtime and program share one linear memory and one heap, and Cranelift sees them as one module. Component composition (wasm-compose, wac) is not used: components share no memory. Wizer is no longer needed; static data are data segments.

Entry: the runtime exports `run` and calls the program's `main` through a funcref table slot that codegen fills, so the runtime is built without knowing the program.

**decide 1: compile cost per `kio run`.** Each run compiles runtime plus program with Cranelift (tens of ms). Recommend: Wasmtime's compilation cache keyed by module hash, so an unchanged program is not recompiled.

## Interface (caps.wit)

**decide 2: a `compiled` world beside today's `guest` world.** The interpreter's world stays for `--interp` and differential tests.

```wit
world compiled {
  import kio:caps/fs; import kio:caps/net; import kio:caps/web;   // unchanged
  import trap: func(site: u32, a: s64, b: s64);                   // a runtime error; never returns
  export run: func(grants: list<grant>) -> result<outcome, _>;    // grants in main's parameter order
}
```

- The host checks grants (missing, duplicate, unknown) before running, from `main`'s parameters. The report is host-side.
- `trap` is a new import: one more host function on the sandbox wall, so it gets a security review (C7). It records `(site, a, b)` and traps; the host formats the message.
- `outcome` is the display string and a failed flag, formatted in the runtime from the descriptor table, matching `Program::display` exactly.
- The glue now in guest/src/lib.rs moves into the runtime: error enums by `CapTable` order, `send` with a status that is not a u16 is `Err(WebErr::Invalid)`.

## Values

| kio type | Wasm value | Heap object |
|---|---|---|
| `Int` | `i64` | none |
| `Bool` | `i32` | none |
| `()` | nothing; 0 in a slot | none |
| `Str` | `i32` pointer | `[rc u32][desc u32][len u32][cap u32][bytes]` |
| `Bytes` | `i32` pointer | as `Str`; `cap` lets `push` amortize |
| `List<T>` | `i32` pointer | `[rc][desc]` + the runtime's `Vec<u64>` |
| `Map<K, V>` | `i32` pointer | `[rc][desc]` + the runtime's `BTreeMap` (Int keys, or Str keys ordered by bytes) |
| struct, enum, `Option`, `Result` | `i32` pointer | `[rc][desc][variant][payload]`, 8-byte slots |
| capability | `i32` handle | none; never counted |

- **Containers are Rust's own** (`alloc` in `no_std`): `Vec` and `BTreeMap` give the interpreter's exact semantics and costs, including key order.
- **Headers are 8 bytes**, so `i64` slots align.
- **Type descriptors.** Codegen emits a table: per struct and per (enum, variant), which slots are pointers and their descriptors; a container's element, key and value descriptors; names for display. Objects carry a descriptor index, so `rt_dec`, `rt_unique`, `rt_eq` and display need no type argument.
- **Static objects** (string literals, interned per module; payload-free variants like `None`): refcount `u32::MAX`, pinned. `rt_inc`/`rt_dec` skip them; `rt_unique` treats them as shared and copies.

**decide 3: no unboxing in v1.** `Option<Int>` and small structs stay on the heap. Recommend: correctness first; C8 measures.

## Ownership

Every expression yields an **owned** reference (+1).

| Position | Rule |
|---|---|
| Reading a local | `rt_inc` (a copy) |
| A fresh result (literal object, concat, call, builtin) | already +1, no inc |
| Consuming: `let`, assignment, argument, field or element initializer, `return`, `?`'s early exit | takes ownership, no inc |
| Non-consuming: operands of `==`, `+` on `Str`; builtin receiver and arguments; index or field base; match scrutinee after binding; the `for` iterable at loop end | `rt_dec` after use |
| Assignment `x = e` | evaluate `e`, then `rt_dec` the old value, then store |
| A local's scope ends | `rt_dec`, then set the slot to 0; `rt_dec(0)` is a no-op |
| A place write (`xs[i] = v`, `push`, `insert`) | indexes, then the value, then for each container along the place: `rt_unique`, store the result back into its parent, bounds-check, descend |
| `for x in xs` | the loop owns a reference to `xs` in a hidden local, released on every exit; that count is what makes `xs.push` in the body copy |

**Early exits** (`return`, `break`, `continue`, `?` in expression position): before evaluating a subexpression that can exit, owned temporaries are spilled to hidden locals; every exit decs the hidden locals and the locals of each scope it leaves. A trap needs no cleanup: the instance is discarded.

Freeing, equality and display are **iterative** (a worklist), so a value nested 10^6 deep cannot overflow the runtime's stack. `rt_inc`/`rt_dec` fast paths are inlined (load, pinned check, add, store); only a free calls the runtime.

Later (after C8): Perceus-style elision of inc/dec pairs at last uses, and borrowed parameters; neither may elide the `for` loop's reference.

## Runtime errors

Every error site has a number; the host keeps `site -> (span, message template)` beside the compiled module. Generated code calls `trap(site, a, b)` with run-time values, so `index {i} out of bounds for length {len}` and `byte {n} out of range 0..=255` match the interpreter exactly.

Integer checks emitted explicitly, in the interpreter's order: `/` and `%`: divisor 0 is "division by zero", then `MIN / -1` and `MIN % -1` are "Int overflow" (Wasm's `rem_s` gives 0 and `div_s` traps without a site). `-MIN`, `+`, `-`, `*` check overflow. `+=` and `-=` report the statement's span.

The runtime is built `panic = "abort"` with a panic handler that calls `trap` with an internal-error site, so a runtime bug never looks like a program error. "Host returned a malformed value" becomes unreachable: the canonical ABI enforces types.

## Host calls

The runtime owns `cabi_realloc` and reserves an object header in each allocation, so a returned `string` or `list<u8>` becomes a `Str` or `Bytes` in place, and arguments pass as pointer and length with no copy. Codegen calls runtime wrappers generated by capgen (`rt_cap_dir_read(handle, path) -> ptr`), never raw canonical imports.

## Limits

- **Fuel:** Wasmtime fuel. Compiled code spends less per operation; C8 resets defaults. Differential tests compare only runs within both limits.
- **Stack:** `--stack` (decision 6). The runtime's own Rust stack holds only runtime frames, so the guest's 8 MB link setting shrinks; linked `--stack-first`, so an overflow faults rather than corrupts.
- **Memory:** `memory.grow` past `--memory` is refused; the allocator calls `trap` with out of memory.

## Testing

```
every program ─┬─► interpreter ─► value, error and span, host calls ─┐
               └─► compiled    ─► value, error and span, host calls ─┴─► equal; live objects 0 unless trapped
```

The runtime is tested natively first (`cargo test`, Miri). Test builds count live objects and poison freed memory without reusing it. From C2 the differential harness covers lang/tests programs and examples; from C6, generated programs.

## Work order

C1 runtime ─► C2 Int, Bool, control, calls, the harness ─► C3 heap values ─► C4 caps, errors, display ─► C5 integration (`--interp` keeps the interpreter) ─► C6 generator ─► C7 review, including `trap` ─► C8 measure.

New crate `codegen/` on `wasm-encoder` and `wasmparser`; `lang` stays dependency-free.
