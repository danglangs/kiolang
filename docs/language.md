# kio primer

kio uses a subset of Rust syntax, so every kio program parses as Rust. Types and semantics are kio's own: there are no references or ownership, and values are copied. The Rust compiler is never involved. kio's checker and interpreter read the source.

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
| Scalars | `Int` (i64), `Bool`, `Str`, `Bytes`, `()` |
| Containers | `List<T>`, `Map<K, V>` (key `Int` or `Str`) |
| Errors | `Option<T>`, `Result<T, E>`, `?` |
| User types | `struct`, and `enum` with payloads |
| Control | `if`, `while`, `for x in list`, `for i in 0..n`, `match`, `break`, `continue`, `return` |
| Functions | free functions, recursion |
| Capabilities | `Dir`, `Http`, generated from `caps.wit` |

Everything else is rejected with a message that names the kio alternative, for example `&str` (use `Str`), `Vec` (use `List`), closures (use `for`), `impl`, traits, generics, `use`, `mod`, floats, tuples, globals, `unsafe`.

## Semantics

- **Values are copied.** Assignment and argument passing never share state. Only `let mut` locals change: `x = v`, `x += v`, `x -= v`, `x.f = v`, `xs[i] = v`, `m.insert(k, v)`, `xs.push(v)`, `m.remove(k)`. A function changes its caller's data only by returning a new value. `for x in xs` iterates over `xs` as it was when the loop started.
- **Statements end in `;`**, as in Rust. A block's last expression has no `;` and is the block's value. `if`, `while`, `for` and `match` used as statements need no `;`. `if` chains with `else if`.
- **Types are explicit where they cannot be read off the value.** `[]`, `Map::new()` and `None` need a declared or expected type: `let mut rows: List<Row> = [];`.
- **Operators.** `+ - * / %` on `Int`, `+` on `Str`, `== !=` on scalars, `< <= > >=` on `Int`, `&& || !` on `Bool`. `/` and `%` truncate toward zero, as in Rust.
- **Runtime errors stop the run:** `Int` overflow, division by zero, an out-of-bounds index, invalid UTF-8 in `to_str`, call depth over 1_000. Nothing wraps or panics silently.
- **`match` is exhaustive.** Patterns are one level deep: a variant with bindings, a literal, a name or `_`.
- **`Str` is UTF-8.** `len()` counts bytes, in constant time.
- **Deterministic.** No clock or randomness. `Map` iterates in key order.
- **Comments:** `//` only.
- **Reserved words.** All Rust keywords not used by kio are reserved and cannot be names.

## Built-ins

| Type | Methods |
|---|---|
| `Int` | `to_str() -> Str` |
| `Str` | `len() -> Int`, `split(Str) -> List<Str>`, `lines() -> List<Str>`, `trim() -> Str`, `contains(Str) -> Bool`, `starts_with(Str) -> Bool`, `parse_int() -> Option<Int>`, `to_bytes() -> Bytes` |
| `Bytes` | `len() -> Int`, `to_str() -> Str` |
| `List<T>` | `xs[i]`, `len() -> Int`, `push(T)` |
| `Map<K, V>` | `Map::new()`, `get(K) -> Option<V>`, `insert(K, V)`, `remove(K)`, `keys() -> List<K>` |

## Capabilities

The checker's capability table is generated from [caps.wit](../caps.wit). Today:

| Type | Methods |
|---|---|
| `Dir` | `read(Str) -> Result<Bytes, IoErr>`, `create(Str, Bytes) -> Result<(), IoErr>` |
| `Http` | `get(Str) -> Result<Bytes, HttpErr>` |

`IoErr` is `NotFound`, `Denied`, `TooLarge` or `InvalidPath`. `HttpErr` is `Denied`, `Timeout`, `TooLarge` or `Status(Int)`.

## Rules the checker enforces

1. **Capabilities are parameters only.** A cap can be received as a parameter, have a method called on it, or be passed as an argument. These are rejected:

   ```
   let d = input;                     // bound to a local
   struct Job { dir: Dir }            // stored in a field
   fn pick(a: Dir, b: Dir) -> Dir     // returned
   let dirs = [input, out];           // put in a list
   fn f(x: Option<Dir>)               // nested in a type
   ```

   So a function's authority is exactly its capability parameters, and the report is read off the signatures.

2. **Capabilities cannot be constructed.** There is no literal, constructor or cast that produces a `Dir`. Only the host provides them, as arguments to `main`.

3. **`main` is the only entry point.** Its parameters are all capabilities and are the program's whole authority. Their names are what the operator binds with `--dir name=path`. It returns `()` or `Result<(), E>`.

4. **`?` needs matching error types.** `d.read(x)?` inside a function returning `Result<_, IoErr>` is fine. Inside one returning `Option` or a different error type it is rejected. No implicit conversion.

## Authority report

One line per function: each capability parameter with the methods it may call, including through helpers and recursion. A capability passed to a helper is charged with the helper's calls. The report describes possible authority, not an execution trace.

```
main       : input read, out create
load       : d read
parse_row  : none
total      : none
```

## Errors you will see

```
error: `summarize` has no capability parameter but calls `Dir.read`
  pass a `Dir` in, or move the read to the caller

error: capability `input` bound to local `d`
  pass `input` directly instead

error: cannot return `Dir` from `pick`
  capabilities are parameters only

error: `?` on `Result<Bytes, IoErr>` in a function returning `Option<Int>`
  change the return type or `match` on the result

error: no method `delete` on `Dir`
  Dir has: read, create

error: kio has no references
  use `Str` instead of `&str`
```

## Not in v0

- Attenuation: deriving a narrower cap, such as a sub-directory. First candidate for v1.
- Closures, traits, `impl` and user generics.
- Floats and tuples.
- Modules. A program is one file; `mod`, `use` and `pub` are reserved for them.
