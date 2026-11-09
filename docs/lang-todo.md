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

- [x] **7. Methods and `Bytes`.** Checker and interpreter; no parser change.
  - Built-in methods: one table in `check.rs`, `fn method(recv: &Ty, name: &str) -> Option<(Vec<Ty>, Ty)>`, giving parameter and return types. Eval dispatches on `(value, name)` in one `match`. Task 8's capability methods are typed the same way, from `CapTable`.
  - This task's methods: `Int.to_str() -> Str`, `Str.len() -> Int` (bytes), `Str.to_bytes() -> Bytes`, `Bytes.len() -> Int`, `Bytes.to_str() -> Str`. Invalid UTF-8 in `to_str` is the runtime error "invalid UTF-8".
  - `Ty::Bytes`, `Value::Bytes(Vec<u8>)`. No `Bytes` literal. `==` works on `Bytes`.
  - Errors with hints: unknown method lists the type's methods ("Str has: len, to_bytes"), with "did you mean"; wrong arity shows the signature; methods on a type with none: "Bool has no methods".
  - The receiver is evaluated first, then the arguments left to right.
  Done: a `Str` to `Bytes` to `Str` round trip with non-ASCII text; `len` on both; unknown-method and arity rejection tests assert hints. Kio code cannot produce invalid UTF-8 yet, so that runtime test moves to task 9, where a stub `Dir` returns raw bytes.

- [x] **8. Capability types.** Checker only; no eval change.
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

- [x] **9. Capability calls.** Interpreter; `run` gains capability arguments.
  - `Value::Cap(String)` holds the name of the `main` parameter it came from. Helpers receive it as an ordinary argument; rule 1 guarantees it is only ever passed or called, never stored or built. `Value` must stay 64 bytes.
  - `run(p, caps)` binds each `main` parameter `name` to `Value::Cap(name)`. A program whose `main` has capability parameters needs those names: `Caps::has(&self, name: &str, ty: &str) -> bool` is checked for every parameter before evaluation starts; a missing or mistyped grant is `RunErr` "no grant for `input: Dir`" and no code runs.
  - `Caps::call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String>`. `Ok` is the method's return value, which must have the declared type (for `Dir.read`, a `Value::Enum("Result", ..)`); `Err(msg)` is a host failure, which becomes `RunErr` and stops the run. Arguments are evaluated left to right before the call.
  - `StubDir` in `lang/tests/` (not in `src`): a `BTreeMap<String, Vec<u8>>` of files per granted name, recording every call as `(cap, method, path)`. `read` of a missing file returns `Err(IoErr::NotFound)`; `create` stores the bytes.
  - The temporary runner keeps passing an empty `Caps`; a program asking for capabilities there fails with the no-grant error.
  Done: a helper reads stub bytes through a passed `Dir` and `main` writes a result with `create`; the stub records exactly those calls with the `main` parameter names; a rejected program records zero calls; a missing grant runs no code; `to_str` on invalid UTF-8 read from the stub is the runtime error "invalid UTF-8"; `?` on a stub `NotFound` propagates.

- [x] **10. Authority report.** `report.rs`: `pub fn report(p: &Program) -> String`. Reads only the checked AST in `Program`; no evaluation.
  - For each function `f` and each capability parameter `d` of `f`, `uses(f, d)` is the set of method names reachable on `d`: every `d.m(..)` anywhere in `f`'s body (including untaken branches), plus `uses(g, e)` for every call `g(.., d, ..)` passing `d` as `g`'s parameter `e`.
  - Compute it as a fixed point: start every set empty, repeat over all functions until no set grows. This terminates because sets only grow and are bounded by the method names.
  - Output: one line per function, sorted by name, the name padded to the longest name plus one, then `: `, then `d m1 m2, e m3` with parameters in declaration order and methods sorted, or `none` when the function has no capability parameter or none is used. A parameter with no uses is omitted. Example:
    ```
    fn load(d: Dir) -> Result<Bytes, IoErr> { d.read("a") }
    fn save(o: Dir, b: Bytes) -> Result<(), IoErr> { o.create("b", b) }
    fn main(input: Dir, out: Dir) -> Result<(), IoErr> { let b = load(input)?; save(out, b) }
    fn pure(n: Int) -> Int { n }

    load : d read
    main : input read, out create
    pure : none
    save : o create
    ```
  - The runner prints the report after a successful check, before running.
  Done: tests for the example above; a two-level helper chain; mutual recursion passing a capability back and forth (terminates, both get the union); an unused capability parameter (omitted, `none` if it was the only one); a method in an untaken branch (still reported); the README example's report matches exactly.

