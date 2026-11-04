# lang TODO

Build the `lang` crate: parser, checker, authority report and interpreter for the language in [language.md](language.md). Each task below is self-contained and can be handed to an agent as is.

## Rules for every task

- Read [language.md](language.md) first. It is the spec. If it is ambiguous, stop and ask; do not invent semantics.
- Rust std only. No dependencies, no macros beyond `derive`, no `unsafe`.
- Minimal code: straightforward control flow, no speculative abstractions, no features beyond the task. A human reviews every line.
- Comments only where the code cannot say it.
- Tests in `lang/tests/`, using inline source strings. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass, as CI runs them.
- Errors teach, as rustc does: the exact span, what is wrong, and a hint with the fix or the kio alternative (`Dir has: read, create`, `declare it with `let mut``). Never a panic. Tests assert hints, not just messages.

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
  - `run` evaluates `main` and returns its `Value` (`Int`, `Bool`, `Str`, `Unit`). Runtime errors are `RunErr { span, msg }`: overflow, including `i64::MIN / -1` and `-i64::MIN`, division or remainder by zero, and evaluation depth over 3_000 nested expressions, which bounds stack use for any program.
  Done: `fib(20)` returns 6765; a mutual-recursion test; a test where `false && 1 / 0 == 0` does not error; overflow, zero-division and depth runtime tests (1_000 recursive calls succeed; deep recursion and maximal nesting stop with an error on an 8 MB stack). Rejection tests, checking span and message: unknown name, unknown function, wrong arity, type mismatch, non-`Bool` condition, missing return, `if` branches of different types, assignment to a non-`mut` local, duplicate function.

Tasks 4 onward are outlines. Each gets a pinned spec, like tasks 1 to 3, before it is handed off. A task that spans parse, check and eval and grows large is split by pass.

- [x] **4a. Struct syntax.** Parser only.
  - Item `struct Name { f: T, .. }` (trailing comma allowed); `Item` gains a `Struct` variant. `parse` returns items in source order. Empty structs are allowed.
  - Literal `Name { f: e, .. }` as an `ExprKind::StructLit(name, Vec<(String, Expr)>)`. Field shorthand `Name { f }` is rejected with hint "write `f: f`".
  - A literal is not parsed directly in an `if` or `while` condition, as in Rust: `if x == P { a: 1 } { .. }` parses `P` as a name. Parenthesized, it is allowed.
  - `struct` stops being a later-task keyword.
  Done: parse tests for a declaration, a literal with nested literal, a field chain `a.b.c`, a literal in parentheses in an `if` head, and rejection of shorthand.

- [x] **4b. Struct types and values.** Checker and interpreter.
  - `Ty` becomes a non-`Copy` enum with `Ty::Struct(String)` and `Display` instead of `name()`.
  - Collect struct declarations before signatures. Errors: duplicate struct, duplicate field, unknown field type, a struct that contains itself directly or through other structs (no recursive types; use indexes into a `List` later).
  - Literal: every field exactly once, each with its declared type. Errors: unknown struct, unknown field, missing field (name it), duplicate field.
  - Field access `e.f` on a struct value. Error: no field `f` on that type, listing the fields.
  - Assignment target is a name or a chain of fields rooted at a `let mut` local: `p.a.b = v`. `+=` and `-=` work on fields too.
  - `==` and `!=` stay scalar-only.
  - `Value::Struct(String, Vec<Value>)`: struct name and field values in declaration order. `Program` keeps the struct declarations; eval finds a field's index by name in the declaration. No side tables. A bytecode VM can resolve indexes ahead of time later.
  - Copy semantics: `let b = a;` then changing `b.x` leaves `a.x` unchanged; a struct argument changed inside a callee leaves the caller's value unchanged.
  Done: tests for each error above, nested field read and write, and both copy tests.

Order: enums and `Result` first, because capability methods return `Result<_, IoErr>`; then capabilities, the core feature; then the remaining data types.

