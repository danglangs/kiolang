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
    match parse_row(line) {
      Some(r) => rows.push(r),
      None => {}
    }
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
| Containers | `List<T>`, `Map<K, V>` (key `Int` or `Str`) |
| Errors | `Option<T>`, `Result<T, E>`, `?` |
| User types | `struct`, and `enum` with payloads, generic or not (`struct Page<T>`); a type may contain itself only through `List` or `Map` |
| Control | `if`, `if let`, `while`, `loop`, `for x in list`, `for i in 0..n`, `match`, `break`, `continue`, `return` |
| Functions | free functions, recursion, generic functions `fn f<T>(..)`, function values of type `fn(T, ..) -> R`, closures |
| Capabilities | `Dir`, `Http`, `Kv`, `WsMessage`, `WsSend`, generated from `caps.wit` |

Everything else is rejected with a message that names the kio alternative, for example `&str` (use `Str`), `Vec` (use `List`), `impl`, traits, bounds such as `T: Clone`, `use`, `mod`, tuples, globals, `unsafe`.

## Semantics

- **Values are copied.** Assignment and argument passing never share state. Only `let mut` locals change: `x = v`, `x += v`, `x -= v`, `x.f = v`, `xs[i] = v`, `m.insert(k, v)`, `xs.push(v)`, `m.remove(k)`. A function changes its caller's data only by returning a new value. `for x in xs` iterates over `xs` as it was when the loop started.
- **`loop { .. }`** repeats until `break`; `continue` and `return` work as in Rust. A `loop` with no `break` never finishes normally, so `fn f() -> Int { loop { return 1; } }` needs no tail. `break` takes no value: use a `let mut` and `break`.
- **Statements end in `;`**, as in Rust. A block's last expression has no `;` and is the block's value. `if`, `while`, `loop`, `for` and `match` used as statements need no `;`. `if` chains with `else if`.
- **Types are explicit where they cannot be read off the value.** `[]`, `Map::new()` and `None` need a declared or expected type: `let mut rows: List<Row> = [];`.
- **Operators.** `+ - * / %` on `Int` or on `Float`, `+` on `Str`, `== !=` on two values of one type, structurally (structs, enums, `Option`, `Result`, `List`, `Map` included; capabilities have no equality), `< <= > >=` on `Int` or `Float`, `&& || !` on `Bool`. On `Int`, `/` and `%` truncate toward zero, as in Rust. `Int` and `Float` never mix: `1 + 2.0` is an error; convert with `.to_float()` or `.to_int()`.
- **`Float` is Rust's `f64`.** Literals `1.5`, `2.0`, `1e9`, `1.5e-3` (`1.` and `.5` are errors). Arithmetic gives IEEE results and never stops the run: `1.0 / 0.0` is `inf`, `0.0 / 0.0` is `NaN`. `==` is IEEE: `NaN != NaN` and `0.0 == -0.0`, also inside structs, enums and lists. A `Float` is not a `Map` key and cannot be matched: compare with `<` or `==`.
- **Runtime errors stop the run:** `Int` overflow, `Int` division by zero, an out-of-bounds index, invalid UTF-8 in `to_str`. Deep recursion stops the run with "stack overflow": `--stack` sets the limit, 2 MB by default (about 40,000 calls of a small function), at most 8 MB. Nothing wraps or panics silently.
- **`match` is exhaustive.** A pattern is `_`, a name (binds the value), an `Int`, `Str` or `Bool` literal, or a variant whose parts are patterns, at any depth: `Err(IoErr::Exists)`, `Some(Some(x))`, `Shape::Line(1, _)`. Arms are tried in order; a name binds at most once per pattern. Every value must match some arm (`Int` and `Str` need a `_` or name at that position) and every arm must match some value no earlier arm does, else "unreachable arm". A match too large to check is rejected: split it. An arm may be `return`, `break` or `continue` (`None => break,`); it has type `Never`.
- **`if let PAT = e { .. } else { .. }`** runs the block with `PAT`'s names bound when `e` matches, else the else branch. Without `else` its type is `()`. It chains with `else if` and `else if let`. It need not be exhaustive.
- **`Str` is UTF-8.** `len()` counts bytes, in constant time. Literal escapes: `\n \r \t \0 \" \\`.
- **Deterministic.** No clock or randomness. `Map` iterates in key order.
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
- **Generic functions.** Type parameters are uppercase names after the function's name. In the body a `T` is opaque: it can be passed, returned, held in `List<T>`, `Option<T>`, `Result` or as a `Map` value, and matched by a name or `_`; it has no methods, operators, `==` or literals, so take a function for what the code needs (`eq: fn(T, T) -> Bool`). At a call each type parameter comes from the arguments, where it first appears (a later one must agree: "`T` is Int here but Str at argument 2"), then from the expected type; arguments that need a type (`None`, `[]`, `Map::new()`, a closure) are checked after the others, so `or(None, 5)` works. Otherwise write it: `empty::<Int>()`. A generic function used as a value needs its types fixed by `::<..>` or an expected `fn` type. A type parameter is never a capability (rule 1). Each distinct list of type arguments makes one copy of the function, named `first<Int>` (a value prints `<fn first<Int>>`); recursion that keeps changing them (`f<T>` calling `f<List<T>>`) is refused: "this generic function instantiates without end".

  ```
  fn first<T>(xs: List<T>) -> Option<T> { if xs.len() == 0 { None } else { Some(xs[0]) } }
  fn apply<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
  let a = first(["x"]);                       // T = Str
  let xs: List<Int> = empty();                // T from the expected type
  let n = apply(|x| x + 1, 41);               // T from 41, then U from the closure
  let f: fn(List<Int>) -> Option<Int> = first;
  ```
