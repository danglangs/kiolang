# kio primer

kio uses a subset of Rust syntax, so every kio program parses as Rust. Types and semantics are kio's own: there are no references or ownership, and values are copied. The Rust compiler is never involved. kio's checker and compiler read the source.

## One screen of kio

```
struct Row { name: Str, amount: Int }

fn parse_row(line: Str) -> Option<Row> {
  let parts = line.split(",");
  if parts.len() != 2 { return None; }
  Some(Row { name: parts[0], amount: parts[1].parse_int()? })
}

fn total(rows: List<Row>) -> Int {
  let mut sum = 0;
  for r in rows { sum = sum + r.amount; }
  sum
}

fn load(d: Dir, name: Str) -> Result<List<Row>, IoErr> {
  let text = d.read(name)?.to_str();
  let mut rows: List<Row> = [];
  for line in text.lines() {
    let Some(r) = parse_row(line) else { continue; };
    rows.push(r);
  }
  Ok(rows)
}

fn main(input: Dir, out: Dir) -> Result<(), IoErr> {
  let rows = load(input, "invoices.csv")?;
  out.create("total.txt", total(rows).to_str().to_bytes())
}
```

## The language

| Area | Contents |
|---|---|
| Scalars | `Int` (i64), `Float` (f64), `Bool`, `Str`, `Bytes`, `()` |
| Containers | `List<T>`, `Map<K, V>` (key `Int` or `Str`, in key order), `HashMap<K, V>` (the same keys, in insertion order) |
| Errors | `Option<T>`, `Result<T, E>`, `?` |
| JSON | `Json`, built in: `enum Json { Null, Bool(Bool), Num(Int), Float(Float), Str(Str), Arr(List<Json>), Obj(Map<Str, Json>) }` |
| User types | `struct`, and `enum` with payloads, generic or not (`struct Page<T>`); a type may contain itself only through `List`, `Map` or `HashMap`; struct update `P { x: 1, ..p }` |
| Constants | `const MAX: Int = 64;` at the top level: an `Int`, `Float`, `Bool` or `Str` literal |
| Control | `if`, `if let`, `let`-else, `while`, `loop`, `for x in list`, `for i in 0..n`, `for b in bytes` (each byte an `Int` 0..=255), `match`, `break`, `continue`, `return` |
| Functions | free functions, recursion, generic functions `fn f<T>(..)`, function values of type `fn(T, ..) -> R`, closures |
| Capabilities | `Dir`, `Http`, `Kv`, `Incoming`, `Outgoing`, `WsMessage`, `WsSend`, `Rng`, `Clock`, `Log`, `Db`, generated from `caps.wit` |

Everything else is rejected with a message that names the kio alternative, for example `&str` (use `Str`), `Vec` (use `List`), `format!` (use `+` and `to_str()`), `if let .. && ..` (nest an `if`), `impl`, traits, bounds other than `Eq` and `Ord` (`T: Clone`), `use`, nested modules, tuples (`let (a, b) = ..`: a struct or one `let` per value; `for (k, v) in m`: loop over `m.keys()` and `get`), globals, `unsafe`.

## Semantics