- [x] **5a. Enum and `match` syntax.** Parser only.
  - Item `enum Name { A, B(T, ..), .. }` (trailing comma allowed); `Item` gains `Enum`. Struct-like variants `A { f: T }` are rejected with hint "use `A(T)` or a struct payload".
  - Path expression `Name::Variant` as `ExprKind::Path(String, String)`. Payloads use the existing call: `Status::Due(5)` is `Call(Path, [5])`.
  - `match e { pat => expr, .. }`: arms separated by `,`, which is optional after a `{ .. }` arm body and allowed after the last arm. The scrutinee is a condition: no struct literal, as in `if`. `match` is block-like: no `;` needed as a statement.
  - Patterns, one level deep, as `Pat { kind, span }`: `_`; a name (binds it); an `Int` literal, optionally negative; a `Str`; `true` or `false`; `Enum::Variant`; `Enum::Variant(a, _, ..)` and `Variant(a, ..)` where each part is a name or `_`. A bare name like `None` parses as a name; the checker resolves it.
  - Rejected with kio hints: guards `pat if cond`, nested patterns `Some(Some(x))`, `..` in patterns.
  - `enum` and `match` stop being later-task keywords. The checker rejects enums, paths and `match` with "enums are not available yet" until 5b.
  Done: parse tests for a declaration, path, payload call, a `match` with every pattern kind, a block-bodied arm without `,`, and one rejection test per rejected form.

- [x] **5b. Enum types, values and `match`.** Checker and interpreter.
  - Declarations: collected with structs, before signatures. Errors: duplicate type name (struct or enum), duplicate variant, unknown payload type. Enums join the recursive-type check: an enum whose payloads reach itself is rejected.
  - `Ty::Enum(String)`. `Value::Enum(String, usize, Vec<Value>)`: enum name, variant index in declaration order, payload values. Eval finds the index by name in the declaration kept in `Program`, as for structs.
  - Expressions: `E::A` for a variant without payload; `E::B(x, ..)` as a call on a path, with arity and payload types checked. A payload variant without arguments, or arguments to a variant without payload, is an error.
  - `match`: the scrutinee is an `Int`, `Bool`, `Str` or enum. Every arm body has the same type, `Never` arms aside, as for `if`. Arms are tried in order; the first match runs.
  - Patterns: `_` and a name match anything (a name binds the value, scoped to its arm). Literals must match the scrutinee type. `E::V(..)` must name the scrutinee's enum; bare `V(..)` and a bare name equal to a variant of the scrutinee's enum resolve to that variant. Binding count must equal the payload count; each binding gets its payload type.
  - Exhaustiveness: an enum match is exhaustive if it has `_`, a binding name, or every variant. `Bool` needs both literals or a catch-all. `Int` and `Str` need a catch-all. Missing variants are named in the error. An arm after a catch-all is unreachable: an error.
  - `==` stays scalar-only.
  Done: a function matching an enum with payloads runs; bare variant patterns; tests for each error above, including non-exhaustive (naming the missing variant) and unreachable arm; enum copy test.

- [x] **5c. Errors that teach.** Add a hint to every `Diag` that lacks one; no new features. Wording below; keep each hint one line.
  - `unknown name`, `unknown function`, `unknown struct`, `unknown variant`, `no field`: "did you mean `x`?" naming the closest in-scope candidate by edit distance (at most 2 edits, or a third of the length); otherwise list what exists ("fields: a, b", "variants: A, B", or "declare it with `let`" for names). One small `closest(name, candidates)` helper in `check.rs`.
  - Duplicates (function, parameter, field, variant, type): "first defined here" is not expressible with one span, so the hint says "rename one of them".
  - `expected T, found U`: when one side is `()` and the other is not, hint "remove the trailing `;` to return the value" if the found `()` comes from a block whose last statement is an expression statement; otherwise no hint.
  - Arity: "`f` is declared as `fn f(a: Int, b: Str)`" (for variants, "`E::V` holds (Int, Str)").
  - `cannot be applied to`: list what the operator accepts, e.g. "`+` works on two Int or two Str".
  - `if` / `match` branches differ: "every branch must have the same type".
  - Non-exhaustive: "add arms for the missing cases, or `_ => ..`".
  - `cannot match on T`: "match works on Int, Bool, Str and enums".
  - `missing field`: "add `b: ..`". `no main`: "add `fn main() { .. }`". `main` with parameters: "capabilities come later; remove the parameters".
  - `only functions can be called`: "call a function by name, like `f(x)`".
  - Parser `expected X`: hint only where the fix is clear: missing `;` gives "add `;`", missing `}` gives "a `{` is not closed", missing `)` likewise.
  - Lexer: unterminated string gives "add a closing `\"`"; integer too large gives "Int is 64-bit: at most 9223372036854775807".
  Done: every existing rejection test asserts its hint; `closest` has its own tests; no `Diag` is built with `None` hint except where this list says "otherwise no hint".

