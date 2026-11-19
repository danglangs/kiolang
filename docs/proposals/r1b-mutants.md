# R1b: mutation runs on the review page (design, 2026-11-18)

E3c's six slips were all "tests pass, behaviour weakened": a sentinel row id, a magic signature, a dropped event id. The page says `tested`, and the walkthrough's section 6 says that means passed, not good. F1.2 answered this by hand: four mutants applied to flow.kio, each expected to fail a test (its README, "Mutation checks"); that table is what let the reviewer skip 339 lines of tests. R1b puts that run on the page.

The decision a reviewer should make from it: *if this changed function were wrong in this way, a test would fail.* Established by `kio review` applying the mutant to NEW's text, running NEW's tests on it in fresh sandboxes under the test grants, and reporting the test that failed. Not established: that the tests check the requirement (see "What a mutant does not show").

No one-way door: no change to the language, caps.wit or `lang`'s dependencies. Mutants are text edits to NEW's own files, re-checked and run through the `kio test` path unchanged; the host already links `lang`'s lexer. The one lasting contract is the mutations file format below, a `kio review` input that a later version can drop or version like a grants file.

## 1. Who writes the mutants

| Source | Trust | Cost | Noise |
|---|---|---|---|
| **Generated** by kio on the changed functions some test reaches: seeded, reproducible | the author chose nothing; kio's operators are public | many sites (flow F1.2: about 50); capped and sampled | equivalent mutants survive (the README's fifth: `<=` for `<` in a `SELECT` `try_claim` re-checks) |
| **Written** file (`--mutations FILE`) by the reviewer or a maintainer, outside the change | as `--grants`: whoever runs `kio review` chose it; hashed in Identity; the page says if FILE sits inside NEW's tree | one test run per entry; a few entries | none: each names a wrong behaviour someone cares about |

Recommendation: both, generated first. Generated mutants need nothing from anyone and cannot be steered by the author; they catch the "tests do not constrain this at all" case cheaply. A written file carries domain mutants no operator produces (drop `AND lease = ?` from a SQL string, drop a header) and is what a reviewer writes when they *ask for evidence*: the next review shows whether the asked-for test exists. Mutants run on NEW only; OLD is never mutated.

## 2. Generated mutants (R1b1)

Sites: every token in a changed function of NEW (`Changes::changed`, the page's existing notion) that `main` reaches and at least one test reaches statically (`lang::reachable`). A changed function no test reaches is already the `changed, reached by no test` line; every mutant in it would survive, so none is made and that line stays the evidence. Operators, token-level, each yielding one mutant per site:

| Operator | Example |
|---|---|
| comparison flip | `<` <-> `<=`, `>` <-> `>=`, `==` <-> `!=` |
| boolean | `&&` <-> `||`; `true` <-> `false` |
| integer literal +1 | `5` -> `6` (`0` -> `1`) |
| string literal emptied | `"Idempotency-Key: "` -> `""` (not inside a `#[test]` function) |
| constructor | `Some(x)` -> `None`; `Ok(x)` -> `Err(..)` only when both payload types match; `is_ok` <-> `is_err`, `is_some` <-> `is_none` |
| statement dropped | a `;`-terminated expression statement or a `let _x = ..;` removed |

A mutant is NEW's files with one replacement, checked as NEW is (`kio_host::check_program`); one that does not check is discarded and counted (`3 did not check`). No mutant touches a grants file, `kio.deps`, a dependency's files (their pins would fail) or a test function.

Selection: all sites are enumerated in file, span order; `--mutants N` (default 20, max 500) are drawn by a shuffle seeded from the tests' `--seed`, so a rerun gives the same mutants and the page prints the seed. Each mutant runs only the tests that reach its function, examples before properties, and stops at the first failure: a property stops at its first failing case, with no shrinking (F1.2's friction 4: shrinking cost minutes per mutant) and no interpreter oracle (a mutant's compiled failure is the kill; the oracle is for kio bugs, run on the unmutated side). Fuel per run: ten times what that test used on unmutated NEW; `out of fuel` or a trap counts as killed, shown as such. A test's wall timeout (10 s) stays the backstop; a kill by it says `(timeout; machine-dependent)`. Cost on flow F1.2 at the default: 20 mutants, most killed by the first reaching example test in under 50 ms; a survivor runs every reaching test and property, about 5 s each at 100 cases, so roughly 20 to 60 s added to the review. `--mutants 0` turns it off.

## 3. The mutations file (R1b2)

```
# flow.mutants: a function, a description, one `-`/`+` pair. The `-` text must occur exactly
# once in that function's text in NEW. `---` between entries. Names as the page shows them.
flow::finish: drop the lease from the done update
- SET state = 'done' WHERE id = ? AND lease = ?
+ SET state = 'done' WHERE id = ?
---
flow::headers: drop the key header
- ["Content-Type: application/json", "Idempotency-Key: " + job.id]
+ ["Content-Type: application/json"]
---
flow::finish: off by one on the limit
- job.attempts >= limit
+ job.attempts > limit
---
worker.kio: deliver: any status counts as delivered
- resp.status >= 200 && resp.status < 300
+ true
```

The site is a function plus a snippet, never a line number: a reflowed file keeps the entry valid, and a snippet that no longer occurs is a fact the reviewer needs (the code it pinned has changed). `-` found zero or several times in the function, a function NEW lacks, or a mutant that does not check: an `error` line with the entry and a hint, exit 1. An entry function (`worker.kio: deliver`) mutates that entry's program and runs the root's tests, as `changed, reached by no test` already relates them. FILE is read once, before the runs, hashed in Identity and re-read at the end as fakes are. FILE may sit anywhere; if its canonical path is inside NEW's directory the line says `(inside NEW's tree)`, since then the change may have written it.

## 4. The page

```
Tests
  tested       mutants: 7 of 8 killed (generated 4 of 4 at seed 1, 3 did not check; flow.mutants 3 of 4)
  tested       killed by: db_equals_model_and_nothing_is_stranded 4, stale_completion_changes_nothing 2, deliver_posts_the_body 1
  not checked  survived: flow.mutants: flow::finish: off by one on the limit (`>=` -> `>`); ran 9 tests, 2 properties
  not checked  survived: generated: worker.kio: deliver: `<` -> `<=` in `resp.status < 300`
