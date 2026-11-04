# lang TODO

Build the `lang` crate: parser, checker, authority report and interpreter for the language in [language.md](language.md). Each task below is self-contained and can be handed to an agent as is.

## Rules for every task

- Read [language.md](language.md) first. It is the spec. If it is ambiguous, stop and ask; do not invent semantics.
- Rust std only. No dependencies, no macros beyond `derive`, no `unsafe`.
- Minimal code: straightforward control flow, no speculative abstractions, no features beyond the task. A human reviews every line.
- Comments only where the code cannot say it.
- Tests in `lang/tests/`, using inline source strings. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass, as CI runs them.
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

`Program` can only be built by `check`, so unchecked code can never run. It holds resolved names and types and no interpreter state.

## Tasks

Each task covers lexing, parsing, checking and evaluating its constructs, unless it says otherwise.

- [x] **1. Lexer.** Workspace (edition 2021), crate, `.gitignore` with `target/`. In `lib.rs`: `Span { start, end }` byte offsets, `Diag { span, msg, hint: Option<String> }`. In `lex.rs`: `pub fn lex(src: &str) -> Result<Vec<Token>, Diag>`, stopping at the first error; `Token { kind, span }`.
  - Identifiers: ASCII `[A-Za-z_][A-Za-z0-9_]*`.
  - Keywords: `fn let mut if else while for in return match break continue true false struct enum`. Reserve every other Rust keyword so later features cannot break programs: `as async await box const crate dyn extern impl loop mod move pub ref self Self static super trait type unsafe use where yield abstract become do final macro override priv typeof unsized virtual try gen union`. Reserved words lex as `Reserved(name)`; the parser rejects them by name.
  - `Int` literals: decimal digits fitting `i64`; larger is an error. `Str` literals: `"..."` with `\n \t \" \\`; other escapes and unterminated strings are errors.
  - Symbols: `( ) { } [ ] , ; : :: . .. -> => = += -= + - * / % == != < <= > >= && || ! ?`.
  - `0..n` lexes as `Int`, `..`, `Ident`. `1.5` is an error: kio has no floats.
  - Skip whitespace and `//` comments.
  - Errors with kio hints: `&` (no references; pass by value), `|` (no closures; use `for`), `'` (no chars or lifetimes; use `Str`), `/*` (use `//`), any other character.
  Done: one token-stream test per bullet, and error tests checking span, message and hint.

- [x] **2. Parser core.** `ast.rs` and `parse.rs`. `pub fn parse(src: &str) -> Result<Vec<Item>, Diag>` lexes, then parses with recursive descent, stopping at the first error. Every expression, statement and type node carries a `Span`.
  - Items: `fn name(a: T, ..) -> T { .. }`; no return type means `()`. Only `fn` for now.
  - Types: a name with optional `<T, ..>` arguments, and `()`. Meaning is checked in task 3.
  - Statements: `let x = e;`, `let mut x: T = e;`, `x = e;`, `x += e;`, `x -= e;` (left side: a name, field or index), `return;` and `return e;`, `while e { .. }`, expression statements.
  - A block is statements plus an optional tail expression with no `;`. `if`, `while` and block expressions as statements need no `;`.
  - Expressions: `Int`, `Str`, `true`, `false`, names, `()`, `( e )`, `if e { .. } else if e { .. } else { .. }`, `f(a, ..)`, `e.m(a, ..)`, `e.f`, `e[i]`.
  - Precedence, highest first: postfix `() . []`; unary `- !`; `* / %`; `+ -`; `== != < <= > >=`; `&&`; `||`. Binary operators are left-associative. Comparisons do not chain: `a < b < c` is an error, as in Rust.
  - Rejected with kio hints: `Reserved` words in item or expression position (`impl`, `trait`, `use`, `mod`, `pub`, ..), tuples `(a, b)`, `struct`, `enum`, `for`, `match` and `break` until their tasks, trailing tokens after the last item.
  - Nesting deeper than 64 (blocks plus expressions) is a `Diag`, so hostile input cannot overflow the Rust stack.
  Done: one parse test per bullet, asserting tree shape; a precedence test (`1 + 2 * 3 == 7 && !b`); one rejection test per rejected item, checking span and message; a 65-deep nesting test on the default test stack.