- [x] **11a. Lists.** Parser, checker and interpreter.
  - Parser: list literal `[a, b, ..]` and `[]` as `ExprKind::List(Vec<Expr>)` (trailing comma allowed). Indexing and index assignment already parse.
  - `Ty::List(Box<Ty>)` from `List<T>`; `Value::List(Vec<Value>)`. `Value` stays 64 bytes.
  - `[]` needs an expected `List<T>` (same rule as `None`, same hint style: "add a type: `let xs: List<Int> = [];`"). `[a, ..]` takes its element type from the expected type if any, else from the first element; every element must have it.
  - `xs[i]`: `xs: List<T>`, `i: Int`, type `T`. Out of bounds (including negative) is the runtime error "index 5 out of bounds for length 3".
  - Assignment targets: a place is a `let mut` local followed by any chain of `.f` and `[i]` (`xs[i] = v`, `xs[i].f = v`, `p.items[0] = v`). Extend the checker's `place` and eval's `assign` path walk; the index is evaluated before the value.
  - Methods: `List.len() -> Int`; `xs.push(v)` on a place, with `v: T`, returning `()`. `push` on a non-place (e.g. `f().push(1)`) is rejected: "`push` changes a list; call it on a `let mut` variable".
  - `==` stays scalar-only, so lists are not comparable yet.
  - Capabilities: `List<Dir>` is rejected by the type rule and `[input, out]` by the expression rule, with no new code; add tests.
  Done: literal, indexing, nested index-field assignment, `push`, `len`, out-of-bounds (with span), `[]` without a type rejected with its hint; copy tests (`b = a` then changing either leaves the other unchanged, for nested lists, a struct holding a list, and a list argument changed in a callee); `List<Dir>` and `[input, out]` rejected.

- [x] **11b. Loops.** Parser, checker and interpreter.
  - `for x in e { .. }` where `e: List<T>`; `x: T` is bound per iteration, not `mut`. Iteration is over the value of `e` when the loop starts, so changing the list inside the body does not change the iteration.
  - `for i in a..b { .. }` with `a, b: Int`; `i` takes `a, a+1, .., b-1`; empty if `a >= b`. `..` only in this position; elsewhere it is a parse error with hint "ranges are only for `for` loops".
  - `break` and `continue` as statements, inside `while` and `for` only. Outside a loop: "`break` outside a loop", hint "use `return` to leave a function". No labels, no `break value`.
  - Eval: `Stop` gains `Break` and `Continue`; loops catch them, function calls never see them (the checker guarantees it).
  - The loop body is `()`; `for` and `while` are statements with type `()`.
  Done: sum of a list; a range loop; `break` and `continue` in both loop kinds; nested loops where `break` leaves only the inner loop; changing `xs` inside `for x in xs` does not change the iteration; `break` outside a loop is rejected with its hint.

