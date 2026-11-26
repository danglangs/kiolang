# Review a change

An agent hands you a change to a kio program. This walkthrough gets the two versions side by side, runs `kio review` on them, reads the page it prints, and ends in one of three decisions. Every flag and line format is in the reference, [guide.md, Reviewing a change](guide.md#reviewing-a-change); this page is the workflow.

All output below is from real runs: sections 7d and 7e of kio after 0.1.11 (R1d), the rest of section 7 of kio 0.1.10, the rest of kio 0.1.5; the commit in each `Identity` is elided.

## 1. What you need

- kio 0.1.10 or later (`kio --version`); 0.1.5-0.1.9 print the same sections without the survivor carets and with `(changed: ..)` on each grant line. No Rust: download a release as in [first-job.md](first-job.md#1-download), e.g. `https://github.com/danglangs/kiolang/releases/latest/download/kio-aarch64-apple-darwin.tar.gz`.
- For the worked example in section 7a, a checkout of [kio-web](https://github.com/danglangs/kio-web).

## 2. Get the change as two directories

`--base` takes a path, not a git revision. Each side loads its own modules (beside its entry) and its own `kio.deps`, whose paths are relative to the `kio.deps` file. So the old side must be a real tree, not loose files.

**A program with no `kio.deps`** (one file, or modules beside it): write the old files out.

```console
$ mkdir -p old
$ git show HEAD:tally.kio > old/tally.kio
$ git show HEAD:README.md > old/README.md
$ kio review tally.kio --base old/tally.kio --requirements README.md
```

**A program with a `kio.deps`** that points outside its directory (kio-web's examples use `../../web.kio`): `git show` breaks the relative path.

```console
$ git show HEAD:examples/flow/kio.deps > old/kio.deps    # with app.kio, flow.kio, tests.kio
$ kio check examples/flow/app.kio --base old/app.kio
old/app.kio:9:1: error: old/kio.deps:2: ../../web.kio does not exist
  hint: a path in kio.deps is relative to the directory kio.deps is in
```

Check the old revision out whole instead, with `git worktree` (this is what [tools/authority-diff.sh](guide.md#reviewing-authority-in-a-pull-request) does):

```console
$ git worktree add --detach ../base main
Preparing worktree (detached HEAD 4582ddb)
HEAD is now at 4582ddb kio.deps in every example and tests/ (kio PK1): no more copying web.kio and api.kio
$ cd examples/flow
$ kio check app.kio --base ../../../base/examples/flow/app.kio
no change in authority
$ git worktree remove ../../../base        # when done
```

## 3. Run `kio review`

```
kio review NEW --base OLD [--grants G [grant flags]] [--test-grants T] [--requirements FILE] [--entry FILE [--entry-grants G2 [grant flags]] [--entry-test-grants T2]].. [--seed S] [--mutants N] [--mutations FILE]
```

A new entry has no file in OLD: give the path it would have, `--base old/api.kio` (section 7d).

| Flag | Give it | Why |
|---|---|---|
| `--grants G` | the operator's grants file for `main` (`app.grants`), then any grant given per run (`--value day=2026-11-20`) | shows an `error` if NEW's `main` needs a grant G does not give; a grant after G binds as `kio run` takes it, marked `(command line)` |
| `--test-grants T` | the tests' grants file, if tests need one (an `Http`, an `Hmac`, a `Secret`) | without it a test that needs one fails. It may not grant a writable `--dir` or store, nor any `--dir` that holds NEW's files (a test could read the code instead of running it) |
| `--requirements FILE` | the file that says what the program should do (`README.md`), a path relative to each side's entry | its diff is what the change claims |
| `--entry FILE` | each other program the change touches (`worker.kio`), a path relative to NEW's entry, the same on OLD's side | its authority and blind spots, prefixed `worker.kio:`; an entry OLD lacks is all new authority |
| `--entry-grants G2` | right after its `--entry`: that program's grants file (`worker.grants`), and grants per run after it | as `--grants`, for that entry |
| `--entry-test-grants T2` | after its `--entry`: that program's tests' grants file, when T does not suit them (its tests need a grant T does not give, as a `--value`) | its tests run under T2; without it, under T |
| `--mutants N` | how many generated mutants to run (default 20, `0` for none) | more mutants, more of the changed code checked against the tests, more time |
| `--seed S` | the tests' seed (default 1), which also draws the mutants | another sample; the same seed gives the same page |
| `--mutations FILE` | mutants you ask for: per entry a function, what it breaks, a `- ` line (text found once in that function) and a `+ ` line (guide, Written mutants; kio-web's `examples/flow/flow.mutants`) | each names a wrong behaviour that a test must catch; one that survives is exit 3. Keep FILE outside NEW's tree: the page notes `(inside NEW's tree)`, since the author could have written it |

Review every entry the change touches: an entry left out is authority nobody reviewed. Each entry's tests run too, on both sides, named after it (`worker.kio: retries`); a test of a module two entries share runs once.

For kio-web's flow: `--grants app.grants --test-grants test.grants --requirements README.md --entry worker.kio --entry-grants worker.grants`, with `FLOW_KEY` set (test.grants reads the key from it). Forget `FLOW_KEY` and the Tests section is one `error`, `--hmac key: `FLOW_KEY` is not set`, and no test runs.

Exit codes: 0 nothing added; 3 authority added, a dependency added or changed (its code is not on the page), or a test removed; 1 an `error` line; 2 usage.

## 4. Read it, section by section

Every line carries a label; a line under a labelled one shares it. The label says what independently backs the line, and so what you may skip reading:

| Label | Backed by | You can skip | You still own |
|---|---|---|---|
| `enforced` | the checker derives it from the code; the host refuses any call outside the grants | reading the code to learn what it can reach | whether that authority fits the requirement; what is done with it |
| `derived` | the checker's attribution of authority to a function (`+ flow::init: db exec`, `  query in flow::claim` under a grant line); the host enforces per grant, not per function | tracing which function makes each call | as for `enforced` |
| `tested` | these tests passed, compiled, in this run (properties: on the generated cases shown); for mutants, a test failed on the edited code | rerunning the tests | whether the tests check the right thing |
| `measured` | fuel and peak memory, deterministic, on the tests | profiling the tests | production load, wall time |
| `not checked` | nothing | nothing | look at it, or accept it knowingly |
| `error` | a failure | nothing | stop: fix it, or reject |

The sections, in order: **Requirements** (the requirement file's diff), **Authority** (`kio check --base`'s lines, and whether `--grants` binds and what it binds: `db=var/inv.db:ro, hook=http://127.0.0.1:9099 (POST)`, a secret by its source, `env:NAME`), **Tests** (both sides' counts, tests added and removed, changed functions no test reaches, each property's cases, each NEW failure, then the mutants: how many the tests killed, which test killed them, and each survivor with a caret under the edited token), **Resources** (per-test fuel and peak memory changes over 2%, `; unchanged code` on a test that reaches no changed function, each added test's, then totals), **Not checked** (what the host never looks at for each method `main` reaches, then each changed function that uses those methods once, with its grants: `cents, total: out (changed)`, `(new entry)` for an entry the base lacks), **Identity** (sha256 of every file that went into the page).

A test grants file may answer an `Http` grant from a file instead of a server (`--http hook=fake:hook.http`, a [fake host](guide.md#a-fake-host)). The page then says so three times:

```
Tests
  tested       grants: test.grants (hook answered from hook.http)
Not checked
  not checked  hook: Http: tests ran against hook.http, not a server; the real endpoint's answers are untested
Identity
  test fake hook.http sha256:..
```

`tested` still means the tests passed, against the answers written in `hook.http`: read that file as part of the change (does a real endpoint answer like this, a 503 then a 200?). kio reads it once, so both sides' tests got the same answers; a change to it during the review is an `error` in Identity. The file must sit inside NEW's directory, and its name is shown ASCII-escaped and quoted as on the binds line, so `h\u{43e}ok.http` cannot pass for `hook.http`.

## 5. Decide

kio gives no verdict. You choose one of three:

| Decision | When |
|---|---|
| **Accept** | every line is evidence you trust, or something you knowingly accept |
| **Reject** | an `error`; authority the requirement did not ask for; a cost regression you do not want |
| **Ask for evidence** | something that matters is only `not checked`: ask for a test or property that pins it, or read that one function |

Ask for evidence is a correct outcome, not a failure to decide: it names the one thing a test should prove, and the next review shows whether it does.

Checklist:

1. Any `error`? Reject, or ask for a fix.
2. Authority: for each `+` or `~` line, does the requirement diff ask for it? A `~` method on an existing grant is authority added like a `+` grant: the requirement must ask for it. If not, reject. The `derived` line under it names where to read. Do the same for each grants-file line (`app.grants: + --dir ..`).
3. Tests: is any test `removed:`, or a property running fewer cases? Find out why before accepting.
4. Do the added tests cover what the requirement diff claims? A `changed:` test (its own code or a function it reaches changed) is read like an added one: it tests something else now. Name tests are a claim too: read the one or two that matter. A `changed, reached by no test:` function ran in no test: read it, or ask for a test that calls it. The converse is weaker: reach is static, so a function named on a test's call path counts as reached even if that path never runs (`if false { risky(d) }`). Mutants: each `survived:` line is an edit to changed code that no test noticed. Read the snippet (the caret marks the edited token): if the edit changes behaviour the requirement cares about, ask for a test that fails on it; if it changes nothing (an equivalent mutant), move on. One test doing all the killing (`killed by:`) is the test to read. `no mutant drawn: F` and `mutants skipped T` say where the sample did not look: raise `--mutants`, or give T a fake host. To ask for evidence of one behaviour, write it as a mutant in a `--mutations` file: the next review kills it (a test now catches it) or shows it `survived:` with exit 3.
5. Resources: any change you would not accept in production? A row ending `unchanged code` moved on a test that runs no changed function: the program's setup cost moved (a new type `main` uses), not that test's code.
6. Not checked: for each line, is it covered by a test you trust, or by a function you read? Otherwise ask.
7. Identity: is this the commit you meant to review?

## 6. What kio never tells you

- **Whether the requirement is right.** The Requirements diff is the change's claim, not a check.
- **What is inside an effect.** The SQL text, the body sent, what is written: `Not checked` lists each one the program can reach.
- **Whether the tests are good.** `tested` means they passed, not that they prove the claim. They are usually written by the same author as the code (the page says so). Killed mutants show the tests notice edits to the changed code, not that they check the requirement: a test comparing a function with a copy of itself kills every mutant in it. Read the test the `killed by:` line names most. (A test cannot read its own source instead: review refuses a test grants `--dir` that holds NEW's files.)
- **Production cost.** Resources are measured on the tests only, not on runs of `main`.
- **Bugs in kio.** Authority is derived and enforced within kio's model ([threat model](../README.md#threat-model)); an implementation bug in the checker or host would break it.

## 7. Worked examples

### a. F1.1 to F1.2 in kio-web's flow

Commit `272a863` adds the worker step to [flow](https://github.com/danglangs/kio-web/tree/main/examples/flow) (F1.2); its parent is F1.1. Both predate `kio.deps`, so `web.kio` and `api.kio` are copied beside each side's `app.kio`, as flow's README said at the time.

```console
$ cd kio-web
$ git log --oneline -4 -- examples/flow
4582ddb kio.deps in every example and tests/ (kio PK1): no more copying web.kio and api.kio
272a863 flow F1.2: worker with leases, fenced completions, backoff, dead state; at-least-once delivery keyed by event id
c26520b flow: Bytes.try_to_str replaces the hand-written UTF-8 validator (kio L27)
e53af3c flow F1.1: ingest signed events as pending jobs (POST /events)
$ git worktree add -q ../flow-old 272a863^
$ git worktree add -q ../flow-new 272a863
$ cp web.kio api.kio ../flow-old/examples/flow/
$ cp web.kio api.kio ../flow-new/examples/flow/
$ cd ../flow-new/examples/flow
$ FLOW_KEY=test-key kio review app.kio --base ../../../flow-old/examples/flow/app.kio \
    --grants app.grants --test-grants test.grants --requirements README.md \
    --entry worker.kio --entry-grants worker.grants
Requirements
  not checked  what the change claims; kio does not check it
               --- ../../../flow-old/examples/flow/README.md
               +++ README.md
               @@ -132,3 +132,124 @@
               ...
               +# F1.2, Worker
               ...
Authority
  derived      grants unchanged; new functions reach: flow::init
  derived      + flow::init: db exec
  enforced     app.grants: unchanged
  enforced     --grants app.grants binds app.kio's main: key=env:FLOW_KEY:verify, now, db=flow.db, --listen 127.0.0.1:8080
  enforced     worker.kio: + db: Db (exec query)
  derived      worker.kio:   exec in flow::claim, flow::finish, flow::init, flow::try_claim
  derived      worker.kio:   query in flow::claim, flow::try_claim
  enforced     worker.kio: + now: Clock (now_ms)
  derived      worker.kio:   now_ms in main
  enforced     worker.kio: + dest: Http (send POST)
  derived      worker.kio:   send POST in deliver
  enforced     worker.kio: worker.grants: new file
  enforced     worker.kio: --entry-grants worker.grants binds worker.kio's main: db=flow.db, now, dest=https://dest.example (POST; Authorization:env:DEST_AUTH)
Tests
  tested       grants: test.grants
  tested       old: 10 passed, 0 failed
  tested       new: 18 passed, 0 failed
  tested       added: tests::a_claim_needs_the_job_still_due_and_unchanged
  tested       added: tests::db_equals_model_and_nothing_is_stranded
  tested       added: tests::every_attempt_carries_the_event_id_as_the_key
  tested       added: tests::expired_lease_at_the_limit_is_dead_not_stranded
  tested       added: tests::expired_lease_is_reclaimed_and_the_old_worker_changes_nothing
  tested       added: tests::failure_retries_with_doubling_backoff_then_dies
  tested       added: tests::pending_claim_done
  tested       added: tests::stale_completion_changes_nothing
  not checked  changed: tests::any_signed_bytes_get_an_answer (reaches changed flow::init, flow::store)
  not checked  changed: tests::bad_signature_beats_bad_json (reaches changed flow::init, flow::store)
  not checked  changed: tests::bad_signature_is_401_and_never_inserts (reaches changed flow::init, flow::store)
  not checked  changed: tests::bad_signature_never_inserts (reaches changed flow::init, flow::store)
  not checked  changed: tests::malformed_json_is_400 (reaches changed flow::init, flow::store)
  not checked  changed: tests::missing_signature_is_401 (reaches changed flow::init, flow::store)
  not checked  changed: tests::new_event_is_202_and_a_pending_row (reaches changed flow::init, flow::store)
  not checked  changed: tests::repeated_id_is_200_and_changes_nothing (reaches changed flow::init, flow::store)
  not checked  changed: tests::replay_equals_each_distinct_event_once (reaches changed flow::init, flow::store)
  not checked  changed: tests::signed_non_utf8_body_is_400 (reaches changed flow::init, flow::store)
  not checked  changed, reached by no test: worker.kio: deliver, main
  tested       tests::any_signed_bytes_get_an_answer: 100 cases compiled; seed 1
  tested       tests::bad_signature_never_inserts: 100 cases compiled; seed 1; 5 discarded
  tested       tests::db_equals_model_and_nothing_is_stranded: 100 cases compiled; seed 1
  tested       tests::replay_equals_each_distinct_event_once: 100 cases compiled; seed 1
  tested       tests::stale_completion_changes_nothing: 100 cases compiled; seed 1
  tested       mutants: 15 of 18 killed (generated at seed 1: 20 drawn of 43 sites, 2 did not check)
  tested       killed by: tests::a_claim_needs_the_job_still_due_and_unchanged 7, tests::failure_retries_with_doubling_backoff_then_dies 4, tests::every_attempt_carries_the_event_id_as_the_key 2, tests::expired_lease_is_reclaimed_and_the_old_worker_changes_nothing 2
  not checked  survived: generated: flow.kio:28:31: flow::store: `"database error"` -> `""` in `...!init(db) { return Err("database error"); }`
                                                                                                                          ^^^^^^^^^^^^^^^^
  not checked  survived: generated: flow.kio:49:43: flow::int_of: `0` -> `1` in `...alue::Int(n) => n, _ => 0 }`
                                                                                                            ^
  not checked  survived: generated: flow.kio:70:6: flow::headers: `"Content-Type: application/json"` -> `""` in `["Content-Type: application/json", "Idempotency-Key: " +...`
                                                                                                                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
Resources
  measured     tests::any_signed_bytes_get_an_answer: fuel 3458446 -> 3693046 (+6.79%), peak memory 1280 KB -> 1280 KB (+0.00%)
  measured     tests::bad_signature_beats_bad_json: fuel 24594 -> 26940 (+9.54%), peak memory 1280 KB -> 1280 KB (+0.00%)
  ...
  measured     added: tests::db_equals_model_and_nothing_is_stranded: fuel 153936773, peak memory 1280 KB
  ...
  measured     added: tests::stale_completion_changes_nothing: fuel 8491549, peak memory 1280 KB
  measured     total over 10 tests on both sides: fuel 66629671 -> 67905004 (+1.92%), largest peak memory 1280 KB -> 1280 KB (+0.00%)
Not checked
  not checked  res: Outgoing send: what the client receives
  not checked  db: Db exec: the SQL text and what it changes
  not checked  flow::init, flow::store: db (changed)
  not checked  worker.kio: db: Db exec, query: the SQL text and what it changes
  not checked  worker.kio: dest: Http send POST: the paths, headers and bodies sent
  not checked  worker.kio: deliver, flow::headers: dest (new entry)
  not checked  worker.kio: flow::backoff_ms, flow::claim, flow::finish, flow::init, flow::int_of, flow::text_of, flow::try_claim: db (new entry)
  not checked  tests and properties were written by the same author as the code unless stated
Identity
  kio 0.1.10 (<commit>)
  old ../../../flow-old/examples/flow/api.kio sha256:889e267b...
  ...
  test seed 1
$ echo $?
3
```

Reading it:

- **Authority**: grants unchanged (`main` already had `db exec`); exit 3 is `init`, new and reached from `main`. It is the `CREATE TABLE`, moved out of `store`. Expected.
- **Authority, `worker.kio`**: a new program, so every grant is `+`: the database (`exec query`, to claim and finish jobs), the clock, and `POST` to one destination, whose key the host adds (the binds line: `dest=https://dest.example (POST; Authorization:env:DEST_AUTH)`, the key named by its source, never its value). That is what the F1.2 requirement describes. The `derived` lines under each grant name the functions that make each call: `deliver` posts, `flow::claim` and `flow::try_claim` query.
- **Tests**: 10 to 18, nothing removed; the 8 added are the worker's transitions and two properties against a model. The F1.1 properties still run 100 cases. But `changed, reached by no test: worker.kio: deliver, main`: the tests call the worker's `flow::` steps, never its loop or its delivery, so those two ran in no test.
- **Mutants**: 20 of the 43 edits kio can make in the changed functions the tests reach (the worker's `flow::` steps among them), at least one in each of the 8 functions; 2 did not check, 15 of the other 18 failed a test. Three survived: `store`'s error message emptied (no test reads it: harmless), `int_of`'s fallback `0` made `1` (a non-integer column never reaches it in the tests), and the `Content-Type` header emptied: no test checks what `deliver` sends besides the key. That one is worth asking for. No mutant touches `deliver` or the worker's `main`: no test reaches them (the line above).
- **Resources**: +3% to +10% fuel on the ingest tests, +1.92% in total, memory flat. None ends `unchanged code`: every ingest test reaches `init`, and now runs a longer `CREATE TABLE` and a wider `INSERT`; acceptable. The added model property costs 154M fuel, the rest under 9M.
- **Not checked**: `db exec: the SQL text`, used by the changed `flow::init` and `flow::store` (the line under it). The table gains three columns, and the requirement diff says "An F1.1 database made before F1.2 has no migration": `CREATE TABLE IF NOT EXISTS` leaves an old table as it is, and the new `INSERT` names `next_ms`. The worker's lines add its SQL and the bodies it posts (every function `(new entry)`, as the worker is new): the tests cover the claims and completions against a model, and `every_attempt_carries_the_event_id_as_the_key` the delivery key.

**Decision: accept**, after reading `deliver` and the worker's `main` (what no test ran), if no F1.1 database is deployed; the missing migration is a known, stated gap. If one is deployed, **ask for evidence**: a migration and a test that runs it on an F1.1 table.

### b. A change that adds authority nobody asked for

The files are in [examples/review](../examples/review) (`v1/` and one directory per change); run the commands from there.

`tally` sums a CSV's cents into `out/total.txt`. The requirement change asks for a row count; the code change also posts the CSV somewhere, and drops the one test that ran `main`.

```console
$ diff -u v1/README.md b/README.md
...
 Rows without a number in `cents` are skipped.
+Also write the number of data rows to `out/count.txt`.
$ diff -u v1/tally.kio b/tally.kio
@@ -11,10 +11,17 @@
   t
 }
 
-fn main(orders: Dir, out: Dir) -> Result<Int, Str> {
+fn rows(csv: Str) -> Int {
+  let n = csv.lines().len();
+  if n == 0 { 0 } else { n - 1 }
+}
+
+fn main(orders: Dir, out: Dir, stats: Http) -> Result<Int, Str> {
   let csv = orders.read("today.csv").map_err(|_| "no orders")?.to_str();
   let t = total(csv);
   out.write("total.txt", (t.to_str() + "\n").to_bytes()).map_err(|_| "cannot write")?;
+  out.write("count.txt", (rows(csv).to_str() + "\n").to_bytes()).map_err(|_| "cannot write")?;
+  let _ = stats.send("POST", "/v1/tally", [], csv.to_bytes(), 2000);
   Ok(t)
 }
 
@@ -29,9 +36,6 @@
 }
 
 #[test]
-fn main_writes_the_total(orders: Dir, out: Dir) -> Result<(), Str> {
-  orders.write("today.csv", "id,cents\n1,100\n2,250\n".to_bytes()).map_err(|_| "setup")?;
-  assert_eq(main(orders, out)?, 350);
-  assert_eq(out.read("total.txt").map_err(|_| "no total")?.to_str(), "350\n");
-  Ok(())
+fn counts_rows() {
+  assert_eq(rows("id,cents\n1,100\n2,250\n"), 2);
 }
$ kio review b/tally.kio --base v1/tally.kio --requirements README.md --grants b/tally.grants
Requirements
  not checked  what the change claims; kio does not check it
               ...
               +Also write the number of data rows to `out/count.txt`.
Authority
  enforced     + stats: Http (send POST)
  derived        send POST in main
  enforced     tally.grants: unchanged
  error        --grants b/tally.grants does not bind to b/tally.kio's main:
                 b/tally.kio:19:32: no grant for `stats: Http`
                 hint: grant it: `--http stats=host`
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 3 passed, 0 failed
  tested       new: 3 passed, 0 failed
  tested       added: counts_rows
  not checked  removed: main_writes_the_total
  not checked  changed, reached by no test: main
  tested       mutants: 2 of 4 killed (generated at seed 1: 4 drawn of 4 sites, 0 did not check)
  tested       killed by: counts_rows 2
  not checked  survived: generated: b/tally.kio:16:11: rows: `0` -> `1` in `if n == 0 { 0 } else { n - 1 }`
                                                                                    ^
  not checked  survived: generated: b/tally.kio:16:15: rows: `0` -> `1` in `if n == 0 { 0 } else { n - 1 }`
                                                                                        ^
Resources
  measured     skips_bad_rows: fuel 16241 -> 16715 (+2.92%), peak memory 1280 KB -> 1280 KB (+0.00%); unchanged code
  measured     sums_the_cents_column: fuel 15013 -> 15487 (+3.16%), peak memory 1280 KB -> 1280 KB (+0.00%); unchanged code
  measured     added: counts_rows: fuel 11639, peak memory 1280 KB
  measured     total over 2 tests on both sides: fuel 31254 -> 32202 (+3.04%), largest peak memory 1280 KB -> 1280 KB (+0.00%)
Not checked
  not checked  out: Dir write: what is written or removed
  not checked  stats: Http send POST: the paths, headers and bodies sent
  not checked  main, rows: out, stats (changed)
  not checked  tests and properties were written by the same author as the code unless stated
...
$ echo $?
1
```

**Decision: reject.** `+ stats: Http (send POST)` is a new grant the requirement does not mention, carrying the whole CSV; the grants file does not give it (`error`); and the only test of `main` was removed, so the changed `main` is `reached by no test` (and gets no mutants). You decided this from the Authority and Tests sections, without reading the diff. (Without `--grants` the page is the same minus the grants lines and the `error`, and exits 3.) Resources moved +3% on tests that never call `main`, and the page says so (`unchanged code`): each run first sets up every type the program uses, and `stats.send` brings `HttpResponse` and `HttpErr` (about 500 fuel a run; [guide, Resources](guide.md#reviewing-a-change)).

### c. A change that stays inside its authority and is wrong

The same `tally`, with a "refactor: pull row parsing into `cents`". The requirement is unchanged; it still says only rows without a number are skipped.

```console
$ diff -u v1/tally.kio c/tally.kio
@@ -1,11 +1,17 @@
 // tally: sum the cents column of orders/today.csv into out/total.txt.
+// The cents of one row, if it has a valid one.
+fn cents(line: Str) -> Option<Int> {
+  let f = line.split(",");
+  if f.len() != 2 { return None; }
+  let c = f[1].trim().parse_int()?;
+  if c > 0 { Some(c) } else { None }
+}
+
 fn total(csv: Str) -> Int {
   let lines = csv.lines();
   let mut t = 0;
   for line in lines.slice(1, lines.len()) {
-    let f = line.split(",");
-    if f.len() != 2 { continue; }
-    let Some(c) = f[1].trim().parse_int() else { continue; };
+    let Some(c) = cents(line) else { continue; };
     t += c;
   }
   t
$ kio review c/tally.kio --base v1/tally.kio --requirements README.md --grants c/tally.grants
Requirements
  not checked  what the change claims; kio does not check it
               no change
Authority
  enforced     no change in authority
  enforced     tally.grants: unchanged
  enforced     --grants c/tally.grants binds c/tally.kio's main: orders=orders:ro, out=out
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 3 passed, 0 failed
  tested       new: 3 passed, 0 failed
  not checked  changed: main_writes_the_total (reaches changed cents, total)
  not checked  changed: skips_bad_rows (reaches changed cents, total)
  not checked  changed: sums_the_cents_column (reaches changed cents, total)
  tested       mutants: 8 of 10 killed (generated at seed 1: 11 drawn of 11 sites, 1 did not check)
  tested       killed by: main_writes_the_total 8
  not checked  survived: generated: c/tally.kio:7:8: cents: `>` -> `>=` in `if c > 0 { Some(c) } else { No...`
                                                                                 ^
  not checked  survived: generated: c/tally.kio:7:10: cents: `0` -> `1` in `if c > 0 { Some(c) } else { None...`
                                                                                   ^
Resources
  measured     no test changed by more than 2%
  measured     total over 3 tests on both sides: fuel 54384 -> 54486 (+0.19%), largest peak memory 1280 KB -> 1280 KB (+0.00%)
Not checked
  not checked  out: Dir write: what is written or removed
  not checked  cents, total: out (changed)
  not checked  tests and properties were written by the same author as the code unless stated
...
$ echo $?
0
```

Exit 0 and a clean page, but `if c > 0` now drops refunds (negative cents) from the total. kio cannot see that: the authority is the same, and no test has a negative row. The page does tell you where to look: the one effect is `out: Dir write: what is written`, used by the changed `cents` and `total` (`cents, total: out (changed)`), and the three tests are listed as `changed` because they reach `cents`, passing but not checked against the change; none has a refund. And both survivors sit on the line, their carets under `>` and `0`: `c > 0` can become `c >= 0` or `c > 1` and no test notices, so no test pins which amounts count.

**Decision: ask for evidence.** Either read `cents` (6 lines), or ask for a test of the requirement's edge: a row with a negative and a zero amount. Here the author adds it, and the next review answers:

```console
$ kio review c2/tally.kio --base v1/tally.kio --requirements README.md
...
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 3 passed, 0 failed
  tested       new: 3 passed, 1 failed
  error        added: refunds_count_against_the_total
  not checked  changed: main_writes_the_total (reaches changed cents, total)
  not checked  changed: skips_bad_rows (reaches changed cents, total)
  not checked  changed: sums_the_cents_column (reaches changed cents, total)
  error        FAIL refunds_count_against_the_total
                 c2/tally.kio:47:3: assert_eq failed: left `100`, right `70`
  tested       mutants: 8 of 10 killed (generated at seed 1: 11 drawn of 11 sites, 1 did not check)
  ...
$ echo $?
1
```

Now it is a reject with a reason anyone can rerun. A property (`total` equals the sum of every row's number, over generated rows) would pin it for good.

### d. A new entry

The files are in [examples/review/feeds](../examples/review/feeds); run the commands from there. `v1/` is a feed aggregator: `feed.kio`, a module its entries share, `fetch.kio` (polls a feed over `Http`) and `digest.kio` (a day's digest from a `Db`). The change adds an API entry, `d/api.kio`, that pages items with `feed::page_size`. `v1/` has no `api.kio`: review it against the file that is not there.

```console
$ kio review d/api.kio --base v1/api.kio
Authority
  not checked  new entry: v1/api.kio has no file
  enforced     + db: Db (query)
  derived        query in listing
  enforced     + limit: Value (get)
  derived        get in main
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 1 passed, 0 failed
  tested       new: 2 passed, 0 failed
  tested       added: lists_a_page
  not checked  changed, reached by no test: main
  tested       mutants: 1 of 1 killed (generated at seed 1: 2 drawn of 2 sites, 1 did not check)
  tested       killed by: lists_a_page 1
Resources
  measured     feed::ok_statuses: fuel 5985 -> 7833 (+30.88%), peak memory 1280 KB -> 1280 KB (+0.00%); unchanged code
  measured     added: lists_a_page: fuel 16992, peak memory 1280 KB
  measured     total over 1 tests on both sides: fuel 5985 -> 7833 (+30.88%), largest peak memory 1280 KB -> 1280 KB (+0.00%)
Not checked
  not checked  db: Db query: the SQL text and what it changes
  not checked  listing: db (new entry)
  not checked  tests and properties were written by the same author as the code unless stated
Identity
  kio 0.1.11 (<commit>)
  old v1/api.kio absent
  old v1/feed.kio sha256:40f97febac17a365fae229e767f13563a4ce725721492f178620a9b7b81c3807
  new d/api.kio sha256:d493928b83cae09552a69cfb0c054a2ca8fe7d243921db79b2bf74393aa6db9f
  new d/feed.kio sha256:40f97febac17a365fae229e767f13563a4ce725721492f178620a9b7b81c3807
  test seed 1
$ echo $?
3
```

OLD's side is `v1/`'s modules that `api.kio` names, under an empty `main`: every grant is `+` (exit 3), and `feed::page_size`, unchanged, is neither `changed` nor mutated; `feed::ok_statuses` ran on both sides, so it is not `added:` (its +31% fuel is the new `main`'s types, `unchanged code`). `old v1/api.kio absent` is checked at the end too: a file written there during the review is an `error`. **Decision: ask for evidence.** The grants are what an API over a database needs, and the SQL is the one `Not checked` line to read; but `main`, which parses `limit`, ran in no test (`reached by no test`). Ask for a test of `main` with a `--value limit=..` in the test grants. (Without the base-less form, the reviewer wrote a stub `fn main() {}` as `v1/api.kio`; every function of `feed.kio` then read as changed, was mutated, and its tests were `added:`.)

### e. A change across entries

The next change raises `feed::page_size` from 50 to 100 for the digest, changes a test fixture in `feed.kio`, makes `fetch.kio` send an ETag (with a test that reads the request back, `--http-record`), and adds the digest's grants file. One review covers both entries: `--entry digest.kio` with its grants file, the day it is run for given on the command line after it (`--value day=..`, as `kio run` takes it), and its tests' own grants file (`--entry-test-grants`), since its test `dated` needs a `day`, which `e/test.grants` does not give.

```console
$ diff -u v1/feed.kio e/feed.kio
-pub fn page_size(n: Int) -> Int { if n > 50 { 50 } else { n } }
+pub fn page_size(n: Int) -> Int { if n > 100 { 100 } else { n } }
...
-pub fn fixture_status() -> Int { 200 }
+pub fn fixture_status() -> Int { 204 }
$ kio review e/fetch.kio --base v1/fetch.kio --grants e/fetch.grants --test-grants e/test.grants \
    --entry digest.kio --entry-grants e/digest.grants --value day=2026-11-20 --entry-test-grants e/digest-test.grants
Authority
  enforced     no change in authority
  enforced     fetch.grants: + --timeout 30
  enforced     --grants e/fetch.grants binds e/fetch.kio's main: src=https://feeds.example.org
  enforced     digest.kio: no change in authority
  enforced     digest.kio: digest.grants: new file
  enforced     digest.kio: digest.grants: + --timeout 600
  enforced     digest.kio: --entry-grants e/digest.grants binds e/digest.kio's main: db=feeds.db:ro, out=digests, day=2026-11-20 (command line)
Tests
  tested       grants: e/test.grants (src answered from e/feed.http)
  tested       digest.kio: grants: e/digest-test.grants
  not checked  old: 3 passed, 1 failed; the base fails its own tests
  tested       new: 5 passed, 0 failed
  tested       old: tests run without --http-record src=sent (no old test takes its Dir)
  tested       added: sends_the_etag
  not checked  changed: feed::ok_statuses (reaches changed feed::fixture_status)
  not checked  changed: fetches (reaches changed fetch)
  not checked  changed: digest.kio: caps_the_digest (reaches changed feed::page_size)
  not checked  failed in old: fetches
  tested       mutants: 7 of 10 killed (generated at seed 1: 11 drawn of 11 sites, 1 did not check)
  tested       killed by: fetches 6, digest.kio: caps_the_digest 1
  not checked  survived: generated: e/fetch.kio:4:82: fetch: `0` -> `1` in `..."v1\\""], "".to_bytes(), 0)?;`
                                                                                                        ^
  not checked  survived: generated: e/feed.kio:1:40: feed::page_size: `>` -> `>=` in `...e(n: Int) -> Int { if n > 100 { 100 } else { n }...`
                                                                                                                 ^
  not checked  survived: generated: e/feed.kio:1:42: feed::page_size: `100` -> `101` in `...n: Int) -> Int { if n > 100 { 100 } else { n } }`
                                                                                                                    ^^^
  not checked  1 changed function not mutated: reached by no `main` (feed::fixture_status)
Resources
  measured     1 test failed in old only; left out of Resources
  measured     no test changed by more than 2%
  measured     added: sends_the_etag: fuel 20294, peak memory 1280 KB
  measured     total over 3 tests on both sides: fuel 28844 -> 28844 (+0.00%), largest peak memory 1280 KB -> 1280 KB (+0.00%)
Not checked
  not checked  src: Http send GET: the paths, headers and bodies sent
  not checked  fetch: src (changed)
  not checked  digest.kio: db: Db query: the SQL text and what it reads
  not checked  digest.kio: out: Dir write: what is written or removed
  not checked  digest.kio: feed::page_size: db, out (changed)
  not checked  src: Http: tests ran against e/feed.http, not a server; the real endpoint's answers are untested
  not checked  fake e/feed.http changed (+3 -0 lines)
  not checked  tests and properties were written by the same author as the code unless stated
Identity
  ...
  test fake e/feed.http sha256:cd7a31b18b32735348eb19fa80e4972ea30801a422120d1b1c5290c5f3b12b4c
  old test fake v1/feed.http sha256:dc25fb8a09f2250132eb6de02f22a600988fd253d0804bddddb1f1cedd0e8ddb
  digest.kio: entry test grants e/digest-test.grants sha256:d337dba00483aef22c8f42c05352d297afc322f9292d9860c5e7b02b1fd7499f
  digest.kio: entry grants e/digest.grants sha256:6c1a2cf3f898a5f72ac49b1d2c84bf618ee3f5dc1438ae64ee64da38a69ba915
  test seed 1
$ echo $?
0
```

Read it per entry. `e/test.grants` records `src` into `sent`, and no base `fetch.kio` test takes `sent: Dir`: the change brought that line in, so it does not get to stop the base's tests. They run without it (`old: tests run without ..`) and compare as usual. Under the new fake, which answers `304` first, the base's `fetches` fails (`failed in old`, `not checked`: it backs nothing); it shows that the fake changed what the old code sees. `digest.kio`'s tests ran on both sides under their own grants, merged into the section with the entry's name: `caps_the_digest` changed with `page_size`, and `feed::ok_statuses`, in the module both entries load, ran once, as the root's, on both sides. The mutants of `feed::page_size` ran every test that reaches it, each in its own program, so `caps_the_digest` killed one; the two survivors are the boundary: no test asks what `page_size(100)` or `page_size(101)` returns. `feed::fixture_status` changed and only tests reach it: no mutant can be made there, and the page says so. The digest's grants file is new: its `--timeout 600` shows as a line of its own, and `db=feeds.db:ro` changes the `Db` blind spot to what the query reads. The fake feed changed with the code (+3 lines: a `304` first): read it with the change. **Decision: ask for evidence** of the page boundary (a test of `page_size(100)` and `page_size(101)`), and accept the rest. (Before, this took two reviews: the `--value` was a usage error, the digest's tests did not run, so `feed::page_size` read `changed, reached by no test`, and the base's tests not setting up was an `error`, exit 1.)