- **Generic structs and enums.** Type parameters as on functions; a use names every argument (`Page<Int>`, `web::Tree<Str>`). A literal or variant takes its type arguments from its values, then from the expected type, as a call does; `Tree::Leaf` alone needs one: "cannot infer `T` for `Tree::Leaf`". Fields, field writes and patterns have the instance's types. `==` works on an instance whose arguments allow it (`Page<Int>`), not on `Page<T>` inside a generic function. A type argument is never a capability. A type may contain itself only through `List` or `Map`, in every instance (`struct X { w: W<X> }` is refused when `W<T>` holds a `T`). Each instance is its own type, printed without its arguments, as Rust's `Debug` does: `Page { items: [1], next: None }`. A type whose fields keep growing its arguments (`struct S<T> { s: List<S<List<T>>> }`) is refused once used: "generic type `S` instantiates without end". At most 64 instances of one generic function or type, each type argument at most 256 types. `Option` and `Result` stay built in.

  ```
  struct Page<T> { items: List<T>, next: Option<Int> }
  enum Tree<T> { Leaf, Node(List<Tree<T>>, T) }
  let p = Page { items: [1, 2], next: None };     // Page<Int>
  let t: Tree<Str> = Tree::Leaf;
  fn first<T>(p: Page<T>) -> Option<T> { if p.items.len() == 0 { None } else { Some(p.items[0]) } }
  ```
- **Modules.** `mod web;` in the root file loads `web.kio`; its `pub` items are used as `web::f`, `web::T`. Modules are flat: one file each, declared only in the root. A module may use another declared module's `pub` items the same way (cycles are fine) but not the root's.

## Built-ins