- [x] **12. Map.** Checker and interpreter; `Map::new()` already parses as a call on a path.
  - `Ty::Map(Box<Ty>, Box<Ty>)` from `Map<K, V>`; `K` must be `Int` or `Str` ("Map keys must be Int or Str"). `Value::Map(BTreeMap<Key, Value>)` with `enum Key { Int(i64), Str(String) }`, so iteration is in key order. `Value` must stay 64 bytes; box the map if it does not fit.
  - `Map::new()` needs an expected `Map<K, V>`, like `[]`; hint "add a type: `let m: Map<Str, Int> = Map::new();`".
  - Methods: `get(K) -> Option<V>`, `insert(K, V)` and `remove(K)` on a place (like `push`; `insert` replaces an existing value), `len() -> Int`, `keys() -> List<K>` in key order.
  - `Program::fits` gains the `Map` case. `Map` of a capability is rejected by the existing type rule; add a test.
  Done: a word-count function (`for w in words { .. insert(w, n + 1) }`) runs; `keys()` returns sorted keys for both key types; `get` on a missing key is `None`; changing a value read with `get` leaves the map unchanged; `Map<Bool, Int>` and `Map<Str, Dir>` are rejected with hints.

- [x] **13. Remaining built-ins.** Checker method table and eval only.
  - `Str.split(Str) -> List<Str>`, as Rust's `str::split`; an empty separator is the runtime error "`split` needs a non-empty separator".
  - `Str.lines() -> List<Str>`, as Rust's `str::lines` (`\n` or `\r\n`, no trailing empty line).
  - `Str.trim() -> Str`, as Rust's `str::trim`.
  - `Str.contains(Str) -> Bool`, `Str.starts_with(Str) -> Bool`.
  - `Str.parse_int() -> Option<Int>`: an optional `-` or `+` and decimal digits, as Rust's `i64::from_str`; anything else, including overflow, is `None`.
  - Add each to `METHODS` so hints list them.
  - Check language.md's built-ins table matches what exists after this task, and fix the table, not the code, if they differ.
  Done: one test per method, including the edge cases above; `examples/words.kio`, a word count using `split`, `Map` and `for`, runs and is listed in the README.

- [x] **14. Primer end to end.** The one-screen program in language.md checks, prints the documented report and runs against `StubDir`. Rust-ism rejection tests: `&str`, `String`, `Vec`, `.iter().map(|x| ..)`, `impl`, `let x: f64`.
  Done: those tests pass.

## Performance

- [x] **P1. Borrowing reads.** Every variable read clones the whole value, so `xs[i]`, `.len()` and `m.get(k)` copy the receiver, and a loop that indexes a list is quadratic: 20,000 elements take 1.8 s and 40,000 take 7.0 s. Evaluate these on the variable in place instead of on a clone. No change to what programs mean.
  Done: the indexing loop over 40,000 elements runs in 0.03 s in release (was 7.3 s); all copy tests still pass. Reads that could see a write from their own operands (a block in an index or argument) keep the copying path.

- [x] **L1. Recursive types through `List` and `Map`.** A type may contain itself through `List<T>` or `Map<K, V>`, which hold their items on the heap, so `enum Json { Arr(List<Json>), Obj(Map<Str, Json>), .. }` is accepted. Direct fields, `Option` and `Result` stay rejected (the value would be infinite). Only the recursion check changes; values, display and `Program::fits` already handle nesting.
  Done: a `Json` enum builds and prints a nested value; a JSON encoder written in kio over it runs; `struct Node { next: Option<Node> }` and `struct A { b: B } struct B { a: A }` are still rejected with their hint.