- [x] **6. Option, Result and `?`.** Checker and interpreter; the parser change is only `?` as a postfix operator (`ExprKind::Try`), binding like `.` and `[]`.
  - Types: `Ty::Option(Box<Ty>)`, `Ty::Result(Box<Ty>, Box<Ty>)`, resolved from `Option<T>` and `Result<T, E>`; wrong arity of type arguments is an error. `Display` prints them as written. This is the shape `List<T>` and `Map<K, V>` reuse.
  - Values: `Some`, `None`, `Ok`, `Err` are `Value::Enum` with enum names `Option` and `Result` and variant indexes `None 0, Some 1` and `Ok 0, Err 1`. No new `Value` variant. Users cannot declare types named `Option` or `Result`.
  - Constructors are bare names: `Some(e)`, `None`, `Ok(e)`, `Err(e)`. Patterns: `Some(x)`, `None`, `Ok(x)`, `Err(e)`, and `Option::Some(x)` etc.
  - Expected types: `expr` takes `expected: Option<&Ty>`, passed down from a `let` type annotation, a parameter type at a call, a struct field or payload type, a return type (`return e` and the body's tail), and both `if` and `match` branches. `None` needs an expected `Option<T>`; `Ok(e)` needs an expected `Result<T, E>` and checks `e: T`; `Err(e)` likewise checks `e: E`. `Some(e)` without an expected type is `Option<type of e>`. Without an expected type, `None`, `Ok` and `Err` are errors with hint "add a type: `let x: Option<Int> = None;`". No inference beyond this.
  - `e?`: on `Option<T>` inside a function returning `Option<_>`, or on `Result<T, E>` inside a function returning `Result<_, E>` with the same `E`, it has type `T` and returns `None` or `Err(e)` early. Any other combination is rejected with language.md's message and hint. No conversion between error types.
  - `==` stays scalar-only.
  Done: `?` propagates through two calls on both types; `?` on `Result` in an `Option` function and on mismatched error types is rejected with hints; `None` without a type is rejected with its hint; `let x: Option<Int> = None;`, `return None;` and `Ok(())` in `-> Result<(), E>` are accepted; a `match` on `Option` and on `Result` runs.

- [ ] **7. Methods and `Bytes`.** Checker and interpreter; no parser change.
  - Built-in methods: one table in `check.rs`, `fn method(recv: &Ty, name: &str) -> Option<(Vec<Ty>, Ty)>`, giving parameter and return types. Eval dispatches on `(value, name)` in one `match`. Task 8's capability methods are typed the same way, from `CapTable`.
  - This task's methods: `Int.to_str() -> Str`, `Str.len() -> Int` (bytes), `Str.to_bytes() -> Bytes`, `Bytes.len() -> Int`, `Bytes.to_str() -> Str`. Invalid UTF-8 in `to_str` is the runtime error "invalid UTF-8".
  - `Ty::Bytes`, `Value::Bytes(Vec<u8>)`. No `Bytes` literal. `==` works on `Bytes`.
  - Errors with hints: unknown method lists the type's methods ("Str has: len, to_bytes"), with "did you mean"; wrong arity shows the signature; methods on a type with none: "Bool has no methods".
  - The receiver is evaluated first, then the arguments left to right.
  Done: a `Str` to `Bytes` to `Str` round trip with non-ASCII text; `len` on both; unknown-method and arity rejection tests assert hints. Kio code cannot produce invalid UTF-8 yet, so that runtime test moves to task 9, where a stub `Dir` returns raw bytes.

- [ ] **8. Capability types.** Checker only; no eval change.
  - `CapTable { caps: Vec<CapType> }`, `CapType { name, methods: Vec<(String, Vec<Ty>, Ty)> }`, plus the error enums it needs, `IoErr` and `HttpErr`, declared as ordinary enums the checker adds before user types. `CapTable::default()` stays empty. A test helper builds `Dir`, `Http`, `IoErr`, `HttpErr` as in language.md. `Ty::Cap(String)`.
  - Rule 1, one rule for every escape: a capability-typed expression may appear only as a parameter name used directly as a method receiver (`d.read(p)`) or as a whole call argument (`helper(d)`). Anywhere else is an error at that expression. So these are rejected:
    ```
    let d = input;                         capability `input` bound to local `d`; hint: pass `input` directly instead
    struct Job { dir: Dir }                capability type in a field; hint: capabilities are parameters only
    enum E { A(Dir) }                      same, for a payload
    fn pick(a: Dir) -> Dir                 cannot return `Dir` from `pick`; hint: capabilities are parameters only
    fn f(x: Option<Dir>)                   capability nested in `Option<Dir>`; hint: take `Dir` as a parameter
    (if c { a } else { b }).read("x")      capability used as a value; hint: call methods on the parameter itself
    Some(d), Job { dir: d }, d == e        same
    ```
    The type rule covers fields, payloads, returns and nesting; the expression rule covers locals, branches and constructors. Parameters typed as a capability are the only capability values.
  - Rule 2: there is no syntax that produces a capability. `Dir` as an expression, `Dir::new()`, `Dir {}` are unknown-name, unknown-variant or unknown-struct errors; add the hint "capabilities come only as parameters of `main`" when the name is a capability type.
  - Rule 3: `main` takes only capability parameters, and returns `()` or `Result<(), E>`. Other parameter types: "`main` parameters must be capabilities, like `input: Dir`". This replaces "`main` takes no parameters yet".
  - Method calls on a capability are typed from `CapTable`, exactly like task 7's built-in methods, with the same unknown-method and arity hints ("Dir has: read, create").
  - Rule 1 also bans assignment to a capability parameter: parameters are never `mut`, so this already holds.
  Done: one rejection test per line in the rule 1 table, asserting message and hint; rule 2 and rule 3 tests; accepted: `main(input: Dir, out: Dir)` passing both to a helper that calls `read` and `create`; a capability passed through two helpers.

- [ ] **9. Capability calls.** `Caps::call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Value`, where `cap` is the name of `main`'s parameter that reaches the call through helpers. An in-memory `StubDir` for tests.
  Done: a helper reads stub bytes through a passed `Dir`; a rejected program records zero stub calls; `to_str` on invalid UTF-8 from the stub is the runtime error "invalid UTF-8".

- [ ] **10. Authority report.** As specified in language.md: map each helper's capability parameters back to the caller's arguments, and iterate until nothing changes so recursion terminates. Sort lines by function name. The spec gets worked examples before handoff.
  Done: report tests for direct calls, a two-level helper chain, mutual recursion, and a pure function (`none`).

- [ ] **11. Lists and loops.** `List<T>`, `[a, b]`, `[]` with declared type, indexing, index assignment `xs[i] = v` (nested: `xs[i].f = v`), `len`, `push`, `for x in list`, `for i in 0..n`, `break`, `continue`.
  Done: an out-of-bounds runtime error. Copy tests: after `b = a`, a change to either leaves the other unchanged, for nested lists, structs holding lists and function arguments; changing `xs` inside `for x in xs` does not change the iteration. Capability escape tests for `List<Dir>` and `[input, out]`.

- [ ] **12. Map.** `Map<K, V>` with `Int` or `Str` keys, `Map::new()` (a path call), `get`, `insert`, `remove`, `keys` in key order. Use `BTreeMap`.
  Done: a word-count function runs; `keys()` returns sorted keys; changing a value read with `get` leaves the map unchanged.

- [ ] **13. Remaining built-ins.** The other `Str`, `Int` and `List` methods in language.md.
  Done: one test per method.

- [ ] **14. Primer end to end.** The one-screen program in language.md checks, prints the documented report and runs against `StubDir`. Rust-ism rejection tests: `&str`, `String`, `Vec`, `.iter().map(|x| ..)`, `impl`, `let x: f64`.
  Done: those tests pass.

## Later, not in `lang`

Generating the `CapTable` from `caps.wit` (a `guest` build script using `wit-parser`), `guest`, `host` and the MCP `check` tool.

`guest` must measure the interpreter's stack at the depth budget on `wasm32` (about 3 MB natively in release), then set Wasmtime `max_wasm_stack` and the linker `-z stack-size` to cover it. A stack overflow must trap, never corrupt memory.

Optimization, when speed or the compiler needs it: replace the tree-walker with a bytecode VM, as Lua, CPython and Wasm engines do. Functions compile to flat instructions run by a loop; kio call frames live in a heap `Vec`, so Rust stack use no longer grows with recursion, and the depth budget becomes a plain call limit.