- [x] **3. Checker and interpreter core.** `check.rs` and `eval.rs`; `check` and `run` re-exported from `lib.rs`.
  - Types: `Int`, `Bool`, `Str`, `()`. Any other type name is an error until its task.
  - Functions: collect all signatures first, so call order in the file does not matter. Duplicate function or parameter names are errors. `main` must exist and take no parameters in this task; any return type.
  - Scopes: one per block. `let` may shadow. A `let` with a type checks the initializer against it.
  - Assignment: target is a name only (fields and indexes come later); it must be `let mut`. `x += e` means `x = x + e`.
  - Operators: `+ - * / %` and unary `-` on `Int`; `+` on `Str`; `== !=` on two values of the same scalar type; `< <= > >=` on `Int`; `&& || !` on `Bool`, with `&&` and `||` short-circuiting.
  - `if` and `while` conditions are `Bool`. `if` without `else` has type `()`. Both branches of `if`/`else` have the same type.
  - Return paths: a block's type is its tail's type, or `()` without a tail. A block that ends in `return`, or in an `if`/`else` whose branches both end in `return`, never falls through and matches any type. `return e` and the body must match the declared type.
  - Calls: the callee is a function name; arity and argument types must match. Field access, indexing and method calls are errors until their tasks.
  - `check` reports at most one `Diag` per function and keeps checking the rest, in source order.
  - `Program` holds the checked functions; its fields are private. `CapTable` and the `Caps` trait exist but are empty.
  - `run` evaluates `main` and returns its `Value` (`Int`, `Bool`, `Str`, `Unit`). Runtime errors are `RunErr { span, msg }`: overflow, including `i64::MIN / -1` and `-i64::MIN`, division or remainder by zero, and evaluation depth over 5_000 nested expressions, which bounds stack use for any program.
  Done: `fib(20)` returns 6765; a mutual-recursion test; a test where `false && 1 / 0 == 0` does not error; overflow, zero-division and depth runtime tests (1_000 recursive calls succeed; deep recursion and maximal nesting stop with an error on an 8 MB stack). Rejection tests, checking span and message: unknown name, unknown function, wrong arity, type mismatch, non-`Bool` condition, missing return, `if` branches of different types, assignment to a non-`mut` local, duplicate function.

Tasks 4 onward are outlines. Each gets a pinned spec, like tasks 1 to 3, before it is handed off. A task that spans parse, check and eval and grows large is split by pass.

- [ ] **4. Structs.** Declarations, struct literals (not in `if` or `while` heads, as in Rust), field access, field assignment `x.f = v` on `mut` locals. Turn `Ty` into a non-`Copy` enum with `Display`.
  Done: unknown field, missing field and duplicate field are rejected; a struct copy test.

- [ ] **5. Enums and `match`.** Variants with payloads, paths such as `Status::Paid`, exhaustive `match` with one-level patterns, `match` arms as statements.
  Done: a function matching on an enum with payloads runs; non-exhaustive match is rejected.

- [ ] **6. Option, Result and `?`.** Built-in generic enums `Some`, `None`, `Ok`, `Err`. `?` with matching error types only. `None` takes its type from the declared or expected type.
  Done: `?` propagates through two calls; `?` on a `Result` inside an `Option` function is rejected.

- [ ] **7. Lists and loops.** `List<T>`, `[a, b]`, `[]` with declared type, indexing, index assignment `xs[i] = v` (nested: `xs[i].f = v`), `len`, `push`, `for x in list`, `for i in 0..n`, `break`, `continue`.
  Done: an out-of-bounds runtime error. Copy tests: after `b = a`, a change to either leaves the other unchanged, for nested lists, structs holding lists and function arguments; changing `xs` inside `for x in xs` does not change the iteration.

- [ ] **8. Str, Bytes and Int built-ins.** The methods in language.md's built-ins table, and the `Bytes` type.
  Done: one test per method, and invalid UTF-8 in `to_str` is a runtime error.

- [ ] **9. Map.** `Map<K, V>` with `Int` or `Str` keys, `Map::new()` (a path call), `get`, `insert`, `remove`, `keys` in key order. Use `BTreeMap`.
  Done: a word-count function runs; `keys()` returns sorted keys; changing a value read with `get` leaves the map unchanged.

- [ ] **10. Capability types.** `CapTable`: capability types with method signatures, built by hand in tests for `Dir`, `Http`, `IoErr` and `HttpErr` as in language.md. `main` takes capability parameters. Checker rules 1 to 3, with one worked accept or reject example per rule in the spec.
  Done: one rejection test per escape in rule 1, including `Option<Dir>`, plus a forged-construction test.

- [ ] **11. Capability calls.** `Caps::call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Value`, where `cap` is the name of `main`'s parameter that reaches the call through helpers. An in-memory `StubDir` for tests.
  Done: a helper reads stub bytes through a passed `Dir`; a rejected program records zero stub calls.

- [ ] **12. Authority report.** As specified in language.md: map each helper's capability parameters back to the caller's arguments, and iterate until nothing changes so recursion terminates. Sort lines by function name. The spec gets worked examples before handoff.
  Done: report tests for direct calls, a two-level helper chain, mutual recursion, and a pure function (`none`).

- [ ] **13. Primer end to end.** The one-screen program in language.md checks, prints the documented report and runs against `StubDir`. Rust-ism rejection tests: `&str`, `String`, `Vec`, `.iter().map(|x| ..)`, `impl`, `let x: f64`.
  Done: those tests pass.

## Later, not in `lang`

Generating the `CapTable` from `caps.wit` (a `guest` build script using `wit-parser`), `guest`, `host` and the MCP `check` tool.

`guest` must measure the interpreter's stack at the depth budget on `wasm32` (about 4 MB natively in release), then set Wasmtime `max_wasm_stack` and the linker `-z stack-size` to cover it. A stack overflow must trap, never corrupt memory.

Optimization, when speed or the compiler needs it: replace the tree-walker with a bytecode VM, as Lua, CPython and Wasm engines do. Functions compile to flat instructions run by a loop; kio call frames live in a heap `Vec`, so Rust stack use no longer grows with recursion, and the depth budget becomes a plain call limit.