- **Values are copied.** Assignment and argument passing never share state. Only `let mut` locals change: `x = v`, `x += v`, `x -= v`, `x.f = v`, `xs[i] = v`, `m.insert(k, v)`, `xs.push(v)`, `m.remove(k)`. A function changes its caller's data only by returning a new value. `for x in xs` (a `List` or `Bytes`) iterates over `xs` as it was when the loop started.
- **`loop { .. }`** repeats until `break`; `continue` and `return` work as in Rust. A `loop` with no `break` never finishes normally, so `fn f() -> Int { loop { return 1; } }` needs no tail. `break` takes no value: use a `let mut` and `break`.
- **Statements end in `;`**, as in Rust. A block's last expression has no `;` and is the block's value. `if`, `while`, `loop`, `for` and `match` used as statements need no `;`. `if` chains with `else if`.
- **Types are explicit where they cannot be read off the value.** `[]`, `Map::new()`, `HashMap::new()` and `None` need a declared or expected type: `let mut rows: List<Row> = [];`.
- **Operators.** `+ - * / %` on `Int` or on `Float`, `+` on `Str`, `== !=` on two values of one type, structurally (structs, enums, `Option`, `Result`, `List`, `Map`, `HashMap` included, a `HashMap`'s regardless of order; capabilities have no equality), `< <= > >=` on `Int`, `Float` or `Str` (`Str` by its UTF-8 bytes, a prefix first, as Rust's `str` and `sort()`: `"B" < "a"`, not locale-aware), `&& || !` on `Bool`. On `Int`, `/` and `%` truncate toward zero, as in Rust. `Int` and `Float` never mix: `1 + 2.0` is an error; convert with `.to_float()` or `.to_int()`.
- **`Float` is Rust's `f64`.** Literals `1.5`, `2.0`, `1e9`, `1.5e-3` (`1.` and `.5` are errors). Arithmetic gives IEEE results and never stops the run: `1.0 / 0.0` is `inf`, `0.0 / 0.0` is `NaN`. `==` is IEEE: `NaN != NaN` and `0.0 == -0.0`, also inside structs, enums and lists. A `Float` is not a map key and cannot be matched: compare with `<` or `==`.
- **Runtime errors stop the run:** `Int` overflow, `Int` division by zero, an out-of-bounds index, invalid UTF-8 in `to_str`, `Rng.bytes` with a count outside 0 to 65,536 or `Rng.int` with an empty range. Deep recursion stops the run with "stack overflow": `--stack` sets the limit, 2 MB by default (about 40,000 calls of a small function), at most 8 MB. Nothing wraps or panics silently.
- **`match` is exhaustive.** A pattern is `_`, a name (binds the value), an `Int`, `Str` or `Bool` literal, a variant whose parts are patterns, at any depth: `Err(IoErr::Exists)`, `Some(Some(x))`, `Shape::Line(1, _)`, or alternatives `P1 | P2`, anywhere a pattern goes (`"GET" | "HEAD"`, `Some(1 | 2)`, `Ok(x) | Err(x)`, a leading `|` allowed), tried left to right. Every alternative binds the same names with the same types. Arms are tried in order; a name binds at most once per pattern. Every value must match some arm (`Int` and `Str` need a `_` or name at that position) and every arm must match some value no earlier arm does, else "unreachable arm"; a top-level alternative that earlier arms and alternatives already cover is "unreachable alternative". A match too large to check is rejected: split it. An arm may be `return`, `break` or `continue` (`None => break,`); it has type `Never`.
- **`if let PAT = e { .. } else { .. }`** runs the block with `PAT`'s names bound when `e` matches, else the else branch. Without `else` its type is `()`. It chains with `else if` and `else if let`. It need not be exhaustive.
- **`let PAT = e else { .. };`** binds `PAT`'s names for the rest of the block when `e` matches; otherwise it runs the else block, whose last statement must be `return`, `break` or `continue` (a nested `if` that returns on every branch does not count). `PAT` must be refutable ("this pattern always matches": write `let`), its names are not `mut` and not in scope in the else block, and `let PAT: T = e else` takes a type as `let` does. A pattern other than a name in `let` needs the `else`.
- **`Str` is UTF-8.** `len()` counts bytes, in constant time. Literal escapes: `\n \r \t \0 \" \\`.
- **Deterministic.** No ambient clock or randomness: time and random values come only through a `Clock` or `Rng` parameter, which the operator can fix (`--clock now=fixed:..`, `--rng r=seed:1`) to replay a run. `Map` iterates in key order, `HashMap` in insertion order.
- **Comments:** `//` only.
- **Reserved words.** All Rust keywords not used by kio are reserved and cannot be names.
- **Type names start uppercase.** `struct Point`, `enum Status`. A lowercase name before `::` is a module, as in `web::text`.
- **Function values.** A function's name not followed by `(` is a value of type `fn(T, ..) -> R` (`fn(T)` returns `()`), usable wherever a type is: parameters, returns, locals, fields, payloads, `List` and `Option` elements. `f(x)` calls a local or parameter `f` of `fn` type; a local hides a function of the same name, as in Rust. `(e)(x)` calls any expression of `fn` type, as in `(r.handler)(req)`. Functions have no equality and are not `Map` keys; `main` is not a value. One prints as `<fn name>`.

  ```
  struct Route { path: Str, handler: fn(Request) -> Response }
  fn dispatch(routes: List<Route>, req: Request) -> Response {
    for r in routes { if r.path == req.path { return (r.handler)(req); } }
    not_found()
  }
  ```
