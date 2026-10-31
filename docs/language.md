# kio primer

kio source parses as Rust, so an agent that knows Rust can write it. The Rust compiler is never involved: kio's own checker and interpreter read the source.

## One screen of kio

```
struct Row { name: Str, amount: Int }

enum Status { Paid, Due }

fn parse_row(line: Str) -> Option<Row> {
  let parts = line.split(",")
  if parts.len() != 2 { return None }
  Some(Row { name: parts[0], amount: parts[1].parse_int()? })
}

fn total(rows: List<Row>) -> Int {
  let mut sum = 0
  for r in rows { sum = sum + r.amount }
  sum
}

fn load(d: Dir, name: Str) -> Result<List<Row>, IoErr> {
  let text = d.read(name)?.to_str()
  let mut rows = []
  for line in text.lines() {
    match parse_row(line) {
      Some(r) => rows.push(r),
      None => {}
    }
  }
  Ok(rows)
}

fn main(input: Dir, out: Dir) -> Result<(), IoErr> {
  let rows = load(input, "invoices.csv")?
  out.create("total.txt", total(rows).to_str().to_bytes())
}
```

## Types

| Kind | Types |
|---|---|
| Scalars | `Int`, `Bool`, `Str`, `Bytes`, `()` |
| Containers | `List<T>`, `Option<T>`, `Result<T, E>` |
| User-defined | `struct`, `enum` |
| Capabilities | `Dir`, `Http`, and whatever else `prelude.kio` declares with `cap` |

Everything is immutable except `let mut` locals. There are no references, lifetimes or ownership rules. Values are copied.

## Rules the checker enforces

1. **Capabilities are parameters only.** A cap can be received as a parameter, have a method called on it, or be passed as an argument. These are rejected:

   ```
   let d = input                      // bound to a local
   struct Job { dir: Dir }            // stored in a field
   fn pick(a: Dir, b: Dir) -> Dir     // returned
   let dirs = [input, out]            // put in a list
   ```

   So a function's authority is exactly its capability parameters, and the report is read off the signatures.

2. **Capabilities cannot be constructed.** There is no literal, constructor or cast that produces a `Dir`. Only the host provides them, as arguments to `main`.

3. **`main` is the only entry point.** Its capability parameters are the program's whole authority. Their names are what the operator binds with `--dir name=path`.

4. **`?` needs matching error types.** `d.read(x)?` inside a function returning `Result<_, IoErr>` is fine. Inside one returning `Option` or a different error type it is rejected. No implicit conversion.

5. **No closures, globals, traits, generics, `unsafe` or reflection.** Named functions and the built-in types only. Recursion is allowed; fuel bounds it.

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
```

## Not in v0

- Attenuation: deriving a narrower cap, such as a sub-directory. First candidate for v1.
- Closures and traits.
- Generics beyond the built-in containers.
- A real standard library. v0 ships a few built-ins on `Str`, `Bytes` and `List`, listed in `prelude.kio`.
