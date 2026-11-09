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

How it is built (C2a, proven on `40 + 2`):

1. **runtime.wasm is complete.** `runtime` is built for `wasm32-unknown-unknown` (codegen/build.rs) with `wit_bindgen::generate!` for the `compiled` world: the `run` export, the `trap` import, `cabi_realloc` and the `component-type` custom section are in it. No WASI imports. wit-component's `ComponentEncoder` wraps the final module unchanged.
2. **Entry: placeholder bodies, not a table slot.** The runtime exports `kio_main() -> i64` (main's value as a slot) and `kio_main_desc() -> i32` (its descriptor), whose bodies call through a `read_volatile` pointer so LLVM keeps them opaque. Codegen finds them by export name and replaces their bodies: `kio_main` calls `rt_init(table)`, then main. Simpler than a funcref slot, and a direct call.
3. **Appending without shifting.** `wasm_encoder::reencode::Reencode` re-emits the runtime, overriding only the type, function, memory, code and data sections (and the data count) to add after what exists. Imports never change: generated code calls the `rt_trap` export, which keeps the `trap` import in runtime.wasm, so an import is never added. Runtime bodies are copied as raw bytes.
4. **Static data after the heap's start chunk.** The runtime links stack first (1 MiB), then its data; dlmalloc's first chunk is `__heap_base..__heap_end`, which ends at the initial memory size. Codegen puts the program's data (literals and the descriptor table) at that old end and raises the memory's minimum by the pages it needs; dlmalloc grows memory above them, so the heap never overlaps them.
5. **Static data layout (C3a, codegen/src/data.rs).** One data segment: first each distinct string literal once, as a pinned `Str` (`[u32::MAX][3][len][len][bytes]`, padded to 8); then the descriptor table, a `RawTable` (runtime/src/desc.rs) followed by its five arrays, whose address `kio_main` passes to `rt_init`. The table holds a descriptor for every type in the module (locals, returns, every expression's type), each generic instance its own; `Statics::desc(&Ty)` and `Statics::str` return the index or address, adding one if new.
6. **Runtime operations (C3a, runtime/src/abi.rs):** `rt_<op>` per `Builtin` and `Mut`, plus `rt_eq(desc, a, b)`, `rt_concat`, `rt_list_new(desc, cap)` (then `rt_list_push` per element), `rt_map_new`, `rt_list_get`/`rt_bytes_get`/`rt_list_set`/`rt_list_at` (a slot's address, C3c) for indexes. Pointers are `i32`, `Int` and slot values `i64`, `Bool` results `i32`. A result whose descriptor the runtime cannot name (a `List<Str>` from `split`, an `Option<V>` from `get`) takes it as the first argument. Ownership: non-mutating operations borrow every argument and return +1; mutating ones change the unique place in place and consume the value argument. **Errors:** a fallible operation takes the error site as its last argument and calls `rt_trap(site, a, b)` itself, with `a`, `b` the message's values (index and length; the byte; 0, 0 otherwise); it never returns an error, so codegen emits no check.
7. **Leak check.** The runtime always counts live objects (one add per allocation and free; pinned statics are never counted). `run` traps at site `u32::MAX - 1` with the count if any remain after it displays and frees main's value; `Compiled::run` reports "compiled program leaked N objects", and the differential harness counts and fails on it.
8. **Ownership in `FuncGen` (C3b).** `owned` lists the Wasm locals that hold an owned pointer at the current point of the code: parameters, then each `let` once emitted, then hidden locals of operands evaluated so far. A block records `owned.len()` on entry and at its end decs and zeroes what was added; a loop records it too, and `break`/`continue` dec everything above it; `return` decs all of `owned`. Operands of a call, a runtime operation or `==` are each evaluated into a new hidden local and pushed on `owned` if pointers, so an exit inside a later operand releases them; then they are pushed from those locals (consumed by a call; borrowed by the runtime, then decd) and popped from `owned`. `x = e` evaluates `e`, decs `x`, stores; `x += e` on `Str` concatenates borrowing both, then decs `e` and the old `x`. An expression statement's pointer is decd. `run` decs main's value after displaying it.
9. **Lists, maps and places (C3c).** Elements are `i64` slots, converted on the way in and out (`slot_in`, `slot_out`). A list literal is `rt_list_new(desc, n)` held in a hidden local on `owned` while its elements run, then one `rt_list_push` per element. A place write (`xs[i][j] = v`, `+=`, `push`, `insert`, `remove`, `b.push` on any place) evaluates the indexes and arguments as operands, then `walk`s: `rt_unique` on the root local and store back; then per index step `rt_list_at(list, i, site)` (bounds check; returns the slot's address), load the element, `rt_unique`, store it back into the slot, descend. `=`, `+=` and `-=` load and store the last slot through `rt_list_at` (`=` used `rt_list_set` until C3d); `b.push` stores `rt_bytes_push`'s result back into the slot or the root. `for x in xs`: the list in a hidden local on `owned` below the loop's mark (so `break` and `continue` keep it, `return` drops it, the loop's end decs it); its length read once; each pass `rt_list_get` into `x`, decd and zeroed at the pass's end; after a `break`, the scope end decs that pass's `x`.
10. **Structs, enums, `match`, `?` (C3d).** A struct literal or a variant with a payload is `rt_alloc(desc, variant)` held in a hidden local on `owned` while its parts run in source order, each stored into its slot (byte `8 + 8i` in a struct, `16 + 8i` in a variant), which takes it. A payload-free variant (`None`, `E::A`) is a pinned object `[u32::MAX][desc][variant][0]` in the literal area, one per (descriptor, variant) used (`Statics::pinned`). `e.f` loads the slot and incs it; a base that is a local or a field of one is read in place (no inc/dec of the base, as the interpreter's `in_place`), any other base is evaluated and decd after. `match`: the scrutinee in a hidden local on `owned` (so every exit from an arm releases it; the match's end decs it); each arm is a Wasm `block` that the pattern's tests leave with `br_if` on failure: a variant's number (`i32.load` at 8) before its payload's parts, left to right, reloading each part from the scrutinee along its slot path; `Int`/`Bool` by compare, `Str` by `rt_eq` with the literal. Bindings are `rt_inc`'d copies of their parts, on `owned` like `let`s, released and zeroed at the arm's end; the arm's value leaves through `br` to the match's `block`. `x?`: `x` in a hidden local on `owned`; if its variant is `None`/`Err`, the return value is built (the return type's pinned `None`; `x` itself, inc'd, when its type is the return type; else a new `Err` of the return type's descriptor holding an inc'd copy of the error), then `release(0)` and `return`; otherwise the payload is loaded and inc'd and `x` decd. A place step `.f` is the slot at `cur + 8 + 8f`: made unique and stored back like an element; the last step's slot, field or element, takes `=` by store-then-dec-old (`rt_list_set` is no longer used). `main` may return a struct, an enum or an `Option` (`kio_main_desc` is its descriptor); a `Result` waits for C3e.
11. **`main`'s value, `Option` results, `sort` and `pop` (C3e, C3f).** `main` returns any type: `kio_main_desc` is its descriptor and the runtime's `run` displays it through `display::outcome`, which shows a `Result` that is `Err` as its payload alone with `failed` set, as the interpreter path does. A runtime operation whose result is an `Option` (`parse_int`, `m.get`, `strip_prefix`/`strip_suffix`, `find`, `ok`, `err`, `pop`) takes the result's descriptor and its pinned `None` first (`obj::option`), so a `None` result allocates nothing. `contains`, `ok_or`, `unwrap_or` and `m.get` take their argument as a slot. `sort` and `pop` are place mutations: `walk` makes the path unique, then `rt_list_sort(list)` or `rt_list_pop(desc, none, list)`, whose `Some` takes the popped element's reference (no inc), and the `Option` is the expression's value.
12. **`Float` and slices (C10, C11).** A `Float` is an `f64` in Wasm locals, parameters and results, and its bits in a slot (`i64.reinterpret_f64` in, `f64.reinterpret_i64` out; `kio_main` returns the bits). Descriptor 5 is `Float`, after `Bytes`, so each scalar's index is its tag (`List` to `Enum` are tags 6 to 9). Every operator is its Wasm instruction except `%`, which is `rt_float_rem` (Rust's `%`: Wasm has no float remainder). `==` is `f64.eq` on a scalar; `rt_eq` compares `Float` slots as floats, and compares an object with itself part by part (a value holding a NaN is not equal to itself, as in the interpreter). `to_float`, `abs`, `floor`, `ceil`, `sqrt` and `is_nan` (`x != x`), `is_finite` (`|x| < inf`), `is_infinite` (`|x| == inf`) are inline. `round` is `rt_float_round` (on the bits: `core` has no `f64::round`, and Wasm's `f64.nearest` rounds halves to even); `to_str` is `rt_float_to_str` (`core`'s `{}`, as the interpreter's `to_string`); `to_int` and `parse_float` return an `Option` as in 11. `slice` is `rt_str_slice(s, a, b, site, boundary)` or `rt_bytes_slice(v, a, b, site)`. Its out-of-bounds message needs three values, `slice {a}..{b} out of bounds for length {c}`: the runtime's `trap3` first calls `trap` at site `EXTRA` (`u32::MAX - 2`) with the length, which the host keeps for the next message's `{c}` and returns from, then traps at the call's site.

13. **Capabilities (C4).** The host binds grants to `main`'s parameters (host/src/grants.rs, shared with the interpreter path) and passes them in parameter order to `run`, which keeps them alive for the run and records each one's kind (`CapTable` order, which is caps.wit's `grant` order) and component handle. `kio_main` calls `rt_grant(i, kind)` per parameter (an internal error on a mismatch) and passes the handles to `main`; a `Cap` local is that `i32` handle, never counted. `CapCall` calls `rt_cap_<cap>_<method>(result desc, handle, args..)` (runtime/src/caps.rs, hand-written; codegen's `every_capability_method_compiles` covers all 24), which borrows its arguments and returns +1. A `Str`/`Bytes` argument goes to the host as a pointer into its object; `List<Str>` (only `send`, imported directly rather than through wit-bindgen) as a list of pointers into its `Str`s. The runtime defines `cabi_realloc` (replacing wit-bindgen's weak one): during a capability call a byte-aligned allocation (a `string` or `list<u8>` the host lowers) reserves 16 bytes in front, and the wrapper turns it into a `Str`/`Bytes` in place (`obj::adopt`). Errors are built by variant name from the descriptor table; a `send` status or `close` code that is not a `u16` is `Invalid` without a host call. A handle that is not live, or not of the method's resource, traps in the canonical ABI before the host runs.

**decide 1: compile cost per `kio run`.** Each run compiles runtime plus program with Cranelift (tens of ms). Recommend: Wasmtime's compilation cache keyed by module hash, so an unchanged program is not recompiled.

## Interface (caps.wit)

**decide 2: a `compiled` world beside today's `guest` world.** The interpreter's world stays for `--interp` and differential tests.

```wit
world compiled {
  import kio:caps/fs; import kio:caps/net; import kio:caps/web;   // unchanged
  import trap: func(site: u32, a: s64, b: s64);                   // a runtime error; never returns
  export run: func(grants: list<grant>) -> result<outcome>;       // grants in main's parameter order
}
```

- The host checks grants (missing, duplicate, unknown) before running, from `main`'s parameters. The report is host-side.
- `trap` is a new import: one more host function on the sandbox wall, so it gets a security review (C7). It records `(site, a, b)` and traps; the host formats the message. A call at the runtime's `EXTRA` site returns instead, keeping `a` for the next message ("How it is built" 12).
- `outcome` is the display string and a failed flag, formatted in the runtime from the descriptor table, matching `Program::display` exactly.
- The glue now in guest/src/lib.rs moves into the runtime: error enums by `CapTable` order, `send` with a status that is not a u16 is `Err(WebErr::Invalid)`.

## Values

| kio type | Wasm value | Heap object |
|---|---|---|
| `Int` | `i64` | none |
| `Bool` | `i32` | none |
| `Float` | `f64`; its bits in a slot | none |
| `()` | nothing; 0 in a slot | none |
| `Str` | `i32` pointer | `[rc u32][desc u32][len u32][cap u32][bytes]` |
| `Bytes` | `i32` pointer | as `Str`; `cap` lets `push` amortize |
| `List<T>` | `i32` pointer | `[rc][desc]` + the runtime's `Vec<u64>` |
| `Map<K, V>` | `i32` pointer | `[rc][desc]` + the runtime's `BTreeMap` (Int keys, or Str keys ordered by bytes) |
| struct, enum, `Option`, `Result` | `i32` pointer | `[rc][desc][variant][payload]`, 8-byte slots |
| capability | `i32` handle | none; never counted |

- **Containers are Rust's own** (`alloc` in `no_std`): `Vec` and `BTreeMap` give the interpreter's exact semantics and costs, including key order.
- **Headers are 8 bytes**, so `i64` slots align.
- **Type descriptors.** Codegen emits a table: per struct and per (enum, variant), which slots are pointers and their descriptors; a container's element, key and value descriptors; names for display (a struct's or enum's module-qualified key, record field names in a `fields` array parallel to `slots`, bare variant names; `Option` and `Result` print variants alone). Objects carry a descriptor index, so `rt_dec`, `rt_unique`, `rt_eq` and display need no type argument.
- **Static objects** (string literals, interned per module; payload-free variants like `None`): refcount `u32::MAX`, pinned. `rt_inc`/`rt_dec` skip them; `rt_unique` treats them as shared and copies.

**decide 3: no unboxing in v1.** `Option<Int>` and small structs stay on the heap. Recommend: correctness first; C8 measures.

## Ownership

Every expression yields an **owned** reference (+1).

| Position | Rule |
|---|---|
| Reading a local | `rt_inc` (a copy) |
| A fresh result (literal object, concat, call, builtin) | already +1, no inc |
| Consuming: `let`, assignment, argument, field or element initializer, `return`, `?`'s early exit | takes ownership, no inc |
| Non-consuming: operands of `==`, `+` on `Str`; builtin receiver and arguments; index or field base; the match scrutinee at the match's end; the `for` iterable at loop end | `rt_dec` after use |
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

How it is built (C2b): `codegen::Site { span, template }`, one per check; the host's `trap` checks the site is in the table, then replaces `{a}` and `{b}` in the template with `trap`'s arguments. Checks are inline, no runtime call: `+` overflows when `(a ^ r) & (b ^ r) < 0`, `-` when `(a ^ b) & (a ^ r) < 0`; `*` when `a != 0` and `r / a != b`, with `a = -1, b = MIN` tested first (that division would trap). Locals: each IR local with a Wasm value type gets a Wasm local, parameters first; `()` locals get none; hidden locals follow. A limit trap (stack overflow, fuel, timeout) from `Compiled::run` carries `Kio::explain`'s message. Function types are deduplicated.

The runtime is built `panic = "abort"` with a panic handler that calls `trap` with an internal-error site, so a runtime bug never looks like a program error. "Host returned a malformed value" becomes unreachable: the canonical ABI enforces types.

## Host calls

The runtime owns `cabi_realloc` and reserves an object header in each allocation, so a returned `string` or `list<u8>` becomes a `Str` or `Bytes` in place, and arguments pass as pointer and length with no copy. Codegen calls runtime wrappers (`rt_cap_dir_read(desc, handle, path) -> ptr`), never raw canonical imports; they are hand-written (item 12).

## Limits

- **Fuel:** Wasmtime fuel. Compiled code spends less per operation; C8 resets defaults. Differential tests compare only runs within both limits.
- **Stack:** `--stack` (decision 6). The runtime's own Rust stack holds only runtime frames, so the guest's 8 MB link setting shrinks; linked `--stack-first`, so an overflow faults rather than corrupts.
- **Memory:** `memory.grow` past `--memory` is refused; the allocator calls `trap` with out of memory.

## Testing

```
every program ─┬─► interpreter ─► value, error and span, host calls ─┐
               └─► compiled    ─► value, error and span, host calls ─┴─► equal; live objects 0 unless trapped
```

The runtime is tested natively first (`cargo test`, Miri). Every build counts live objects; test builds also poison freed memory without reusing it. From C2 the differential harness covers lang/tests programs and examples; from C6, generated programs.

## Work order

C1 runtime ─► C2 Int, Bool, control, calls, the harness ─► C3 heap values ─► C4 caps, errors, display ─► C5 integration (`--interp` keeps the interpreter) ─► C6 generator ─► C7 review, including `trap` ─► C8 measure.

New crate `codegen/` on `wasm-encoder` and `wasmparser`; `lang` stays dependency-free.
