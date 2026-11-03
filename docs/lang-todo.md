# lang TODO

Build the `lang` crate: parser, checker, authority report and interpreter for the language in [language.md](language.md). Each task below is self-contained and can be handed to an agent as is.

## Rules for every task

- Read [language.md](language.md) first. It is the spec. If it is ambiguous, stop and ask; do not invent semantics.
- Rust std only. No dependencies, no macros beyond `derive`, no `unsafe`.
- Minimal code: straightforward control flow, no speculative abstractions, no features beyond the task. A human reviews every line.
- Comments only where the code cannot say it.
- Tests in `lang/tests/`, using inline source strings. `cargo test -p lang` passes, `cargo clippy -p lang` is clean.
- Unsupported syntax fails with a `Diag` naming the kio alternative, never a panic.

## Layout

```
Cargo.toml          workspace, members = ["lang"]
lang/src/lib.rs     pub API, Diag, Span
lang/src/lex.rs     tokens
lang/src/ast.rs     syntax tree
lang/src/parse.rs   hand-written recursive descent
lang/src/check.rs   types, capability rules, produces Program
lang/src/report.rs  authority report
lang/src/eval.rs    tree-walking interpreter, Value, Caps trait
```

Public API, fixed from task 3 on:

```rust
pub fn check(src: &str, caps: &CapTable) -> Result<Program, Vec<Diag>>;
pub fn report(p: &Program) -> String;
pub fn run(p: &Program, caps: &mut dyn Caps) -> Result<Value, RunErr>;
```

`Program` can only be built by `check`, so unchecked code can never run.

## Tasks

Each task covers lexing, parsing, checking and evaluating its constructs, unless it says otherwise.

- [ ] **1. Lexer.** Workspace, crate, `Span { start, end }` byte offsets, `Diag { span, msg, hint }`. Tokens: identifiers, keywords, `Int` and `Str` literals with `\n \t \" \\` escapes, punctuation, `//` comments. Reject float literals, `'` and `&` with kio messages.
  Done: token-stream tests, and error tests checking span and message.

- [ ] **2. Parser core.** `fn` items, typed parameters, return type, blocks with tail expression, `let`, `let mut`, `=`, `if`/`else`, `while`, `return`, literals, unary and binary operators with Rust precedence, calls, method calls, field access, indexing. Types are names with optional `<..>` arguments. Reject `impl`, `trait`, `use`, `mod`, `pub`, closures and tuples with kio messages.
  Done: parse tests for each construct, and one rejection test per unsupported item.

- [ ] **3. Checker and interpreter core.** `Int`, `Bool`, `Str`, `()`. Name resolution, operator types, call arity and types, `mut` required for assignment, every path returns the declared type. Evaluation with runtime errors for overflow and division by zero. `CapTable` and `Caps` exist but stay empty.
  Done: a recursive `fib` checks and runs. Rejection tests for unknown name, type mismatch, missing return, and assignment to a non-`mut` local.

- [ ] **4. Structs and enums.** Declarations, struct literals (not allowed in `if`, `while` or `match` heads, as in Rust), paths such as `Status::Paid`, field access, enum variants with payloads, exhaustive `match` with one-level patterns.
  Done: a function matching on an enum with payloads runs. Rejection tests for unknown field, missing field and non-exhaustive match.

- [ ] **5. Option, Result and `?`.** `Some`, `None`, `Ok`, `Err` as built-in enums. `?` with matching error types only. `None` takes its type from the declared or expected type.
  Done: `?` propagates through two calls. `?` on a `Result` inside an `Option` function is rejected.

- [ ] **6. Lists, loops, Bytes.** `List<T>`, `[a, b]`, `[]` with declared type, indexing, `for x in list`, `for i in 0..n`, `break`, `continue`, and the `Str`, `Bytes`, `Int` and `List` built-ins in language.md. Values are copied: changing a copy leaves the original unchanged.
  Done: tests per built-in, an out-of-bounds runtime error, and a copy test.

- [ ] **7. Map.** `Map<K, V>` with `Int` or `Str` keys, `Map::new()` (a path call), `get`, `insert`, `remove`, `keys` in key order. Use `BTreeMap`.
  Done: a word-count function runs and `keys()` returns sorted keys.

- [ ] **8. Capabilities.** `CapTable`: capability types, each with methods and signatures, built by hand in tests for `Dir`, `Http`, `IoErr` and `HttpErr` as in language.md. Enforce checker rules 1 to 3. `Caps::call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Value`, where `cap` is the name of `main`'s parameter. Add an in-memory `StubDir` for tests.
  Done: a helper reads stub bytes through a passed `Dir`. One rejection test per escape in rule 1, including `Option<Dir>`, plus a forged-construction test. A rejected program records zero stub calls.

- [ ] **9. Authority report.** As specified in language.md: map each helper's capability parameters back to the caller's arguments, and iterate until nothing changes so recursion terminates. Sort lines by function name.
  Done: report tests for direct calls, a two-level helper chain, mutual recursion, and a pure function (`none`).

- [ ] **10. Primer end to end.** The one-screen program in language.md checks, prints the documented report and runs against `StubDir`. Add the Rust-ism rejection tests: `&str`, `String`, `Vec`, `.iter().map(|x| ..)`, `impl`, `let x: f64`.
  Done: those tests pass.

## Later, not in `lang`

Generating the `CapTable` from `caps.wit` (a `guest` build script using `wit-parser`), `guest`, `host` and the MCP `check` tool.
