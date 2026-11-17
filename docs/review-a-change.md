# Review a change

An agent hands you a change to a kio program. This walkthrough gets the two versions side by side, runs `kio review` on them, reads the page it prints, and ends in one of three decisions. Every flag and line format is in the reference, [guide.md, Reviewing a change](guide.md#reviewing-a-change); this page is the workflow.

All output below is from real runs of kio 0.1.2 with the R1a2 review changes (the next release); the commit in each `Identity` is elided.

## 1. What you need

- kio 0.1.2 or later (`kio --version`). No Rust: download a release as in [first-job.md](first-job.md#1-download), e.g. `https://github.com/danglangs/kiolang/releases/latest/download/kio-aarch64-apple-darwin.tar.gz`.
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
old/app.kio:9: error: old/kio.deps:2: ../../web.kio does not exist
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
kio review NEW --base OLD [--grants G] [--test-grants T] [--requirements FILE]
```

| Flag | Give it | Why |
|---|---|---|
| `--grants G` | the operator's grants file for `main` (`app.grants`) | shows an `error` if NEW's `main` needs a grant G does not give |
| `--test-grants T` | the tests' grants file, if tests need one (an `Http`, an `Hmac`) | without it a test that needs one fails |
| `--requirements FILE` | the file that says what the program should do (`README.md`), a path relative to each side's entry | its diff is what the change claims |

For kio-web's flow: `--grants app.grants --test-grants test.grants --requirements README.md`, with `FLOW_KEY` set (test.grants reads the key from it). Forget `FLOW_KEY` and the Tests section is one `error`, `--hmac key: `FLOW_KEY` is not set`, and no test runs.

Exit codes: 0 nothing added; 3 authority added or a test removed; 1 an `error` line; 2 usage.

## 4. Read it, section by section

Every line carries a label; a line under a labelled one shares it. The label says what independently backs the line, and so what you may skip reading:

| Label | Backed by | You can skip | You still own |
|---|---|---|---|
| `enforced` | the checker derives it from the code; the host refuses any call outside the grants | reading the code to learn what it can reach | whether that authority fits the requirement; what is done with it |
| `derived` | the checker's attribution of authority to a function (`+ flow::init: db exec`); the host enforces per grant, not per function | tracing which function makes each call | as for `enforced` |
| `tested` | these tests passed, compiled, in this run (properties: on the generated cases shown) | rerunning the tests | whether the tests check the right thing |
| `measured` | fuel and peak memory, deterministic, on the tests | profiling the tests | production load, wall time |
| `not checked` | nothing | nothing | look at it, or accept it knowingly |
| `error` | a failure | nothing | stop: fix it, or reject |

The sections, in order: **Requirements** (the requirement file's diff), **Authority** (`kio check --base`'s lines, and whether `--grants` binds), **Tests** (both sides' counts, tests added and removed, each property's cases, each NEW failure), **Resources** (per-test fuel and peak memory changes over 2%, then totals), **Not checked** (what the host never looks at for each method `main` reaches), **Identity** (sha256 of every file that went into the page).

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
2. Authority: for each `+` or `~` line, does the requirement diff ask for it? If not, reject.
3. Tests: is any test `removed:`, or a property running fewer cases? Find out why before accepting.
4. Do the added tests cover what the requirement diff claims? Name tests are a claim too: read the one or two that matter.
5. Resources: any change you would not accept in production?
6. Not checked: for each line, is it covered by a test you trust, or by a function you read? Otherwise ask.
7. Identity: is this the commit you meant to review?

## 6. What kio never tells you

- **Whether the requirement is right.** The Requirements diff is the change's claim, not a check.
- **What is inside an effect.** The SQL text, the body sent, what is written: `Not checked` lists each one the program can reach.
- **Whether the tests are good.** `tested` means they passed, not that they prove the claim. They are usually written by the same author as the code (the page says so).
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
    --grants app.grants --test-grants test.grants --requirements README.md
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
  enforced     --grants app.grants binds app.kio's main
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
  tested       tests::any_signed_bytes_get_an_answer: 100 cases compiled; seed 1
  tested       tests::bad_signature_never_inserts: 100 cases compiled; seed 1; 5 discarded
  tested       tests::db_equals_model_and_nothing_is_stranded: 100 cases compiled; seed 1
  tested       tests::replay_equals_each_distinct_event_once: 100 cases compiled; seed 1
  tested       tests::stale_completion_changes_nothing: 100 cases compiled; seed 1
Resources
  measured     tests::any_signed_bytes_get_an_answer: fuel 3587375 -> 3858475 (+7.56%), peak memory 2112 KB -> 2112 KB (+0.00%)
  measured     tests::bad_signature_beats_bad_json: fuel 25762 -> 28473 (+10.53%), peak memory 1920 KB -> 1920 KB (+0.00%)
  ...
  measured     total over 10 tests on both sides: fuel 71107433 -> 72494821 (+1.96%), largest peak memory 2112 KB -> 2112 KB (+0.00%)
Not checked
  not checked  res: Outgoing send: what the client receives
  not checked  db: Db exec: the SQL text and what it changes
  not checked  tests and properties were written by the same author as the code unless stated
Identity
  kio 0.1.2 (<commit>)
  old ../../../flow-old/examples/flow/api.kio sha256:889e267b...
  ...
  test seed 1
$ echo $?
3
```

Reading it:

- **Authority**: grants unchanged (`main` already had `db exec`); exit 3 is `init`, new and reached from `main`. It is the `CREATE TABLE`, moved out of `store`. Expected.
- **Tests**: 10 to 18, nothing removed; the 8 added are the worker's transitions and two properties against a model. The F1.1 properties still run 100 cases.
- **Resources**: +3% to +10% fuel on the ingest tests, +1.96% in total, memory flat. Every ingest now runs `init`, a longer `CREATE TABLE` and a wider `INSERT`; acceptable.
- **Not checked**: `db exec: the SQL text`. The table gains three columns, and the requirement diff says "An F1.1 database made before F1.2 has no migration": `CREATE TABLE IF NOT EXISTS` leaves an old table as it is, and the new `INSERT` names `next_ms`.

`app.kio` is not the whole change: `worker.kio` is a new program with no base. Read its report and grants as new authority:

```console
$ kio check worker.kio --grants worker.grants
main            : db exec query, now now_ms, dest send POST
deliver         : dest send POST
flow::claim     : db exec query
flow::finish    : db exec
flow::init      : db exec
flow::try_claim : db exec query

reached only from tests:
...
19 more use no capability, 36 are unreached (5 use a capability); --all lists every function
```

**Decision: accept**, if no F1.1 database is deployed; the missing migration is a known, stated gap. If one is deployed, **ask for evidence**: a migration and a test that runs it on an F1.1 table.

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
  error        --grants b/tally.grants does not bind to b/tally.kio's main:
                 b/tally.kio:19:32: no grant for `stats: Http`
                 hint: grant it: `--http stats=host`
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 3 passed, 0 failed
  tested       new: 3 passed, 0 failed
  tested       added: counts_rows
  not checked  removed: main_writes_the_total
Resources
  measured     skips_bad_rows: fuel 16606 -> 17656 (+6.33%), peak memory 1600 KB -> 1600 KB (+0.00%)
  measured     sums_the_cents_column: fuel 15345 -> 16395 (+6.85%), peak memory 1600 KB -> 1600 KB (+0.00%)
  measured     total over 2 tests on both sides: fuel 31951 -> 34051 (+6.58%), largest peak memory 1600 KB -> 1600 KB (+0.00%)
Not checked
  not checked  out: Dir write: what is written or removed
  not checked  stats: Http send POST: the paths, headers and bodies sent
  not checked  tests and properties were written by the same author as the code unless stated
...
$ echo $?
1
```

**Decision: reject.** `+ stats: Http (send POST)` is a new grant the requirement does not mention, carrying the whole CSV; the grants file does not give it (`error`); and the only test of `main` was removed. You decided this from the Authority and Tests sections, without reading the diff. (Without `--grants` the page is the same minus the `error`, and exits 3.) Resources moved +6% on tests that never call `main`: each run first sets up every type the program uses, and `stats.send` brings `HttpResponse` and `HttpErr` (about 1,000 fuel a run; [guide, Resources](guide.md#reviewing-a-change)).

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
  enforced     --grants c/tally.grants binds c/tally.kio's main
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 3 passed, 0 failed
  tested       new: 3 passed, 0 failed
Resources
  measured     no test changed by more than 2%
  measured     total over 3 tests on both sides: fuel 56186 -> 56288 (+0.19%), largest peak memory 1728 KB -> 1728 KB (+0.00%)
Not checked
  not checked  out: Dir write: what is written or removed
  not checked  tests and properties were written by the same author as the code unless stated
...
$ echo $?
0
```

Exit 0 and a clean page, but `if c > 0` now drops refunds (negative cents) from the total. kio cannot see that: the authority is the same, and no test has a negative row. The page does tell you where to look: the one effect is `out: Dir write: what is written`, and no test covers a refund.

**Decision: ask for evidence.** Either read `cents` (6 lines), or ask for a test of the requirement's edge: a row with a negative and a zero amount. Here the author adds it, and the next review answers:

```console
$ kio review c2/tally.kio --base v1/tally.kio --requirements README.md
...
Tests
  tested       grants: defaults (temporary Dir, :memory: stores, fixed clock)
  tested       old: 3 passed, 0 failed
  tested       new: 3 passed, 1 failed
  error        added: refunds_count_against_the_total
  error        FAIL refunds_count_against_the_total
                 c2/tally.kio:47:3: assert_eq failed: left `100`, right `70`
...
$ echo $?
1
```

Now it is a reject with a reason anyone can rerun. A property (`total` equals the sum of every row's number, over generated rows) would pin it for good.