- [ ] **P2. Copy-on-write values.** `design.md` memory model step 2. W12 measured the need: json.kio's parser passes the input `Bytes` to each helper, so every call copies it and parsing is quadratic (10 KB in 0.13 s and about 1e9 fuel, 100 KB in 1.7 s and about 64e9). Two tasks; no change to what programs mean.
  - [x] **P2a. `Str` and `Bytes`.** `Value::Str(Rc<str>)` and `Value::Bytes(Rc<Vec<u8>>)` (or `Rc<[u8]>` where never mutated in place). Cloning a value shares the storage. A write (`b.push(x)`, `s += t` on a `let mut` local) copies only when shared: `Rc::make_mut`. Construction and display are unchanged; host conversion in `guest` adapts (`Value` is public). Keep it mechanical: no new optimizations, no change to `List`/`Map`.
    Done: all tests pass unchanged (the copy-semantics tests are the guard); a new test shows a `Bytes` pushed to after being passed to a function leaves the caller's copy unchanged, and the same for `Str`; kio-web's `json_test.kio` passes; parse of a 100 KB JSON array (as W12) re-measured with `kio run`: time and the fuel it needs, recorded here and in kio-web's README.
    Result: `Value::Str(Rc<str>)`, `Value::Bytes(Rc<Vec<u8>>)`. Parse of 10 KB: 0.13 s to 0.08 s, fuel 1e9 to 0.55e9; 100 KB: 1.7 s to 0.58 s, fuel 64e9 to 5.5e9 (linear now). `fib` and `loop` unchanged.
  - [x] **P2b. `List` and `Map`.** The same for `Value::List(Rc<Vec<Value>>)` and `Value::Map(Rc<BTreeMap<..>>)`, including `xs[i] = v` and `m.insert` copying first when shared. After P2a, measured again. Result: passing a 10,000-element `List` to a helper went from 100 us to 2 us per call (2.0 s to 0.05 s for 20,000 calls); JSON parse, fib and loop unchanged. Reads use `view`, so only writes copy a shared list.

- [ ] **L2. Small gaps found by json.kio (W12).** Each only accepts more programs; no change to existing ones. One commit each, with tests and docs/language.md updated.
  - [x] **L2a. String escapes** `\r` and `\0`, as Rust. `\u{..}` stays unknown, with a hint naming the two new escapes plus `\n \t \" \\`.
  - [x] **L2b. `Option` and `Result` methods.** `is_some()`, `is_none()` on `Option`; `is_ok()`, `is_err()`, `unwrap_or(T)` on `Result`. Built-ins as `unwrap_or` is today (check.rs `builtin`, eval.rs `builtin_method`).
  - [x] **L2c. `==` and `!=` on structs, enums, `Option`, `Result`, `List` and `Map`**, when both sides have the same type and that type contains no capability anywhere (a struct field, payload or element). Structural, as the derived `PartialEq` on `Value`. Capability types stay rejected, so a program cannot compare grants. Error for a capability: "cannot compare `Dir` values" with hint "capabilities have no equality".
  - [x] **L2d. `loop`.** `loop { .. }` with `break` and `continue`, as Rust; a `loop` with no `break` has type `Never`, so `fn f() -> Int { loop { return 1; } }` checks without a tail. `break` with a value is not in v0 (error with hint "use a `let mut` and `break`"). `loop` is removed from the reserved words.
  Not here: constants (they interact with "no globals"; logged in open_questions for the owner).

- [ ] **L3. Small gaps found by the notes app (W9a).** Each only accepts more programs. One commit each, with tests and docs/language.md updated.
  - [x] **L3a. `return`, `break` and `continue` as a match arm.** `Some(_) => return create(r, notes),` and `None => break,` parse, as in Rust. Parser only: an arm body starting with `return`, `break` or `continue` parses that statement (`return` takes an optional expression, ended by `,` or `}`) and becomes a block holding it, so the arm has type `Never` and checker, IR and both backends are unchanged. `break` or `continue` outside a loop keeps its existing error. Done: each form runs in a `match` inside a loop and a function, including `return` with no value in a `-> ()` function; a `match` where every arm returns needs no tail; `break` outside a loop in an arm is rejected with its hint; the parse tree equals the block form's.
    Result: the parser wraps the jump in a one-statement block; `return_stmt` and `jump_stmt` stop before their terminator, so arms and statements share them.
  - [x] **L3b. Modules use each other.** A module may use another declared module's `pub` items as `m::item` (functions, structs, enums, variants, in types, expressions and patterns), so kio-web's `web.kio` can name `json::Json`. Only modules the root declares; the root's own items stay invisible to modules (`ROOT_ITEM` hint unchanged); a module naming itself (`web::f` inside web.kio) is accepted and means its own item. Cycles (web uses json, json uses web) are allowed: signatures are collected before bodies. The recursive-type check already works on keys across modules. Errors: `m::item` for an undeclared `m` is "unknown module `m`" with hint "declare it with `mod m;` in the root file"; a private item keeps `check_public`'s error. Update open_questions #24 and language.md ("Modules"). Done: `modules_cannot_reach_each_other_or_the_root` becomes two tests (siblings accepted, root still rejected); a struct in one module holding another module's enum; two modules calling each other; a private sibling item rejected with its hint; the report names both modules' functions.
    Result: `Scope::member` no longer rejects modules; `Scope::public` lets a module name its own private items as `m::item`. `app::g` inside a module is now "unknown module `app`".

