# Grouping capabilities

Design only (maintainer-approved to write, 2026-11-19; A3 #1, F15; A2 friction 3.7). A change to language semantics is a one-way door: this note is written so the decision takes one reading.

## The problem

Rule 1 forbids a struct that holds a capability, so a function's authority is spelled out in its signature and every caller's. In A3, `booking::handle` and `route` take `db: Db, admin: Secret, rng: Rng, now: Clock, audit: Dir`, and so do six test helpers and 20 tests. Adding `admin: Secret` touched ~30 lines, by script. A2 (`db, secrets, imports, exports, now`) edited ~25 signatures and ~60 call sites the same way.

The report today (v0.1.5, A3 stage 4):

```
booking::handle : db exec query transaction, admin matches, rng bytes, now now_ms, audit append
tests::call     : db exec query transaction, admin matches, rng bytes, now now_ms, audit append
tests::provider : db exec query transaction, admin matches, rng bytes, now now_ms, audit append
```

The precision is real (each line is per capability, per method) and the cost is real (every line repeats the same five, because every helper passes everything through).

## What must survive

| Constraint | Today | Must still hold |
|---|---|---|
| Report per capability, per method, derived by the checker from checked bindings (P5, P8) | `handle : db exec query, admin matches` | never `env *`; every method charged to one capability |
| `kio check --base` grant lines are exact one-liners | `~ db: Db (+query)`, `+ admin: Secret (matches)` | the same lines for the same grants |
| The host never trusts the checker | the host binds grants to `main`'s IR parameters and checks every call itself | enforcement code and audit lines unchanged |
| No ambient authority | a function's authority is its capability parameters; closures cannot capture a capability | a library gains nothing it was not passed |
| Values copy | a capability is a handle, copied like an `Int` | nothing observable changes |
| Compiled code and the interpreter agree | both run one IR | no new IR |
| Review machinery (R1a4 `users`, R1a5 reach, R1b mutants) keeps working | follows a grant parameter to parameter | follows it through a group |

## Options

### A. A capability struct

A `struct` whose fields are all capabilities is a **capability struct**. It is a capability for every purpose of rule 1 (a parameter, passed whole, never stored, returned, captured, in a list or a `fn` type), with one more operation, a field read, and one way to build it, a literal in argument position from the caller's own parameters.

```
pub struct Env { db: Db, admin: Secret, rng: Rng, now: Clock, audit: Dir }

pub fn handle(env: Env, r: Req) -> Resp {
  if init(env.db).is_err() { return err(500, "database error"); }
  match route(env, r) { Ok(x) => x, Err(x) => x }
}

fn main(req: Incoming, res: Outgoing, db: Db, admin: Secret, rng: Rng, now: Clock, audit: Dir, ..) {
  let out = booking::handle(booking::Env { db: db, admin: admin, rng: rng, now: now, audit: audit }, r);
```

Report: a field is a capability of its own, named by its path.

```
main            : db exec query transaction, admin matches, rng bytes, now now_ms, audit append, ..   (unchanged)
booking::handle : env.db exec query transaction, env.admin matches, env.rng bytes, env.now now_ms, env.audit append
booking::init   : db exec query                                                                       (unchanged)
```

Lowering erases the struct: `env: Env` becomes five parameters, `env.db` the first of them, a whole `env` argument five arguments, a literal its five places. The IR, interpreter, codegen, runtime and host see today's program.

### B. Groups on `main` with no type

```
fn main(req: Incoming, res: Outgoing, #[group(env)] db: Db, #[group(env)] admin: Secret, ..)
fn handle(..env, r: Req) -> Resp          // or `env: group`
```

Report `handle : db exec query, admin matches` (the names are `main`'s). A helper's signature reads only with `main` in view; a module cannot declare the group it needs; a test cannot name it; two entries sharing a module (A3's `app.kio` and `remind.kio` both use `booking.kio`, with different `main`s) would have to agree on names. New syntax that is not Rust (`..env`) or a second attribute. It is A without the type, and the type is what makes helpers, modules and tests readable alone.

### C. Leave the language alone; tooling

`kio fix --param 'admin: Secret' handle` adds a parameter to a function, every caller and every test, through the modules; the report prints a pass-through helper as `tests::call : as booking::handle`. No language change. But the diff a reviewer reads still touches 30 lines per grant (R1a4's `changed:` marks them all), the friction recurs with every grant, and the agent already does this rewrite with a script: the cost F15 measured is in reading, not typing. The report line for pass-through helpers is worth doing under A too.

### D. Other capability languages

| Language | Grouping | Why kio cannot copy it |
|---|---|---|
| E, Pony, Joe-E, Caja | capabilities are ordinary object references; any object groups them | authority is found by reachability analysis, not read off a signature; kio's report precision comes from second-class capabilities |
| Austral | capabilities are linear records; a record of capabilities is a record | kio values copy, so a capability in a data record could be copied into a list or a closure; A's parameter-only struct is the non-linear analogue |
| Wyvern | a module `require`s its resources once; functions inside use them | per-module authority, not per function; a function's line would be its module's |
| WASI components | imports grouped by interface (`wasi:filesystem/types`), satisfied by the host | the world is `main`'s grant list; grouping below it is the program's business, as in A |

Nothing better than A; Austral's record-of-capabilities is A under linearity.

### Cost

| | A | B | C |
|---|---|---|---|
| Checker | struct classification; field read and literal as capability places; 8 new errors; tests flattened | attribute, splat syntax, group scope across files | none |
| Lowering | one flattening pass, no new IR | the same | none |
| Codegen, runtime, host | none | none | none |
| Report | paths as capability names; charging through a whole pass and a literal; `passed`, `users`, `param_ty` follow leaves | names from `main` | `as F` line |
| Review | `users` and `passed` follow leaves; tokens and reach unchanged | the same | none |
| Docs | language.md rule 1 and report; guide.md tests | the same | guide.md |
| What a reader gains | a signature names the group once; a module declares what it needs | little | nothing |

## Recommendation: A

### Rule 1, revised

> **Capabilities are parameters only.** A capability is a capability type (`Dir`, `Db`, ..) or a **capability struct**: a `struct` whose fields are all capability types. A capability can be received as a parameter, have a method called on it, or be passed whole as an argument; a capability struct can also have a field read, `env.db`, which is a capability wherever a capability name may stand. A capability struct is built only by a literal that is a whole argument, each value a capability parameter of the caller or a field of one: `handle(Env { db: db, now: now }, r)`, `narrow(Writer { db: env.db, audit: env.audit }, r)`. Everything rejected today stays rejected for both kinds: a local, a field of a data struct, a return, a list or `Option`, a `fn` type, a type argument, a closure capture. `main` takes capability types only; a `#[test]` may take a capability struct, and each field is a grant of the field's name and type.

Checker rules, each an error with its hint:

| # | Rule | Error | Hint |
|---|---|---|---|
| 1 | A struct is a capability struct iff every field is a capability type; a mix is refused | ``struct `Env` mixes capabilities and data: `name: Str` `` | ``a struct holds capabilities only or no capability; move `name` to a data struct`` |
| 2 | No type parameters; no field of capability struct type (v1: one level) | ``capability struct `Env` cannot be generic``; ``capability struct `Job` nests `Env` `` | ``list `Env`'s fields in `Job` `` |
| 3 | A capability struct type is a capability in every rule-1 place | today's messages, with the struct's name: ``capability type in a field``, ``cannot return `Env` from `pick` ``, ``closure captures capability `env` ``, ``function values cannot take or return capabilities``, `` `T` cannot be `Env` `` | today's hints |
| 4 | A field read `env.db` stands only as a receiver, a whole argument, or a literal's value | ``capability `env.db` bound to local `d` `` | ``pass `env.db` directly instead`` |
| 5 | A literal stands only as a whole argument, every field written, no `..base` | ``capability struct `Env` built outside an argument``; ``no `..` in a capability struct literal`` | ``write it as the argument: `handle(Env { .. }, r)` ``; ``name every field`` |
| 6 | A literal's value is a capability parameter or a field of one, of the field's type | `` `Env.db` takes a `Db`, found `Str` `` | ``give a capability parameter or a field of one: `db: db` `` |
| 7 | `main` takes capability types only | `` `main` cannot take capability struct `Env` `` | ``main's parameters are the grants: write `db: Db, now: Clock` and build `Env { .. }` where it is passed`` |
| 8 | In a test, parameter names and capability-struct field names are distinct | ``test `t` names `db` twice: `env.db` and `db` `` | ``rename the field or drop the parameter`` |
| 9 | The 999-parameter limit counts fields | today's message | today's hint |

No equality, no display, no methods: a capability struct is never a value. Nothing else in the language changes (`for`, `match`, generics, modules, `pub` as today).

### Report

- A capability's name in a line is its path: `env.db exec query`. One token per capability; the shape of every line, `--detail` line and R1a4 `derived` line is unchanged.
- Charging (`uses`, `passed`): a whole argument `env` to parameter `e` charges `e.f`'s methods to `env.f` for every field; a literal `Env { f: place }` charges `e.f`'s to `place`. `main`'s parameters are capability types, so `main`'s line, `Authority.main`, every host startup check (`:ro`, `:verify`, verbs) and `--base`'s grant lines are computed exactly as today.
- A test's authority (`entry_authority`) lists fields as grants: `db: Db (exec query transaction), admin: Secret (matches), ..`. `kio test` binds each by name and type from the grants file (the IR entry is already flat), and L30's "names the flag" hint names the field.
- Optional, from C: a function whose every capability is passed whole to one callee prints `tests::call : as booking::handle` (`--all` prints the full line). Decided separately; not needed for A.

### How each constraint holds

| Constraint | Under A |
|---|---|
| Per capability, per method | `env.db query` is a method on one leaf; a struct has no methods, so no line can say `env *` |
| `--base` one-liners | `main` is flat, so grant lines are the same text. A3 refactored to groups against its pre-refactor base: `no change in authority` (acceptance test below) |
| Host trusts nothing | lowering flattens; `grants.rs`, the handle table, every host function and the audit log are untouched; a wrong checker still cannot grant a handle |
| No ambient authority | a struct is received or built from the receiver's own parameters, in argument position; `let`, fields, returns, closures, `fn` types and type arguments are refused with today's errors; a library function's authority is its parameters' leaves |
| Values copy | a struct has no storage: its leaves are the handles they always were; no `let`, write, `==` or print can observe it |
| Compiled = interpreter | one flattened IR; kiogen generates capability structs for the differential harness and `KIO_TEST_HEAP` |
| Review | `passed` follows leaves through whole passes and literals, so `query in booking::create_provider` lines and R1a5's test reach are right; mutants edit tokens; `items_used` already counts a struct named in a signature |

### A3 under A

`booking.kio`: one declaration, two signatures change; every handler keeps its leaf parameters.

```
pub struct Env { db: Db, admin: Secret, rng: Rng, now: Clock, audit: Dir }

pub fn handle(env: Env, r: Req) -> Resp {
  if init(env.db).is_err() { return err(500, "database error"); }
  match route(env, r) { Ok(x) => x, Err(x) => x }
}

fn route(env: Env, r: Req) -> Result<Resp, Resp> {
  ..
    if r.method == "POST" { require_admin(env.admin, r)?; return create_provider(env.db, env.audit, env.now, r); }
  ..
    if r.method == "POST" { return create_booking(env.db, env.rng, env.audit, env.now, r); }
```

`app.kio`: `main` unchanged but for one literal.

```
  let out = booking::handle(booking::Env { db: db, admin: admin, rng: rng, now: now, audit: audit }, r);
```

`tests.kio`: helpers and tests take the group; `test.grants` is unchanged (`--secret admin=file:testdata/admin.token` grants field `admin`).

```
fn call(env: booking::Env, method: Str, target: Str, body: Str) -> booking::Resp {
  booking::handle(env, req(method, target, body, ""))
}
fn provider(env: booking::Env, name: Str) -> Int { .. id_of(call_as(env, ADMIN, "POST", "/providers", ..)) }

#[test]
fn book_a_free_slot_end_to_end(env: booking::Env) {
  let pid = provider(env, "Dr. Ada");
  let s1 = slot(env, pid, "2026-12-01T09:00:00Z", 30);
  let b = book(env, id_of(s1), "Pat@Example.com");
```

Adding a grant (say `log: Log`) is then: a field, a `main` parameter, the literal, and the function that uses it. `kio check`:

```
main            : .. db exec query transaction, admin matches, rng bytes, now now_ms, audit append, ..
booking::handle : env.db exec query transaction, env.admin matches, env.rng bytes, env.now now_ms, env.audit append
booking::route  : env.db query transaction, env.admin matches, env.rng bytes, env.now now_ms, env.audit append
booking::create_provider : db query, audit append, now now_ms

reached only from tests:
tests::call     : env.db exec query transaction, env.admin matches, env.rng bytes, env.now now_ms, env.audit append
```

`kio check app.kio --base <stage 4>`: `no change in authority`. The stage-2 diff that added the secret would read, as today, `+ admin: Secret (matches)` and under `--detail` `+ booking::handle: env.admin matches`.

### Tests that establish it (the task's "Done:")

- Checker, one test per row of the rules table asserting message and hint; plus `struct Env { a: Dir, b: Dir }` accepted, `pub struct` across a module (`booking::Env { .. }`), a generic function taking `env: Env` reported once.
- Report (lang/tests/report.rs): `handle`'s line with paths; charging through a whole pass, a literal from `main`, and a narrowing literal (`Writer { db: env.db, audit: env.audit }`) charges only those leaves; `main`'s line byte-identical to the ungrouped program; `--base` of grouped against ungrouped is `no change in authority`, and `--detail` shows `+ handle: env.admin matches` when the field is new; `users` finds `create_provider` through the group; an `Http` field's verb (`env.api send POST`).
- Lowering and runtime: kiogen generates capability structs; differential tests pass on both runtimes; a 1,000-leaf function is refused by rule 9.
- Host (host/tests/kio_test.rs, review.rs): a test taking `env: Env` is granted by field name from the grants file and by type without one; a missing `--secret admin` names the flag; `kio review` of a change adding a field shows `+ admin: Secret (matches)` once and the `derived` line names the function; a mutant in a function reached only through a group runs.
- A3 (local, not committed): the refactor above checks, `kio test` 28 pass, `kio check --base` against stage 4 prints `no change in authority`.

### Undo

Lowering's flattening is a source rewrite: inline each capability struct parameter as its fields, each `env.f` as `f`, each literal as its places. Undoing the feature is one checker rule back (row 1 refusing every struct with a capability field, today's message) plus that rewrite as a script for programs that used it. Nothing below the checker, no grant, grants file, audit line or `main` report line ever depended on it; only function lines (`env.db`) and `--detail` lines spelled paths.

## Open for the maintainer

| Question | Recommendation | Alternative |
|---|---|---|
| Inferred (a struct of capability fields) or marked | inferred: no new syntax, still parses as Rust, the field types are reserved names so a reader cannot mistake it | `#[cap] struct` (a second attribute); `cap struct` is not Rust |
| Where a capability struct may be a parameter | helpers and tests, not `main`: `main` stays the one place the operator reads the grants, and tests are where the repetition is | also `main`, fields as grant names (then a grant list lives in a type, not in `main`'s signature); or neither (tests keep five parameters; F15 half-solved) |
| Nesting and `..base` | not in v1; both are pure extensions | allow one level now |
| Report spelling | paths, `env.db exec query` | grouped, `env: Env (db exec query, admin matches)`: a second line shape for every tool |
| A `let env = Env { .. }` alias | no: rule 1's "never a local" stays whole; one literal per call site in `main` is the cost | allow an immutable `let` of a literal of parameter names |