| Type | Methods |
|---|---|
| `Int` | `to_str() -> Str`, `to_float() -> Float` |
| `Float` | `to_str() -> Str` (as Rust's `{}`: `1`, `0.30000000000000004`, `NaN`, `inf`, `-0`), `to_int() -> Option<Int>` (toward zero; `None` if NaN, infinite or out of range), `abs()`, `floor()`, `ceil()`, `round()` (half away from zero), `sqrt()`, all `-> Float`, `is_nan() -> Bool`, `is_finite() -> Bool`, `is_infinite() -> Bool` |
| `Str` | `len() -> Int`, `split(Str) -> List<Str>`, `lines() -> List<Str>`, `trim() -> Str`, `contains(Str) -> Bool`, `starts_with(Str) -> Bool`, `ends_with(Str) -> Bool`, `strip_prefix(Str) -> Option<Str>`, `strip_suffix(Str) -> Option<Str>`, `find(Str) -> Option<Int>` (byte offset), `replace(Str, Str) -> Str`, `parse_int() -> Option<Int>`, `parse_float() -> Option<Float>` (as Rust's `f64::from_str`: `1e3`, `inf`, `NaN`), `slice(Int, Int) -> Str` (byte offsets as Rust's `&s[a..b]`; out of range, `a > b`, negative or not on a character boundary is a runtime error), `to_bytes() -> Bytes` |
| `Bytes` | `b[i] -> Int` (0-255), `len() -> Int`, `push(Int)` (0-255), `slice(Int, Int) -> Bytes` (as `Str.slice`, without the boundary rule), `to_str() -> Str` |
| `Option<T>` | `unwrap_or(T) -> T`, `is_some() -> Bool`, `is_none() -> Bool`, `ok_or(E) -> Result<T, E>`, `map(fn(T) -> U) -> Option<U>`, `and_then(fn(T) -> Option<U>) -> Option<U>` |
| `Result<T, E>` | `unwrap_or(T) -> T`, `is_ok() -> Bool`, `is_err() -> Bool`, `ok() -> Option<T>`, `err() -> Option<E>`, `map(fn(T) -> U) -> Result<U, E>`, `map_err(fn(E) -> F) -> Result<T, F>`, `and_then(fn(T) -> Result<U, E>) -> Result<U, E>` |
| `List<T>` | `xs[i]`, `len() -> Int`, `push(T)`, `pop() -> Option<T>`, `contains(T) -> Bool`, `sort()` (`List<Int>` and `List<Str>`, bytes order, stable), `map(fn(T) -> U) -> List<U>`, `filter(fn(T) -> Bool) -> List<T>`, `any(fn(T) -> Bool) -> Bool`, `all(fn(T) -> Bool) -> Bool` |
| `Map<K, V>` | `Map::new()`, `get(K) -> Option<V>`, `insert(K, V)`, `remove(K)`, `keys() -> List<K>` |

The methods taking `fn(..)` take any function value: a name (`json::as_str`), a closure or a local. A closure written as the argument gets its parameter type from the receiver and its return type from its body: `xs.filter(|x| x > 0).map(|x| x * 2)`. In one given to `and_then`, `?` returns the closure's `None` or `Err`, and `Ok(x)` takes the receiver's error type; `None` or `Err(e)` alone need a typed closure (`let f: fn(Str) -> Option<Int> = |s| ..`). The receiver is evaluated, then the function, then each call in list order; `any` and `all` stop at the first value that decides, as Rust's.

## Capabilities

The checker's capability table is generated from [caps.wit](../caps.wit). Today:

| Type | Methods |
|---|---|
| `Dir` | `read(Str) -> Result<Bytes, IoErr>`, `create(Str, Bytes) -> Result<(), IoErr>` (new file only), `list(Str) -> Result<List<Str>, IoErr>` (`""` is the root; sorted names, a directory's ending in `/`), `write(Str, Bytes) -> Result<(), IoErr>` (create or replace atomically), `remove(Str) -> Result<(), IoErr>` (one regular file), `exists(Str) -> Result<Bool, IoErr>` |
| `Http` | `get(Str) -> Result<Bytes, HttpErr>` |
| `Kv` | `get(Str) -> Result<Option<Bytes>, KvErr>`, `set(Str, Bytes) -> Result<(), KvErr>`, `delete(Str) -> Result<(), KvErr>` (a missing key is fine), `keys(Str) -> Result<List<Str>, KvErr>` (keys with that prefix, sorted by bytes), `swap(Str, Option<Bytes>, Option<Bytes>) -> Result<Bool, KvErr>` |
| `WsMessage` | `kind() -> Str` (`"open"`, `"text"`, `"binary"` or `"close"`), `conn() -> Int` (the connection's id, unique per server run), `data() -> Bytes` (empty for open and close) |
| `WsSend` | `text(Str) -> Result<(), WsErr>`, `binary(Bytes) -> Result<(), WsErr>`, `close(Int) -> Result<(), WsErr>`; written to the connection, in order, after `main` returns |

`IoErr` is `NotFound`, `Denied`, `TooLarge`, `InvalidPath` or `Exists` (`create` on an existing path). `HttpErr` is `Denied`, `Timeout`, `TooLarge` or `Status(Int)`. `KvErr` is `TooLarge` (a key outside 1 to 1,024 bytes, a value over 1 MB, `keys` matching over 10,000) or `Full` (the store would pass `--kv-mb`). `WsErr` is `Closed` (after this run's `close`, on the close event, or once the connection closed), `TooLarge` (past 1 MB sent in one run) or `Invalid` (a close code other than 1000-1003, 1007-1013 or 3000-4999).

A WebSocket server is `fn main(msg: WsMessage, conn: WsSend, ..)` under `--listen`: each event of a connection runs `main` in a fresh sandbox, one at a time and in order; state between events goes in a `Kv`. A failed run closes the connection with 1011; the `close` event still runs. A browser page may connect only from the server's own host or an origin granted with `--ws-origin`.

`swap(k, old, new)` sets `k` to `new` (`None` deletes it) only if its value is `old` (`None`: absent), atomically, and returns whether it did. Concurrent requests use it to count or claim ids without losing updates: read, compute, `swap`, retry on `false`.

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

3. **`main` is the only entry point.** Its parameters are all capabilities and are the program's whole authority. Their names are what the operator binds with `--dir name=path`. It returns `()` or `Result<(), E>`.

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
  take it as a parameter: `|out: Dir, ..|`

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
- The bounds `T: Eq` and `T: Ord` (planned: L7c).
- Tuples.
- `use` and nested modules.