- [x] **L4. Gaps found by kio-web's notes apps on Dir and Kv (2026-11-08).** Each only accepts more programs, with Rust's meaning. One commit each, tests asserting hints, docs/language.md updated.
  - [x] **L4a. Nested patterns and `if let`.** A pattern's parts may be any pattern, at any depth: `Err(IoErr::Exists)`, `Ok(true)`, `Some(0)`, `Some(Some(x))`, `Shape::Line(1, _)`, `"GET"` inside a variant. `if let PAT = e { .. } else { .. }` (else optional; without it the type is `()`; `else if` chains as for `if`); the scrutinee is a condition (no struct literal). IR: `Pat::Variant(u32, Vec<Pat>)` becomes recursive (a binding is a leaf `Pat::Bind`); `if let` lowers to `Match` with a `_` arm, so codegen sees no new node; the interpreter matches recursively, binding left to right. Exhaustiveness and unreachable arms by the usefulness algorithm (Maranget, "Warnings for pattern matching") over constructors: each enum (user, `Option`, `Result`, capability error enums) and `Bool` are finite; `Int` and `Str` need a catch-all at that position. Non-exhaustive names one missing pattern: "missing `Err(IoErr::Denied)`", hint "add an arm for it, or `_ => ..`"; an arm no earlier arm leaves room for is "unreachable arm". Codegen does not implement `Match` yet (C3d), so only the IR shape changes for it. Done: each example above runs; non-exhaustive nested matches name the missing pattern; an unreachable nested arm is rejected; `if let` with and without `else`, and in an `else if` chain; bindings in nested positions have the right types; the parse and lowering snapshots for a nested pattern; every existing test unchanged.
    Result: `ir::Pat::Variant(u32, Vec<Pat>)`; `if let` lowers to `Match` with a `_` arm; `Usefulness` in check.rs (Maranget, with witnesses, about 150 lines) capped at 4 M cells and 256 columns (open_questions #58, #59). Also now rejected: a name bound twice in one pattern, a repeated literal or variant arm (unreachable).
  - [x] **L4b. Built-ins.** `Str.ends_with(Str) -> Bool`; `Str.strip_prefix(Str) -> Option<Str>` and `strip_suffix`; `Str.find(Str) -> Option<Int>` (byte offset of the first match, as Rust's `str::find`); `Str.replace(Str, Str) -> Str` (as Rust's `str::replace`, including an empty pattern); `List.contains(T) -> Bool` for `T` with `==` (not containing a capability); `List.sort()` on a place, for `List<Int>` and `List<Str>` (bytes order), stable, other element types rejected with hint "sort works on List<Int> and List<Str>"; `List.pop() -> Option<T>` on a place. Each in check.rs's method table (so hints list it) and eval.rs; IR `Builtin`/`Mut` variants; codegen leaves them `unimplemented!` (the harness counts them) until the compiler task that adds their runtime exports. Done: one test per method with Rust's edge cases (empty strings, no match, overlapping `replace`, sort of an empty list, pop of an empty list), the rejections with hints, language.md's built-ins table.
    Result: IR `Builtin` gains `StrEndsWith`, `StrStripPrefix`, `StrStripSuffix`, `StrFind`, `StrReplace`, `ListContains`; `Mut` gains `ListSort`, `ListPop` (`pop` is a mutation with a value). Codegen: unimplemented, counted by the harness.

- [x] **L5a. `Result.ok()`, `Result.err()`, `Option.ok_or(E)`.** As Rust: `ok() -> Option<T>`, `err() -> Option<E>`, `ok_or(e) -> Result<T, E>` where `E` comes from the argument's type. Built-ins in check.rs's table and eval.rs, new IR `Builtin` variants, codegen `unimplemented!` until its compiler task. Found by kio-web (`load` keeps `match json::parse(..) { Ok(j) => Some(j), Err(_) => None }`). Done: one test each, including `?` on `x.ok_or(e)` in a function returning `Result<_, E>`; language.md's table.
    Result: `Builtin::ResultOk`, `ResultErr`, `OptionOkOr`; `ok_or` is typed in `method_call` (its `E` is the argument's type), not the table.

- [ ] **L6. Function values** (owner decision 2026-11-08, proposals/abstraction.md 1A). Interpreter first; compiled support is a later compiler task (codegen leaves the new IR `unimplemented!`).
  - [x] **L6a. `fn` types and function items as values.** Type `fn(T, ..) -> R` (`-> R` optional for `()`), usable anywhere a type is: parameters, returns, locals, struct fields, enum payloads, `List`/`Option` elements. A function's name (`double`, `web::f`) not followed by `(` is a value of its type. A call `f(x)` where `f` is a local or parameter of `fn` type calls it; `(e)(x)` calls any expression of `fn` type (so `(r.handler)(req)` works). Rules: a `fn` type may not mention a capability anywhere ("function values cannot take or return capabilities", hint "pass the capability to a named function instead"); a function with a capability parameter cannot be used as a value (same message, at the name); `fn` values have no `==` ("functions have no equality") and are not `Map` keys; `main` cannot be used as a value. Display: `<fn name>`. The authority report is unchanged: a function value carries no authority. IR: `ExprKind::FnRef(FuncId)`, `ExprKind::CallValue(Box<Expr>, Vec<Expr>)`; `Value::Fn(FuncId)`; `Program::fits` rejects `Fn` from a host. Errors teach: calling a non-function ("`x` is Int, not a function"), arity and argument types against the `fn` type. Done: passing `double` to `apply`; a route table `List<Route>` with a `handler: fn(Request) -> Response` field called as `(r.handler)(req)`; a function returning a function; a recursive function used as a value; each rejection with its hint; lowering snapshot; language.md. Result: interpreter only (codegen `unimplemented!`, harness counts it as a skip); a local hides a function of the same name; `List.contains` on functions is also rejected; open_questions #64.
  - [ ] **L6b. Closures.** `|x: Int| x + n`, `|a, b| { .. }`: parameter types from the annotation, else from the expected `fn` type (a parameter, field, `let` annotation or built-in argument); without either, "add a type: `|x: Int| ..`". The return type is the body's. Captures: every outer local the body uses is copied into the closure when it is created (values are copied anyway, so later changes to the local are not seen, and the closure cannot change it: assigning to a capture is "`n` is captured by copy and cannot be assigned", hint "return the new value instead"). A closure may not capture a capability ("closure captures capability `out`", hint "take it as a parameter: `|out: Dir, ..|`" — which the `fn` type rule then rejects, so the real fix is a named function; say so in language.md). `return` inside a closure returns from it; `break`/`continue` cannot cross it. The lexer's `|` hint ("no closures") changes: `|` starts a closure in expression position; `||` stays logical or, and `|| e` is a closure with no parameters only where a value is expected (as Rust: in expression-start position). Lowering lifts each closure to a new function whose leading parameters are its captures; IR `ExprKind::Closure(FuncId, Vec<LocalId>)`; `Value::Closure(FuncId, Rc<[Value]>)`; `CallValue` calls both. Display `<closure>`. Done: capture by copy (a later change to `n` not seen); a closure in a struct field and a list; a closure returning a closure; each rejection with its hint; nested closures capturing through each other; language.md.
  - [ ] **L6c. Built-ins that take functions.** `Option.map(f)`, `Option.and_then(f)`, `Result.map(f)`, `Result.map_err(f)`, `Result.and_then(f)`, `List.map(f) -> List<U>`, `List.filter(f) -> List<T>`, `List.any(f) -> Bool`, `List.all(f) -> Bool`, typed by the checker from `f`'s type (the method table cannot express `U`: special-case as `ok_or` is). Closures passed directly get their parameter types from the receiver. Done: each method, chained `xs.filter(|x| x > 0).map(|x| x * 2)`, `json::get(j, "title").and_then(json::as_str)` (the W9a case), language.md.
- [ ] **L7. Generics** (owner decision 2026-11-08, proposals/abstraction.md 2A): type parameters on functions, structs and enums, monomorphized in lowering; bounds `T: Eq` and `T: Ord` only. Pinned after L6.

- [ ] **L8. `Float`** (owner decision 2026-11-08, proposals/abstraction.md 3). `Float` is IEEE 754 binary64, Rust's `f64`, in every respect below. Interpreter first; codegen leaves it `unimplemented!`.
  - Literals: digits `.` digits (`1.5`, `2.0`), with an optional exponent (`1e9`, `1.5e-3`, `2E+4`); `1.` and `.5` are errors with hint "write `1.0`" / "write `0.5`"; `0..n` stays a range. A literal that is not finite as `f64` is an error. The lexer's "kio has no floats" error goes.
  - Operators: `+ - * / %` (Rust's `%` on `f64`), unary `-`, `< <= > >=`, `== !=`, `+=`/`-=`, on two `Float`s. IEEE results: no runtime error for overflow, division by zero or NaN. `Int` and `Float` never mix: `1 + 2.0` is "cannot add Int and Float", hint "convert with `.to_float()` or `.to_int()`".
  - Equality is IEEE (`NaN != NaN`, `0.0 == -0.0`), including inside structs, enums, `Option`, `List` (as Rust's derived `PartialEq`). Not a `Map` key ("Map keys must be Int or Str" unchanged). `match` on a `Float` or a `Float` literal pattern is rejected, hint "compare with `<` or `==`".
  - Methods: `Float.to_str()` exactly as Rust's `format!("{}", x)` (`1.0` prints `1`, `0.1 + 0.2` prints `0.30000000000000004`, `NaN`, `inf`, `-inf`, `-0`); `to_int() -> Option<Int>` (`None` if NaN, infinite or outside `i64` after truncation toward zero); `abs`, `floor`, `ceil`, `round` (Rust's: half away from zero), `sqrt`, `is_nan`; `Int.to_float()`; `Str.parse_float() -> Option<Float>` as Rust's `f64::from_str` (accepts `inf`, `NaN`, `1e3`). Display of a `Float` nested in a container is the same text.
  - IR: `Ty::Float`, float `BinOp`/`UnOp` cases kept distinct from `Int`'s where the interpreter or codegen differ; `Value::Float(f64)`; `Program::fits` (hosts never return floats today). caps.wit has no float, unchanged.
  Done: each literal form and literal error; each operator including NaN, infinities and `-0.0`; mixing rejected with the hint; equality inside a struct with NaN; `match` rejected; every method with Rust's edge cases; display of nested floats; language.md (scalars, operators, built-ins, "Not in v0" updated).

- [x] **P3. Lowering.** Resolve names, field and variant indexes into a typed form before evaluation, as design.md's `Program` describes. Prerequisite for the compiler; split into P3a-P3c in [compiler-todo.md](compiler-todo.md). Owner decision 2026-11-05: no bytecode VM; P3 leads to a Wasm backend.

## Later, not in `lang`

`guest`, `host` and the MCP `check` tool.

`guest` must measure the interpreter's stack at the depth budget on `wasm32` (about 3 MB natively in release), then set Wasmtime `max_wasm_stack` and the linker `-z stack-size` to cover it. A stack overflow must trap, never corrupt memory.

Optimization, when speed or the compiler needs it: replace the tree-walker with a bytecode VM, as Lua, CPython and Wasm engines do. Functions compile to flat instructions run by a loop; kio call frames live in a heap `Vec`, so Rust stack use no longer grows with recursion, and the depth budget becomes a plain call limit.
