# Proposal: function values, generics, floats (needs the owner)

The largest language gaps (roadmap.md "What a real app needs"). Each changes what programs can rely on, so each is a one-way door. Recommendations first.

## 1. Function values

```rust
fn double(x: Int) -> Int { x * 2 }
fn apply(f: fn(Int) -> Int, xs: List<Int>) -> List<Int> { .. f(x) .. }
apply(double, xs);                         // a function item as a value
apply(|x| x + n, xs);                      // a closure: captures `n` by copy
```

| Option | Gives | Cost |
|---|---|---|
| **A. fn items and closures, no capabilities inside (recommend)** | route tables, middleware, `map`/`filter`/`and_then`, sort by key | a `fn(..) -> T` type; closures capture by copy (values are copied anyway, so no lifetimes) |
| B. A plus capability parameters (`fn(Request, Dir) -> Response`) | route tables over capabilities | the report becomes "any function of this type that is used as a value": still sound, less exact |
| C. fn items only | the simplest | no closures, so `map` needs a helper per use |

A keeps rule 1 and the report exact: a function value cannot take, capture or return a capability, so calling one never adds authority. B can follow later without breaking A's programs.

Compiled: a closure is a heap object (a descriptor for its captures, plus a function index), called with `call_indirect`.

## 2. Generics

```rust
fn first<T>(xs: List<T>) -> Option<T> { if xs.len() == 0 { None } else { Some(xs[0]) } }
struct Page<T> { items: List<T>, next: Option<Int> }
fn dedup<T: Eq>(xs: List<T>) -> List<T> { .. }
```

| Option | Gives | Cost |
|---|---|---|
| **A. Type parameters on fns, structs and enums, monomorphized; bounds only `Eq` and `Ord` (built in) (recommend)** | containers and helpers written once; typed JSON decode later | the checker substitutes types; lowering makes one copy per instance (codegen already has a descriptor per instance) |
| B. A plus user traits | `impl Display for T`, user-defined bounds | traits, impls, coherence: a much larger language |

A covers what kio-web and json.kio need. Traits wait until a program needs them.

## 3. Floats

`Float` (f64): literals `1.5`, `1e9`; `+ - * /` with IEEE results (no trap on overflow or NaN, as Rust); `< <=` etc.; `==` is IEEE (`NaN != NaN`), so a struct holding a `Float` compares as Rust's derived `PartialEq` does; not a `Map` key; `to_str` prints as Rust's `{}`; `Int.to_float()`, `Float.to_int() -> Option<Int>` (None if not finite or out of range), `Str.parse_float() -> Option<Float>`. Recommend: add it. JSON numbers and most numeric work need it.

## Order, if approved

1A function values (also gives `Option::map`/`and_then` as built-ins taking a function), then 3 floats, then 2A generics. Each is pinned in lang-todo.md and goes through the interpreter first, then the compiler.
