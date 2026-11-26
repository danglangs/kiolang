# A5: a fourth application, built held-out on 0.1.11 (2026-11-25)

A fresh Opus agent, held out (published v0.1.11 binary and the public docs only), built a feed aggregator: a fetch job over `Http` (ETag, retries with backoff, a failure log), a `--listen` API (filters, keyset paging, admin routes behind a `Secret`), a daily digest job writing Markdown into a `Dir`, and a migration job; then a requirements change (an optional author, migrated on existing databases). A second agent then reviewed each stage with `kio review` alone.

| Stage | Time | Failed check / test runs | Review |
|---|---|---|---|
| 1 Schema, normalization, dedup | 4 min (with reading the docs) | 1 / 0 | exit 3, stub base |
| 2 Fetch job | 3.5 min | 0 / 2 | exit 1 (base's tests "do not run") |
| 3 API | 4 min | 1 / 1 | exit 3 (new entry), stub base |
| 4 Digest, author change | 6 min | 0 / 3 | exit 1 (per-run `--value`) |

1,326 lines, 45 tests (8 properties), all passing. Log: 35 entries, 9 `(+)`. The reviewer accepted all four stages, but three of four (every new job) could not be reviewed without a hand-made stub base.

## Ranked

| # | Shortfall | Evidence | Task |
|---|---|---|---|
| 1 | A new entry program cannot be reviewed: its file does not exist in the base (`cannot read`), and a stub `fn main() {}` makes unchanged module functions "changed", mutated, and their tests `added:` | builder S2-4, S1-3; reviewer #1 (stages 2, 3, 4a) | R1d.1 |
| 2 | The base's test setup failing (a `--http-record` grant only the new tests take) is an `error`, exit 1, and drops `added:` and Resources | S2-5; reviewer #2 | R1d.2 |
| 3 | A change to a shared module needs one review per entry: only the root's tests run, so other entries' changed functions read "reached by no test" | S4-7; reviewer #8 | R1d.3 |
| 4 | A grant given per run (`--value day=..`) cannot be in the grants file and `kio review` takes no grant flags: `error`, exit 1, and the binds line (the only place showing `:ro`) is hidden | S4-2; reviewer #3 | R1d.4 |
| 5 | No requirements file on either side refuses the review (exit 2) instead of printing without that section | reviewer failure 1 | R1d.5 |
| 6 | Module code no `main` reaches is never mutated, silently: stage 1's 12 tested functions got 0 mutants and the page read as well covered | S1-4 | R1d.6 |
| 7 | A new or changed grants file shows only as `new file`: a raised `--timeout 600` or `--audit` is invisible; a changed fake (`.http`) shows only as a hash | reviewer #4, #7 | R1d.7 |
| 8 | `parse_iso_utc` rejects `2026-11-20T10:00:00Z` and `+01:00` offsets, the forms feeds send; every app writes a canonicalizer | S1-1 | L35 |
| 9 | `HttpResponse.headers` entries are `"name: value"` strings, undocumented; every program writes a `header(name)` helper | S2-2 | L35 |
| 10 | A module without `main` cannot be `kio check`ed, and the hint "add `fn main()`" is wrong for a module | S1-2 | L35 |
| 11 | `--http-record` is checked across all tests in a file, unlike other grants, so one test grants file cannot serve two entries | S2-3 | T8 |
| 12 | A local named by its grant hides a function; the hint says rename the local, which the grant name fixes | S3-2 | T8 |
| 13 | An `Http` grant is one origin: a feed list of arbitrary URLs is not expressible | S2-1 | maintainer (grant semantics) |
| 14 | `Dir.write` never creates parent directories and `NotFound` does not say so; no `mkdir` | S4-1 | maintainer (caps.wit); doc now (T8) |

Lower, no task: a new struct field errors one file at a time (S4-8: the checker reports per module); `kio.deps` re-pin per module edit for a dev tool in a subdirectory (S2-8: pinning's point); generated `Str` never holds `\n` (S4-5: documented); review time 23 s with three entries (S3-9); survivors tied to functions rather than changed lines (reviewer #5); `derived` lines cannot show a guard ("writes only after `matches`", reviewer #6: a C1 contracts question); `Db query: .. what it changes` wording on a read-only grant (reviewer #10, folded into R1d.7).

## What helped (`(+)`)

Stage 1's 250 lines passed on the first test run; the guide's migration recipe and "run migrations as a job"; the fixed clock with the fake host made backoff tests exact; `Secret` in tests from a file; a property may take a `Db` and a `Secret`; `:ro` bound for the digest; clear diagnostics (tuples, a local hiding a function); survivors found six real test gaps across stages.

The typed encode/decode note: A5's JSON code (normalize, `/items`) is hand-written field access with `Option` defaults, about 60 lines; with L31 it read cleanly. A second example does not yet show a typed mapping would remove a bug class; the note waits.