- **Closures.** `|x: Int| x + n`, `|a, b| { .. }` and `|| e` are values of `fn` type. A parameter without a type takes it from the expected `fn` type (of a parameter, field, annotated `let`, list element or assigned place); with none, write it. The return type is the body's. Every outer local the body uses is copied into the closure when it is created: a later change to the local is not seen, and the closure cannot assign to it ("`n` is captured by copy and cannot be assigned"). `return` returns from the closure; `break` and `continue` cannot leave it. `||` after a value is logical or; where an expression starts it begins a closure with no parameters. A closure has no `-> T` and no `move`. One prints as `<closure>`.
- **At most 999 parameters** per function, and per closure counting the locals it captures (Wasm's limit is 1,000; a function value takes itself too). Past that: "pass a struct instead".

  ```
  fn adder(n: Int) -> fn(Int) -> Int { |x| x + n }
  let ops: List<fn(Int) -> Int> = [adder(1), |x| x * k];
  ```
- **Generic functions.** Type parameters are uppercase names after the function's name. In the body a `T` is opaque: it can be passed, returned, held in `List<T>`, `Option<T>`, `Result` or as a `Map` value, and matched by a name or `_`; it has no methods, operators or literals, and `==` only with a bound (below), so take a function for what the code needs (`add: fn(T, T) -> T`). At a call each type parameter comes from the arguments, where it first appears (a later one must agree: "`T` is Int here but Str at argument 2"), then from the expected type; arguments that need a type (`None`, `[]`, `Map::new()`, a closure) are checked after the others, so `or(None, 5)` works. Otherwise write it: `empty::<Int>()`. A generic function used as a value needs its types fixed by `::<..>` or an expected `fn` type. A type parameter is never a capability (rule 1). Each distinct list of type arguments makes one copy of the function, named `first<Int>` (a value prints `<fn first<Int>>`); recursion that keeps changing them (`f<T>` calling `f<List<T>>`) is refused: "this generic function instantiates without end".

  ```
  fn first<T>(xs: List<T>) -> Option<T> { if xs.len() == 0 { None } else { Some(xs[0]) } }
  fn apply<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
  let a = first(["x"]);                       // T = Str
  let xs: List<Int> = empty();                // T from the expected type
  let n = apply(|x| x + 1, 41);               // T from 41, then U from the closure
  let f: fn(List<Int>) -> Option<Int> = first;
  ```
- **Generic structs and enums.** Type parameters as on functions; a use names every argument (`Page<Int>`, `web::Tree<Str>`). A literal or variant takes its type arguments from its values, then from the expected type, as a call does; `Tree::Leaf` alone needs one: "cannot infer `T` for `Tree::Leaf`". A literal, variant or generic call whose values leave a type parameter open is checked after the other arguments, as `None` is, so `set(Page { items: [], next: None }, true)` and `let b: Box<Box<Option<Int>>> = Box { v: Box { v: None } };` work. There is no `Box::<Int> { .. }`: write the type on the `let`. Fields, field writes and patterns have the instance's types. `==` works on an instance whose arguments allow it (`Page<Int>`, or `Page<T>` where `T: Eq`). A type argument is never a capability. A type may contain itself only through `List`, `Map` or `HashMap`, in every instance (`struct X { w: W<X> }` is refused when `W<T>` holds a `T`). Each instance is its own type, printed without its arguments, as Rust's `Debug` does: `Page { items: [1], next: None }`. A type whose fields keep growing its arguments (`struct S<T> { s: List<S<List<T>>> }`) is refused once used: "generic type `S` instantiates without end". At most 64 instances of one generic function or type, each type argument at most 256 types. `Option` and `Result` stay built in.

  ```
  struct Page<T> { items: List<T>, next: Option<Int> }
  enum Tree<T> { Leaf, Node(List<Tree<T>>, T) }
  let p = Page { items: [1, 2], next: None };     // Page<Int>
  let t: Tree<Str> = Tree::Leaf;
  fn first<T>(p: Page<T>) -> Option<T> { if p.items.len() == 0 { None } else { Some(p.items[0]) } }
  ```
- **Bounds.** `T: Eq` and `T: Ord` (or `T: Eq + Ord`, the same as `Ord`) on a function's, struct's or enum's type parameter; no other bound exists (`T: Clone`: "only `Eq` and `Ord` bounds exist", kio values are always copied and displayable). In the body, `T: Eq` allows `==`, `!=` and `List<T>.contains`, also on types built from `T` (`List<T>`, `Page<T>`); `T: Ord` adds `< <= > >=` and `List<T>.sort()`. Every type argument must satisfy its bound, at a call (inferred or `::<..>`), a function value, a literal or variant, and a type written anywhere: `Eq` holds for every type `==` accepts (none holding a function); `Ord` only for `Int` and `Str` (not `Float`, IEEE; not structs or enums): "`Point` is not `Ord`: `max` needs `T: Ord`". A bounded type parameter satisfies the same bound where it is passed on (`Ord` also satisfies `Eq`); an unbounded one gets "add the bound: `U: Ord`". `Map<T, V>` and `HashMap<T, V>` still need `Int` or `Str` keys.

  ```
  fn dedup<T: Eq>(xs: List<T>) -> List<T> {
    let mut out: List<T> = [];
    for x in xs { if !out.contains(x) { out.push(x); } }
    out
  }
  fn max<T: Ord>(xs: List<T>) -> Option<T> { .. }   // max([1.5]): `Float` is not `Ord`
  struct Set<T: Ord> { items: List<T> }              // fn f<U>(s: Set<U>): add the bound `U: Ord`
  ```
- **Struct update.** `P { x: 1, ..p }` is a new `P` with the fields written and every other field from `p`, as in Rust: the written fields run in order, then `p` once. The base is a `P` (of a generic struct, the same instance: its type arguments are inferred with the fields'), and comes last; it may be any expression, and every field may be written. `p` itself is unchanged (values are copied).
- **Constants.** `const MAX: Int = 64;` (or `pub const`) at the top level of a file: an `Int`, `Float`, `Bool` or `Str`, whose value is a literal of that type (`-1` and `-0.5` too), not an expression or another constant. A use is replaced by the literal when the program is checked, so there is no global state. `MAX` in its file, `web::MAX` from another (if `pub`). A function, type or other constant of the file cannot have its name; a local may shadow it. A constant cannot be assigned or be a pattern (`x => if x == MAX { .. }` instead).
- **Modules.** `mod web;` in the root file loads `web.kio`; its `pub` items are used as `web::f`, `web::T`, `web::MAX`. Modules are flat: one file each, declared only in the root. A module may use another declared module's `pub` items the same way (cycles are fine) but not the root's.

## Built-ins

| Type | Methods |
|---|---|
| `Bool` | `to_str() -> Str` (`"true"` or `"false"`) |
| `Int` | `to_str() -> Str`, `to_float() -> Float`, `min(Int) -> Int`, `max(Int) -> Int` |
| `Float` | `to_str() -> Str` (as Rust's `{}`: `1`, `0.30000000000000004`, `NaN`, `inf`, `-0`), `to_int() -> Option<Int>` (toward zero; `None` if NaN, infinite or out of range), `abs()`, `floor()`, `ceil()`, `round()` (half away from zero), `sqrt()`, all `-> Float`, `is_nan() -> Bool`, `is_finite() -> Bool`, `is_infinite() -> Bool`, `min(Float) -> Float`, `max(Float) -> Float` (as Rust's: a NaN is ignored unless both are; `-0.0` is below `0.0`) |
| `Str` | `len() -> Int`, `split(Str) -> List<Str>` (an empty separator is a runtime error), `lines() -> List<Str>`, `chars() -> List<Str>` (each Unicode scalar value as a one-character `Str`, as Rust's `chars`), `trim() -> Str`, `contains(Str) -> Bool`, `starts_with(Str) -> Bool`, `ends_with(Str) -> Bool`, `strip_prefix(Str) -> Option<Str>`, `strip_suffix(Str) -> Option<Str>`, `find(Str) -> Option<Int>` (byte offset), `replace(Str, Str) -> Str`, `parse_int() -> Option<Int>`, `parse_float() -> Option<Float>` (as Rust's `f64::from_str`: `1e3`, `inf`, `NaN`), `parse_json() -> Result<Json, Str>` (below), `slice(Int, Int) -> Str` (byte offsets as Rust's `&s[a..b]`; out of range, `a > b`, negative or not on a character boundary is a runtime error), `to_bytes() -> Bytes`, `to_lowercase() -> Str`, `to_uppercase() -> Str` (Rust's Unicode mapping: `"straße"` to `"STRASSE"`), `sha256() -> Bytes` (of its UTF-8 bytes), `ct_eq(Str) -> Bool` (as `Bytes.ct_eq`) |
| `Bytes` | `b[i] -> Int` (0-255), `len() -> Int`, `push(Int)` (0-255), `slice(Int, Int) -> Bytes` (as `Str.slice`, without the boundary rule), `to_str() -> Str`, `extend(Bytes)`, `sha256() -> Bytes` (32 bytes), `to_hex() -> Str` (lowercase), `ct_eq(Bytes) -> Bool` (`==` taking the same time wherever the bytes differ: different lengths are unequal at once, equal lengths compare every byte; for secrets such as tokens and MACs) |
| `Option<T>` | `unwrap_or(T) -> T`, `is_some() -> Bool`, `is_none() -> Bool`, `ok_or(E) -> Result<T, E>`, `map(fn(T) -> U) -> Option<U>`, `and_then(fn(T) -> Option<U>) -> Option<U>`, `unwrap_or_else(fn() -> T) -> T` |
| `Result<T, E>` | `unwrap_or(T) -> T`, `is_ok() -> Bool`, `is_err() -> Bool`, `ok() -> Option<T>`, `err() -> Option<E>`, `map(fn(T) -> U) -> Result<U, E>`, `map_err(fn(E) -> F) -> Result<T, F>`, `and_then(fn(T) -> Result<U, E>) -> Result<U, E>`, `unwrap_or_else(fn(E) -> T) -> T` |
| `List<T>` | `xs[i]`, `len() -> Int`, `push(T)`, `pop() -> Option<T>`, `contains(T) -> Bool`, `sort()` (`List<Int>` and `List<Str>`, bytes order, stable), `map(fn(T) -> U) -> List<U>`, `filter(fn(T) -> Bool) -> List<T>`, `any(fn(T) -> Bool) -> Bool`, `all(fn(T) -> Bool) -> Bool`, `sort_by(fn(T, T) -> Bool)` (`less(a, b)`: `a` goes first; stable), `sort_by_key(fn(T) -> K)` (`K` `Int` or `Str`; stable), `reverse()`, `slice(Int, Int) -> List<T>` (as `Str.slice`), `join(Str) -> Str` (`List<Str>` only) |
| `Map<K, V>` | `Map::new()`, `len() -> Int`, `get(K) -> Option<V>`, `insert(K, V)`, `remove(K)`, `keys() -> List<K>` |
| `HashMap<K, V>` | as `Map`, with `HashMap::new()`; `keys()` in insertion order |
| `Json` | `get(Str) -> Option<Json>` (an `Obj`'s field), `at(Int) -> Option<Json>` (an `Arr`'s element), `as_str() -> Option<Str>`, `as_int() -> Option<Int>`, `as_float() -> Option<Float>` (a `Float`, or a `Num` converted), `as_bool() -> Option<Bool>`, `as_list() -> Option<List<Json>>`, `as_map() -> Option<Map<Str, Json>>`, each `None` for another variant; `to_json() -> Str` |

The methods taking `fn(..)` take any function value: a name (`json::as_str`), a closure or a local. A closure written as the argument gets its parameter type from the receiver and its return type from its body: `xs.filter(|x| x > 0).map(|x| x * 2)`. In one given to `and_then`, `?` returns the closure's `None` or `Err`, and `Ok(x)` takes the receiver's error type; `None` or `Err(e)` alone need a typed closure (`let f: fn(Str) -> Option<Int> = |s| ..`). The receiver is evaluated, then the function, then each call in list order; `any` and `all` stop at the first value that decides, as Rust's; `unwrap_or_else` calls only on `None` or `Err`. `sort_by_key` calls its function once per element, in order, then sorts by the keys. `sort_by` is a merge sort that takes the right run's element only when `less(right, left)`: a `less` that is no order (`<=`, or one that is random) still gives a permutation, the same on every run. `push`, `pop`, `sort`, `sort_by`, `sort_by_key`, `reverse` and `extend` change a `let mut` place (`xs`, `s.items[i]`).

**`HashMap`.** The same keys, methods, copying and rules as `Map` (`m.insert` on a shared map copies it first), and a different type: no conversion either way. Iteration (`keys()`, display) is in insertion order: replacing a value keeps its key's place, a removed key loses it, and inserting it again puts it last. `==` ignores order: the same keys with equal values. The hash is fixed (an `Rng` grant never seeds it), so keys can be chosen to collide; a bucket of more than 8 is kept sorted, so even then each operation takes O(log n). Use `HashMap` for lookups by key (faster), `Map` when key order matters.

**JSON.** `Json` is a built-in enum, like `Option`: no user type may be named `Json`, and its variants are matched and built as any enum's (`Json::Num(1)`). `s.parse_json()` reads RFC 8259 JSON. A number without fraction or exponent that fits `Int` is `Num`, any other `Float`; a number too large for `Float` (`1e999`) is an error. Strings take every escape, `\uXXXX` and surrogate pairs (a lone surrogate is an error). A repeated key keeps its last value. Containers nest at most 128 deep. An error names the byte offset: `"expected ',' or '}' at byte 17"`. `j.to_json()` is compact, with object keys in `Map` order; in strings it escapes `"`, `\`, `\n`, `\t` and other control bytes as `\u00XX`; a `Float` is written as `to_str` writes it, with `.0` added when that has no `.` or `e` (`2.0`, `1e300` as 301 digits and `.0`), and NaN and infinities as `null`. `s.parse_json()` of `j.to_json()` gives back `j` for every `j` without NaN or infinities.

```
let doc = "{\"name\": \"kio\", \"tags\": [\"a\", \"b\"]}".parse_json()?;
let name = doc.get("name").and_then(|j| j.as_str()).unwrap_or("?");
let tags = doc.get("tags").and_then(|j| j.as_list()).unwrap_or([]);
let out = Json::Arr([Json::Num(1), Json::Float(2.5), Json::Null]).to_json();   // [1,2.5,null]
```

## Capabilities

The checker's capability table is generated from [caps.wit](../caps.wit). Today:

| Type | Methods |
|---|---|
| `Dir` | `read(Str) -> Result<Bytes, IoErr>`, `create(Str, Bytes) -> Result<(), IoErr>` (new file only), `list(Str) -> Result<List<Str>, IoErr>` (`""` is the root; sorted names, a directory's ending in `/`), `write(Str, Bytes) -> Result<(), IoErr>` (create or replace atomically), `remove(Str) -> Result<(), IoErr>` (one regular file), `exists(Str) -> Result<Bool, IoErr>` |
| `Http` | `get(Str) -> Result<Bytes, HttpErr>` (a non-2xx status is `Err(Status(n))`), `send(Str, Str, List<Str>, Bytes) -> Result<HttpResponse, HttpErr>` (method, path, `"name: value"` headers, body; every status is `Ok`), `send_all(List<HttpRequest>) -> List<Result<HttpResponse, HttpErr>>` (the requests issued at once; results in list order) |
| `Kv` | `get(Str) -> Result<Option<Bytes>, KvErr>`, `set(Str, Bytes) -> Result<(), KvErr>`, `delete(Str) -> Result<(), KvErr>` (a missing key is fine), `keys(Str) -> Result<List<Str>, KvErr>` (keys with that prefix, sorted by bytes), `swap(Str, Option<Bytes>, Option<Bytes>) -> Result<Bool, KvErr>`, `scan(Str, Option<Str>, Int) -> Result<List<KvEntry>, KvErr>` (prefix, the key to start after, at most how many: entries in key order), `add(Str, Int) -> Result<Int, KvErr>` (the new value) |
| `Incoming` | `method() -> Str`, `target() -> Str` (`/path?query` as sent), `header_names() -> List<Str>` (lowercase), `header_values() -> List<Str>`, `body() -> Bytes`; one per request under `--listen` |
| `Outgoing` | `send(Int, List<Str>, List<Str>, Bytes) -> Result<(), WebErr>` (status, header names, header values, body; once per request; the client gets it at once, and `main` may go on working after it) |
| `WsMessage` | `kind() -> Str` (`"open"`, `"text"`, `"binary"` or `"close"`), `conn() -> Int` (the connection's id, unique per server run), `data() -> Bytes` (empty for open and close), `target() -> Str` and `header(Str) -> Option<Str>` (the upgrade request's `/path?query` as sent, and a header's first value, its name in any case; the same for every event of the connection) |
| `WsSend` | `text(Str) -> Result<(), WsErr>`, `binary(Bytes) -> Result<(), WsErr>`, `close(Int) -> Result<(), WsErr>`; written to the connection, in order, after `main` returns |
| `Rng` | `bytes(Int) -> Bytes` (0 to 65,536 of them), `int(Int, Int) -> Int` (uniform in `lo..hi`, `lo < hi`, no modulo bias); an argument outside those is a runtime error. `--rng name` is the OS's randomness; `--rng name=seed:N` a stream the same every run (under `--listen`, keyed by the request number too): predictable to anyone who knows the seed, so for tests and replay only, never a server facing clients |
| `Clock` | `now_ms() -> Int`: milliseconds since the Unix epoch. `--clock name` is the system clock; `--clock name=fixed:<ms>` or `fixed:2026-11-09T12:00:00Z` always that time |
| `Log` | `write(Str) -> Result<(), LogErr>`: one line to `--log name=path` (appended) or `--log name=-` (stderr), `\`, LF, CR and tab escaped as `\\`, `\n`, `\r`, `\t`, and other control characters, U+2028, U+2029 and the bidirectional controls as `\u{XX}`; under `--listen` prefixed `req N: `. Not the audit log |
| `Db` | `query(Str, List<SqlValue>) -> Result<List<List<SqlValue>>, DbErr>` (one statement with its `?` parameters: the rows, columns in `SELECT` order), `exec(Str, List<SqlValue>) -> Result<Int, DbErr>` (a statement that returns no rows: the rows it changed), `transaction(List<Str>, List<List<SqlValue>>) -> Result<Int, DbErr>` (each statement with its own parameter list, all or nothing: the rows changed in all). One SQLite database, `--db name=path` (a file, created if missing, WAL mode), `--db name=path:ro` (read-only: a program that can call `exec` or `transaction` on it is refused at startup) or `--db name=:memory:`; under `--listen` one connection shared by every request, a statement at a time |

`IoErr` is `NotFound`, `Denied`, `TooLarge`, `InvalidPath` or `Exists` (`create` on an existing path). `HttpErr` is `Denied` (the path, method or redirect is outside the grant, or the connection failed), `Timeout`, `TooLarge` (a request body over 8 MB or a response over 10 MB), `Status(Int)` (`get` on a non-2xx) or `Invalid` (a method that is not `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE` or `OPTIONS` in capitals; a header without `: `, with a control character, or naming `host`, `content-length`, `transfer-encoding`, `connection`, `upgrade`, `te`, `trailer`, `keep-alive` or `proxy-*`; more than 64 headers or one over 8 KB; a `send_all` of more than 64 requests). `HttpResponse { status: Int, headers: List<Str>, body: Bytes }` (header names lowercased, in order) and `HttpRequest { method: Str, path: Str, headers: List<Str>, body: Bytes, timeout_ms: Int }` are built-in structs. `KvErr` is `TooLarge` (a key outside 1 to 1,024 bytes, a value over 1 MB, `keys` matching over 10,000, a `scan` limit outside 1 to 1,000, an `after` over 1,024 bytes or a page over 8 MB), `Full` (the store would pass `--kv-mb`), `NotInt` (`add` on a value that is not an `Int`'s text) or `Overflow` (`add` past 64 bits; the store is unchanged). `KvEntry { key: Str, value: Bytes }` is the built-in struct `scan` returns. `WebErr` is `AlreadySent` or `Invalid` (a status outside 200-599, a header with CR or LF, lists of unequal length). `WsErr` is `Closed` (after this run's `close`, on the close event, or once the connection closed), `TooLarge` (past 1 MB sent in one run) or `Invalid` (a close code other than 1000-1003, 1007-1013 or 3000-4999). `LogErr` is `TooLarge` (a line over 64 KB once escaped, or one that would take the run past 1 MB of lines; nothing is written). `SqlValue` is `Null`, `Int(Int)`, `Float(Float)`, `Text(Str)` or `Blob(Bytes)`: a `?` parameter or a column (SQLite's own types; a `Float` NaN is stored as `Null`, as SQLite does). `DbErr` is `Invalid(Str)` (SQLite's message: bad SQL with the byte offset, two statements in one call, a parameter count that does not match, a statement under `exec` that would return rows), `Denied` (a statement the host refuses: `ATTACH`, `DETACH`, `VACUUM`, every `PRAGMA` but the reads `table_info`, `table_xinfo`, `index_list`, `index_info`, `foreign_key_list`, `foreign_keys`, `data_version` and `page_size`, `BEGIN`/`COMMIT`/`ROLLBACK`/`SAVEPOINT` outside `transaction`, `load_extension`, or any write on a `:ro` database), `Busy` (another process held the database, or another request's statement held the connection, for over 2 seconds), `TooLarge` (a result over 10,000 rows or 8 MB, a text or blob over 8 MB, SQL over 1 MB, over 4,096 parameters, over 1,000 statements in a `transaction`, a statement needing more memory than SQLite may use, or a write past `--db-mb`) or `Constraint(Str)` (a constraint failed: SQLite's message, such as `UNIQUE constraint failed: notes.title`).

`--http api=host` grants GET only (`get` and `send("GET", ..)`); `--http-methods api=POST,PUT` adds methods, and a method not granted is `Denied`. A redirect within the host is followed for GET and HEAD (5 at most, each hop checked against the grant); any other redirect is the response the program sees, `location` among its headers. A request, hops included, takes at most 10 s. `send_all` issues its requests at once and answers in list order whatever finished first, so a program's value never depends on timing except through `Timeout`: `timeout_ms` 0 means the 10 s, a smaller value cuts that one request off as `Err(Timeout)` at its position. Each request counts toward `--calls` and `--io`; the audit log has a line per request with the header names, never a value.

```
let r = api.send("POST", "/notes", ["content-type: application/json", "authorization: Bearer " + token], body.to_json().to_bytes())?;
if r.status != 201 { return Err("status " + r.status.to_str()); }
let pages = api.send_all([HttpRequest { method: "GET", path: "/a", headers: [], body: "".to_bytes(), timeout_ms: 2000 },
                          HttpRequest { method: "GET", path: "/b", headers: [], body: "".to_bytes(), timeout_ms: 2000 }]);
```

SQL text is the program's, so a query built by concatenating input is an injection the program wrote on purpose: pass values as `?` parameters (`db.query("SELECT body FROM notes WHERE id = ?", [SqlValue::Int(id)])`), never as text. A `Str` in SQL is single-quoted; a double-quoted `"name"` is an identifier (SQLite's legacy fallback to a string is off). The schema is the program's to create (`CREATE TABLE IF NOT EXISTS ..`); foreign keys are enforced.

A WebSocket server is `fn main(msg: WsMessage, conn: WsSend, ..)` under `--listen`: each event of a connection runs `main` in a fresh sandbox, one at a time and in order; state between events goes in a `Kv`. A failed run closes the connection with 1011; the `close` event still runs. A browser page may connect only from the server's own host or an origin granted with `--ws-origin`.

`swap(k, old, new)` sets `k` to `new` (`None` deletes it) only if its value is `old` (`None`: absent), atomically, and returns whether it did. Concurrent requests use it to claim ids or replace records without losing updates: read, compute, `swap`, retry on `false`. To count, `add(k, n)` adds `n` to the integer at `k` (absent: 0) atomically and returns the sum, stored as the text `Int.to_str()` writes, so `get` and JSON read it; a value in any other form (`"01"`, `"+1"`, `" 1"`) is `NotInt`. `scan(prefix, after, limit)` returns the entries whose key starts with `prefix` and is greater than `after` (by bytes), in key order, at most `limit` of them; the next page passes the last key as `after`, and a page shorter than `limit` is the last.

## Rules the checker enforces

1. **Capabilities are parameters only.** A cap can be received as a parameter, have a method called on it, or be passed as an argument. These are rejected:

   ```
   let d = input;                     // bound to a local
   struct Job { dir: Dir }            // stored in a field
   fn pick(a: Dir, b: Dir) -> Dir     // returned
   let dirs = [input, out];           // put in a list
   fn f(x: Option<Dir>)               // nested in a type
   ```

   A `fn` type may not mention a capability (`fn(Dir)`, `fn() -> Option<Dir>`), and a function with a capability parameter cannot be used as a value: "function values cannot take or return capabilities". Pass the capability to a named function instead.

   A closure cannot capture a capability: "closure captures capability `out`". The hint, taking it as a parameter `|out: Dir, ..|`, is then refused by the `fn` type rule, so code that needs a capability is a named function that takes it as a parameter.

   A type parameter is never a capability, however it would be fixed (`id(d)`, `id::<Dir>`, `Box<Dir>`, an expected type): "`T` cannot be `Dir`: capabilities are parameters only". A generic function takes a capability as a parameter of its own type (`fn keep<T>(d: Dir, x: T)`), so every copy of it has the same authority and the report lists it once.

   So a function's authority is exactly its capability parameters, and the report is read off the signatures. A function value or closure carries none. The report lists named functions only; a closure's body is part of the function it is written in.

2. **Capabilities cannot be constructed.** There is no literal, constructor or cast that produces a `Dir`. Only the host provides them, as arguments to `main`.

3. **`main` is the only entry point.** Its parameters are all capabilities and are the program's whole authority. Their names are what the operator binds with `--dir name=path`, `--http`, `--kv`, `--db`, `--rng`, `--clock`, `--log`; under `--listen`, `Incoming`, `Outgoing`, `WsMessage` and `WsSend` are bound by type, whatever the names. It may return any type: `kio run` prints it, and an `Err` exits 1.

4. **`?` needs matching error types.** `d.read(x)?` inside a function returning `Result<_, IoErr>` is fine. Inside one returning `Option` or a different error type it is rejected. No implicit conversion.

## Authority report

One line per function: each capability parameter with the methods it may call, including through helpers and recursion. A capability passed to a helper is charged with the helper's calls. The report describes possible authority, not an execution trace.

```
load      : d read
main      : input read, out create
parse_row : none
total     : none
```

## Errors you will see

```
error: `summarize` has no capability parameter but calls `Dir.read`
  pass a `Dir` in, or move the read to the caller

error: capability `input` bound to local `d`
  pass `input` directly instead

error: cannot return `Dir` from `pick`
  capabilities are parameters only

error: function values cannot take or return capabilities
  pass the capability to a named function instead

error: cannot infer `T`
  write `empty::<Int>(..)`

error: closure captures capability `out`
  use a named function that takes it as a parameter: `fn save(out: Dir, ..)`

error: `?` on `Result<Bytes, IoErr>` in a function returning `Option<Int>`
  change the return type or `match` on the result

error: no method `delete` on `Dir`
  Dir has: read, create, list, write, remove, exists

error: kio has no references
  use `Str` instead of `&str`
```

## Not in v0

- Attenuation: deriving a narrower cap, such as a sub-directory. First candidate for v1.
- Traits and `impl`.
- Tuples.
- `use` and nested modules.
- Globals (`static`); constants are literals only ([open_questions](open_questions.md) #34).