```

`--json`: `tests.mutants` with `{seed, requested, made, did_not_check}` and one row per mutant `{source, function, from, to, description, killed_by, how, tests_run, fuel}`. The `killed by` roll-up names each killing test and its count, so one test that kills everything, or a test the change added doing all the killing, is visible on the page.

Exit code. A survivor from the written file is exit 3, as a removed test is: the reviewer asked for exactly this behaviour to be constrained and it is not; accepting it is a decision. A generated survivor is `not checked` and leaves the exit code alone: equivalent mutants are routine and exit 3 for every review would drown the signal (the reviewer reads the survivor's snippet; most take seconds). A mutations file that cannot be applied, or a mutation run that could not finish, is `error`, exit 1. The Resources section is unchanged (unmutated runs only).

## 5. Relation to the rest of the page

- `changed, reached by no test`: those functions get no mutants (every one would survive, cheaply and uselessly); the line stays. The two lines together say: these changed functions ran in no test, and of the ones that did, this is how much the tests notice.
- Fakes (T4): a mutant run gets the same `hook.http` bytes as both sides' tests, so "drop the key header" is killable inside `kio test` by a test that reads its `--http-record`; F1.2 needed `test.sh` for that. The `not checked  hook: Http: tests ran against hook.http` line still applies to mutant runs.
- Changed tests (R1a4): mutants are never placed in a `#[test]` function or a function only tests reach; mutating the tests measures nothing a reviewer asked.

## 6. Security

**Authority.** A mutant is NEW's text under the test grants T, through `test_setup` and `run_once` as every test: fresh temporary `Dir`s, `:memory:` stores, the fixed clock, T's fakes; never `--grants G`. Review already refuses a writable shared target in T. A mutant that would call a method T does not grant fails to check or is refused by the host like any test; it gains nothing. Mutants are built in memory from the bytes already read and hashed; nothing is written to NEW's tree.

**Can NEW tell it is being mutated?** Only by observing its own text. Compiled code cannot read fuel, and the clock is fixed unless T grants a real one; a mutant run is indistinguishable from a `kio test` run in every input the program receives. What remains is the program comparing itself to a copy: a test `assert_eq(finish(..), finish_copy(..))` kills every mutant in `finish` and constrains nothing, and a T that grants `--dir src=.:ro` over NEW's own files lets a test hash its source against a constant. kio can close the second (R1b3: refuse a T directory containing a NEW source file, as it refuses writable ones) and cannot close the first: mutation testing measures the tests' sensitivity to the code, not their fidelity to the requirement. The honest defences are on the page: the killer's name beside each count, and the standing line that tests were written by the code's author. A reviewer who sees one added test kill everything reads that test.

**Cost as an attack.** Slow tests spend the budget: the run stops at the per-test timeout and the page says `error  mutants: 3 of 20 ran; budget exhausted`, never a silent partial count.

## 7. Tasks and Done tests

- **R1b1. Generated mutants.** Operators and selection as section 2; `--mutants N`; page and JSON as section 4 without the file lines. Done (host/tests/review.rs): a NEW whose changed function has a `<` the tests pin (killed, the test named) and a comparison they do not (survived, exit 0, the snippet shown); a mutant that loops is killed `(out of fuel)` within 10x the test's fuel; two runs at one seed give identical pages, a different seed different mutants; a changed function no test reaches gets no mutant; a `#[test]` function gets none; no mutant runs under `--grants G` (a mutant in a function that reaches `main`'s `Http` fails to run under T, counted as did not check or killed, never as sent); `--mutants 0` prints nothing. Result line: flow F1.1 -> F1.2 at seed 1, N made, killed, survivors.
- **R1b2. `--mutations FILE`.** Format, site rules, errors with hints, Identity hash, inside-tree note, survivor exit 3. Done: the four F1.2 mutants as kio-web `examples/flow/flow.mutants`, all killed, the README's hand table replaced by the page; a snippet found twice, a snippet absent, an unknown function, a mutant that does not check: each an `error` with its hint, exit 1; FILE rewritten during the run: Identity `error`; a survivor is exit 3.
- **R1b3. Hardening and docs.** T refuses a directory that contains a NEW source file; `killed by` roll-up; guide.md "Reviewing a change" and review-a-change.md sections 4 to 6 (`tested` for mutants, what a survivor means, the copy attack in section 6); W1's protocol notes the line so the next run measures whether reviewers use it.
