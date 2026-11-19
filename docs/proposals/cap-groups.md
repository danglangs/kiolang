# Grouping capabilities

Design only (maintainer-approved to write, 2026-11-19; A3 #1, F15; A2 friction 3.7). Revised after an outside review, before a soundness review and a build decision. A change to language semantics is a one-way door: this note is written so the decision takes one reading.

**What grouping does and does not do.** Grouping preserves explicit capability provenance and per-operation reporting: every method a function may call is still charged to one capability, and that capability is still traced to the `main` grant it came from. Narrow groups and individual fields support least-authority interfaces; grouping itself does not grant less. A helper that takes a broad `Env` receives all of its authority, whether or not it uses it, and its report line says so.

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
| Report per capability, per method, derived by the checker from checked bindings (P5, P8) | `handle : db exec query, admin matches` | never `env *`; every method charged to one capability, traced to its grant |
| `kio check --base` grant lines are exact one-liners | `~ db: Db (+query)`, `+ admin: Secret (matches)` | the same lines for the same grants |
| The host never trusts the checker | the host binds grants to `main`'s IR parameters and checks every call itself | enforcement code and audit lines unchanged |
| No ambient authority | a function's authority is its capability parameters; closures cannot capture a capability | a library gains nothing it was not passed |
| Values copy | a capability is a handle, copied like an `Int` | nothing observable changes |
| Compiled code and the interpreter agree | both run one IR | no new IR |
| Review machinery (R1a4 `users`, R1a5 reach, R1b mutants) keeps working | follows a grant parameter to parameter | follows it through a group |

## Options

### A. A capability struct

A `struct` with at least one field, every field a primitive capability type (`Dir`, `Db`, .., the types `caps.wit` declares), is a **capability struct**. It is a capability for every purpose of rule 1 (a parameter, passed whole, never stored, returned, captured, in a list or a `fn` type), with one more operation, a field read, and one way to build it, a literal in argument position from the caller's own parameters.

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

`kio fix --param 'admin: Secret' handle` adds a parameter to a function, every caller and every test, through the modules. No language change. But the diff a reviewer reads still touches 30 lines per grant (R1a4's `changed:` marks them all), the friction recurs with every grant, and the agent already does this rewrite with a script: the cost F15 measured is in reading, not typing.

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
| Report | paths as capability names; a binding map per call edge; origins per leaf (below); `passed`, `users`, `param_ty` follow leaves | names from `main` | none |
| Review | `--base` gains binding lines; `users` and `passed` follow leaves; tokens and reach unchanged | the same | none |
| Docs | language.md rule 1 and report; guide.md tests and the grouping guideline | the same | guide.md |
| What a reader gains | a signature names the group once; a module declares what it needs | little | nothing |

## Recommendation: A

### Rule 1, revised

> **Capabilities are parameters only.** A capability is a capability type (`Dir`, `Db`, ..) or a **capability struct**: a `struct` with at least one field, every field a capability type. A capability can be received as a parameter, have a method called on it, or be passed whole as an argument; a capability struct can also have a field read, `env.db`, which is a capability wherever a capability name may stand. A capability struct is built only by a literal that is a whole argument, each value a capability parameter of the caller or a field of one: `handle(Env { db: db, now: now }, r)`, `narrow(Writer { db: env.db, audit: env.audit }, r)`. Everything rejected today stays rejected for both kinds: a local, a field of a data struct, a return, a list or `Option`, a `fn` type, a type argument, a closure capture, a pattern. `main` takes capability types only; a `#[test]` may take a capability struct, and each field is a grant of the field's name and type.

Recognition: a struct is a capability struct iff it has at least one field and every field is a primitive capability type. `struct Empty {}` is a data struct, as today. No `.kio` program in this repository or kio-web declares a struct with a capability field (the checker refuses one today); the one Rust fixture that does, `struct Job { dir: Dir }` in lang/tests/check.rs asserting "capability type in a field", becomes a mixed struct (`struct Job { dir: Dir, n: Int }`) so it keeps asserting a rejection. kiogen's `struct E {}` and `struct Z {}` stay data.

Checker rules, each an error with its hint:

| # | Rule | Error | Hint |
|---|---|---|---|
| 1 | At least one field and every field a capability type; a mix is refused | ``struct `Env` mixes capabilities and data: `name: Str` `` | ``a struct holds capabilities only or no capability; move `name` to a data struct`` |
| 2 | No type parameters; no field of capability struct type (v1: one level) | ``capability struct `Env` cannot be generic``; ``capability struct `Job` nests `Env` `` | ``list `Env`'s fields in `Job` `` |
| 3 | A capability struct type is a capability in every rule-1 place | today's messages, with the struct's name: ``capability type in a field``, ``cannot return `Env` from `pick` ``, ``closure captures capability `env` ``, ``function values cannot take or return capabilities``, `` `T` cannot be `Env` `` | today's hints |
| 4 | A field read `env.db` stands only as a receiver, a whole argument, or a literal's value | ``capability `env.db` bound to local `d` `` | ``pass `env.db` directly instead`` |
| 5 | A literal stands only as a whole argument, every field written, no `..base` | ``capability struct `Env` built outside an argument``; ``no `..` in a capability struct literal`` | ``write it as the argument: `handle(Env { .. }, r)` ``; ``name every field`` |
| 6 | A literal's value is a capability parameter or a field of one, of the field's type | `` `Env.db` takes a `Db`, found `Str` `` | ``give a capability parameter or a field of one: `db: db` `` |
| 7 | `main` takes capability types only | `` `main` cannot take capability struct `Env` `` | ``main's parameters are the grants: write `db: Db, now: Clock` and build `Env { .. }` where it is passed`` |
| 8 | In a test, parameter names and capability-struct field names are distinct, checked before any grant binds | ``test `t` names `db` twice: `env.db` and `db` `` | ``rename the field or drop the parameter`` |
| 9 | The 999-parameter limit counts fields | today's message | today's hint |

No equality, no display, no methods, no pattern: a capability struct is never a value. Nothing else in the language changes (`for`, `match`, generics, modules, `pub` as today).

### Report: function lines, binding maps, provenance

Three things, kept apart.

**1. Function lines (as today).** A function's line is its effects against its own leaves: `(function, leaf, method)`, a leaf being a capability parameter or a field of one, named by its path: `inspect : d query`, `handle : env.db exec query, env.admin matches`. A helper's contract is about its parameters, whatever it is called with. A program without capability structs prints a byte-identical report.

**2. Binding maps, one per call edge.** A call site `s` in `f` of `g` gives a partial map from `g`'s leaves to `f`'s leaves, read off the checked bindings (P5: the type recorded at each span, never a name):

| Argument at `s` | Parameter of `g` | Map |
|---|---|---|
| a leaf `d` or `env.db` | a capability type `p` | `p -> d`, `p -> env.db` |
| a whole group `env` | a capability struct `e` | `e.x -> env.x` for every field `x` |
| a literal `Env { x: place, .. }` | `e` | `e.x -> place` for every field |

Charging is today's `uses` fixpoint over these maps: `effects(f)` includes `map_s(effects(g))` for every edge `s`. A whole-group pass is one edge with a map per field; a literal is one edge whose map may send two fields to one leaf.

**3. Provenance.** Composing the maps along every path from `main` gives each leaf of each function its **origins**: the `main` grants bound to it on some path. `main`'s own leaves are the grants. `main`'s line, `Authority.main`, every host startup check (`:ro`, `:verify`, verbs) and `--base`'s grant lines are computed exactly as today from the fixpoint; `main`'s parameters are capability types, so nothing there changes shape.

Worked examples:

| Program | Lines and maps |
|---|---|
| Two call sites: `main(primary: Db, backup: Db) { inspect(primary); inspect(backup) }`, `inspect(d: Db)` queries | `inspect : d query`; edges `{d -> primary}`, `{d -> backup}`; `main : primary query, backup query`; origins of `inspect.d` = {primary, backup}. The two edges stay distinct; `d` is not "primary or backup" on the function line |
| Recursion: `walk(d: Dir, n: Int) { .. walk(d, n - 1) }`; mutual `a(x, y) { b(y, x) }`, `b(x, y) { a(y, x) }` | the self edge `{d -> d}` adds nothing; the cycle gives `a.x`, `a.y`, `b.x`, `b.y` each both origins and both functions each other's methods: possible authority, over-approximated as today |
| Aliasing: `handle(Env { a: db, b: db }, r)`, `query` on `e.a`, `exec` on `e.b` | `handle : env.a query, env.b exec`; map `{e.a -> db, e.b -> db}`; `main : db exec query` (the union); origins of both leaves {db} |
| Two same-typed grants: `handle(Env { a: x, b: y }, r)` | `main : x query, y exec`; origins `env.a` {x}, `env.b` {y}; a `:ro` on `y` is refused at startup by `main`'s line |
| Narrowing: `route(env)` calls `create_provider(env.db, env.audit, env.now, r)` and `save(Writer { db: env.db, audit: env.audit }, ..)` | maps `{db -> env.db, audit -> env.audit, now -> env.now}` and `{w.db -> env.db, w.audit -> env.audit}`; `create_provider : db query, audit append, now now_ms` unchanged from today |
| A helper shared by two entries: `booking::init(db)` from `app.kio`'s `main` and from `remind.kio`'s | one function line; origins per entry (`app.kio`: {db}, `remind.kio`: {db}); `kio review --entry remind.kio` prefixes that entry's lines (R1a3) |

A test's authority (`entry_authority`) lists fields as grants: `db: Db (exec query transaction), admin: Secret (matches), ..`. `kio test` binds each by name and type from the grants file (the IR entry is already flat), and L30's "names the flag" hint names the field.

### The `--base` comparison rule

`kio check --base` and `kio review` compare three things:

| | Key | Shown | Counts as added |
|---|---|---|---|
| (a) grant lines | `main`'s `(grant, type)` and the methods `main` reaches on it, exactly as today | `~ db: Db (+query)` | as today |
| (b) function lines | `(function, leaf, method)`; a leaf's key is its short name (the last path component) and type when unique among the function's leaves, else its path | the path, `+ handle: env.admin matches` | as today (covered-by rules apply per leaf) |
| (c) binding lines | `(function, leaf)` to its origin set | `~ inspect: d from backup (was primary)` | when a leaf gains an origin: the function now holds a grant it did not |

Leaf identity between a grouped and an ungrouped interface: `db: Db` and `env.db: Db` in the same function are the same leaf under (b), because the key is the short name and type (`fn merge(a: Env, b: Env)` has two `db` leaves, so those are keyed `a.db`, `b.db`). For a program without groups the key is the parameter name, so today's function lines are byte-identical.

**No change.** A pure grouping refactor shows `no change in authority` when, for every function `main` reaches, every leaf in the base has a leaf of the same key in the new program with the same methods and the same origins, and (a) is unchanged. Composing the new binding maps must therefore agree with the old ones leaf by leaf: `handle(db, admin, ..)` with maps `{db -> db, admin -> admin}` and `handle(Env { db: db, admin: admin })` with `{e.db -> db, e.admin -> admin}` agree. The A3 refactor below meets this; so does moving a leaf into or out of a group, or narrowing a group. Renamed parameters are not suppressed: `d` to `dir` is a removed and an added leaf, as today.

**Swaps detected.** Any binding that gives a function a `(grant, method)` it did not have, on any line:

- `handle(Env { a: x, b: y })` to `Env { a: y, b: x }` with `query` on `e.a`, `exec` on `e.b`: (a) `~ x: Db (+exec -query)`, `~ y: Db (+query -exec)`; (c) `~ handle: env.a from y (was x)`, `~ handle: env.b from x (was y)`. Exit 3.
- `f(x); g(y)` to `f(y); g(x)`, both `exec`: (a) unchanged (`x exec, y exec`); (c) `~ f: d from y (was x)`, `~ g: d from x (was y)`. Exit 3: each function holds a grant it did not.
- `inspect(primary)` to `inspect(primary); inspect(backup)`: (a) `~ backup: Db (+query)`; (c) `~ inspect: d from primary, backup (was primary)`.

**Blind spots** (see Limits): a function that already receives both grants, with the calls' roles swapped (`inspect(primary); inspect(backup)` reordered; or `copy(src, dst)` to `copy(dst, src)` when both grants already reach `copy` with the same methods on both). (a), (b) and (c) are all unchanged: it is a correctness change, not an authority change, and belongs in review's Not checked section: `not checked  inspect: d from primary, backup; which call uses which is behaviour`, marked `(changed: F)` by R1a4's rule when a caller's text changed.

Interaction with what exists: R1a4's `derived  read in load` under a grant line is `users` over `passed`, which is (c)'s relation read from the grant side; the two never disagree. P8 holds: only functions `main` reaches have origins or lines. `--detail` prints (b) then (c). `kio review` labels (b) and (c) `derived`, (a) `enforced`; `--json` gains `authority.bindings[]` with `{function, leaf, from, was}`.

### How each constraint holds

| Constraint | Under A |
|---|---|
| Per capability, per method, traced to a grant | `env.db query` is a method on one leaf; a struct has no methods, so no line can say `env *`; each leaf's origins follow the binding maps to `main`'s grants |
| `--base` one-liners | `main` is flat, so grant lines are the same text; function lines keyed by leaf as today; binding changes are their own one-liners |
| Host trusts nothing | lowering flattens; `grants.rs`, the handle table, every host function and the audit log are untouched; a wrong checker still cannot grant a handle |
| No ambient authority | a struct is received or built from the receiver's own parameters, in argument position; `let`, fields, returns, closures, `fn` types, type arguments and patterns are refused with today's errors; a library function's authority is its parameters' leaves |
| Values copy | a struct has no storage: its leaves are the handles they always were; no `let`, write, `==` or print can observe it |
| Compiled = interpreter | one flattened IR; kiogen generates capability structs for the differential harness and `KIO_TEST_HEAP` |
| Review | `passed` follows leaves through whole passes and literals, so `query in booking::create_provider` lines and R1a5's test reach are right; mutants edit tokens; `items_used` already counts a struct named in a signature |

### A3 under A

Handlers keep their leaf parameters: a specialized function takes what it uses, so its line stays short and its origin exact. Only the two functions that genuinely need everything take the group.

`booking.kio`: one declaration, two signatures change.

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

### Guideline for guide.md (added at build time)

> Group what travels together: a service's environment, a job's clock and log. A specialized helper takes the leaves it uses (`fn save(audit: Dir, now: Clock, ..)`) or a narrow group (`Writer { db, audit, now }`), so its report line is its own authority. One program-wide `Env` passed to every function is not the default library interface: each such function's line becomes the whole program's authority, and the report tells a reviewer nothing about what it does.

### Acceptance tests

Functional (the task's "Done:"):

- Checker, one test per row of the rules table asserting message and hint; `struct Env { a: Dir, b: Dir }` accepted; `struct Empty {}` still data; `pub struct` across a module (`booking::Env { .. }`); a generic function taking `env: Env` reported once.
- Report (lang/tests/report.rs): `handle`'s line with paths; charging through a whole pass, a literal from `main`, and a narrowing literal (`Writer { db: env.db, audit: env.audit }`) charges only those leaves; `main`'s line byte-identical to the ungrouped program; a program without groups prints a byte-identical report; `--base` of grouped against ungrouped is `no change in authority`; `--detail` shows `+ handle: env.admin matches` when the field is new; a parameter rename alone prints nothing; `users` finds `create_provider` through the group.
- Lowering and runtime: kiogen generates capability structs; differential tests pass on both runtimes; a 1,000-leaf function is refused by rule 9.
- Host (host/tests/kio_test.rs, review.rs): a test taking `env: Env` is granted by field name from the grants file and by type without one; a missing `--secret admin` names the flag; `kio review` of a change adding a field shows `+ admin: Secret (matches)` once and the `derived` line names the function; a mutant in a function reached only through a group runs.
- A3 (local, not committed): the refactor above checks, `kio test` 28 pass, `kio check --base` against stage 4 prints `no change in authority`.

Adversarial (each a test; the expected text is the acceptance):

| # | Attempt | Expected |
|---|---|---|
| 1 | Two `Db` fields swapped between base and new (`Env { a: y, b: x }`), the callee calls `exec` on `e.a` | grant lines `~ x: Db (-exec)`, `~ y: Db (+exec)`; binding lines `~ handle: env.a from y (was x)`, `~ handle: env.b from x (was y)`; the function line unchanged (`handle : env.a exec`); exit 3; a `:ro` grant on `y` is refused at startup |
| 1b | `f(x); g(y)` to `f(y); g(x)`, both callees `exec` | grant lines unchanged; binding lines `~ f: d from y (was x)`, `~ g: d from x (was y)`; exit 3 |
| 1c | `inspect(primary); inspect(backup)` reordered, or `copy(src, dst)` to `copy(dst, src)` with both already reaching `copy` | (a), (b), (c) unchanged, exit 0; the Not checked line `inspect: d from primary, backup; which call uses which is behaviour` is marked `(changed: main)` |
| 2 | One grant supplied to two fields, `Env { a: db, b: db }`, `query` on `e.a` and `exec` on `e.b` | `main : db exec query` (the union); `--base` against `Env { a: db, b: other }` shows `~ db: Db (+exec)`, `- other: Db (exec)`, `~ handle: env.b from db (was other)` |
| 2b | A pure grouping refactor of A3 stage 4, and the same with one leaf moved out of the group, and with `route` narrowed to `Writer` | `no change in authority` each time; a parameter renamed in the same change prints its `-`/`+` function lines as today |
| 3 | Same-typed grants with different restrictions through every path: `ro: Db` (`:ro`) into a field a callee `exec`s on, through a whole pass, a literal and a narrowing literal; `v: Hmac` (`:verify`) into a field `sign`ed; `api: Http` whose field is `send`t with a non-literal verb | each refused at startup by `main`'s line (`exec` on `ro`, `sign` on `v`); the report says `env.api send *`, and `--base` from `send POST` to `send *` adds |
| 4 | Shadowing and modules: a local `env` of a data type with a field `db` declared after a parameter `env: Env`, then `env.db.query(..)` in and after its block; `web::Env` and the root's `Env`; two modules each declaring `Env`; a literal of the wrong module's type | inside the block `env.db` is data (no method `query`; nothing charged), after it the capability again, by the checked type at the span (P5), as the shadowing test of P5 does for a leaf; a wrong type is "expected `web::Env`, found `Env`" |
| 5 | Escapes of a whole group: `let e = env;`, `[env]`, `Some(env)`, `struct Job { env: Env }`, `enum E { A(Env) }`, `fn pick(..) -> Env`, `fn f(x: fn(Env))`, `id::<Env>(env)`, `\|\| env.db.query(..)`, `match env { e => .. }`, `env = other`, `if let x = env` | each refused with the rule-3 or rule-4 text; no program with one checks |
| 6 | Escapes of a field: `let d = env.db;`, `[env.db]`, `Some(env.db)`, `Job { d: env.db }`, `return env.db`, `\|x\| env.db.read(x)`, `env.db = x`, `match env.db { .. }`, `env.db == env.db` | each refused: ``capability `env.db` bound to local``, ``closure captures capability `env` ``, "capabilities have no equality", today's texts for the rest |
| 7 | `Http` verbs through fields and helpers: `env.api.send(v, ..)` with `v` a constant, a literal, a parameter; `send_all` with a list built in a helper that takes `env` | the same verbs as the leaf program (`send PUT`, `send_all POST`, `send *`); P7's rules unchanged by the path |
| 8 | Test flattening collisions: `fn t(env: Env, db: Int)`, `fn t(a: Env, b: Env)` with a shared field name, `fn t(env: Env, db: Db)` | rule 8's error at the test's signature, before `kio test` opens any grant or temporary directory |
| 9 | A capability struct declared in a dependency (`kio.deps`) and used by the root; a dependency's helper taking a group from the root | the dependency gains only what the root passes; its lines show paths; `kio review`'s dependency comparison unchanged |
| 10 | 999 leaves across groups (`a: Env500, b: Env499`, one data parameter) | rule 9 refuses at the signature |

### Limits

What the report and diff do not see, stated so the review page can say so:

- **Roles swapped among grants a function already holds.** `inspect(primary); inspect(backup)` reordered, or `copy(src, dst)` to `copy(dst, src)` when both grants already reach `copy` with the same methods: no grant line, function line or binding line moves. It is a correctness change. Review lists each function whose leaf has two or more origins of one type under Not checked (`inspect: d from primary, backup; which call uses which is behaviour`), marked `(changed: F)` when a caller's text changed; the audit log, per grant name, is where the actual binding of a run is visible.
- **Possible, not actual.** A function line is possible authority (as today): a leaf reached on two paths holds both origins, a cycle gives every participant every origin. Grouping adds no imprecision of its own, but a broad `Env` makes "possible" wide on purpose; the guideline above is the answer, not the checker.
- **What flows through a capability.** Which SQL text, path or body a call carries is Not checked today and stays so; a group does not change it.
- **Same type, same use, different restriction.** Two `Db` grants with the same methods but one `:ro`: swapping them is refused at startup only if the `:ro` grant now reaches a write; a swap between two `:ro` grants or two unrestricted ones is the first bullet.

### Undo

Lowering's flattening is a source rewrite: inline each capability struct parameter as its fields, each `env.f` as `f`, each literal as its places. Undoing the feature is one checker rule back (row 1 refusing every struct with a capability field, today's message) plus that rewrite as a script for programs that used it. Binding lines (c) stand on their own (they detect swaps the function lines never showed) and need not be undone with it. Nothing below the checker, no grant, grants file, audit line or `main` report line ever depended on the feature; only function lines (`env.db`) and `--detail` lines spelled paths.

## Open for the maintainer

| Question | Recommendation | Alternative |
|---|---|---|
| Inferred (a struct of capability fields) or marked | inferred: no new syntax, still parses as Rust, the field types are reserved names so a reader cannot mistake it | `#[cap] struct` (a second attribute); `cap struct` is not Rust |
| Where a capability struct may be a parameter | helpers and tests, not `main`: `main` stays the one place the operator reads the grants, and tests are where the repetition is | also `main`, fields as grant names (then a grant list lives in a type, not in `main`'s signature); or neither (tests keep five parameters; F15 half-solved) |
| Nesting and `..base` | not in v1; both are pure extensions | allow one level now |
| Report spelling | paths, `env.db exec query` | grouped, `env: Env (db exec query, admin matches)`: a second line shape for every tool |
| A `let env = Env { .. }` alias | no: rule 1's "never a local" stays whole; one literal per call site in `main` is the cost | allow an immutable `let` of a literal of parameter names |
| Binding lines (c) in `kio check --base` | print them under `--detail` with the function lines, and always in `kio review`; a gained origin is exit 3 | print only in `kio review`; or a gained origin exit 0 when `main`'s line is unchanged (then 1b above passes unseen) |
