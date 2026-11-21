# A4: maintaining an app through upgrades (2026-11-21)

A fresh Opus agent, held out (published v0.1.8 binary, public docs and release notes only), took over A3's booking service (1,128 lines, 28 tests, written on 0.1.5) and did four stages: upgrade, adopt, a requirements change, recovery from failure. The app stays local (~/kio-a4).

| Stage | Time | Result |
|---|---|---|
| 1 Upgrade to 0.1.8 | 5 min | 17 of 28 tests failed (`query` now read-only, H37); two `query` to `exec_rows`, an `Unreachable` arm |
| 2 Adopt 0.1.6-0.1.8 | 9 min | L29/L31 built-ins, a `booking::Env` capability group: booking.kio 315 to 262 lines, fuel -5%; review "no change in authority" |
| 3 Reschedule | 12 min | endpoint, audit line, signed event, 8 tests (one model-based property) |
| 4 Recovery | 14 min | outbox killed mid-batch, database restored one step behind: both simulated live, three fixes, a runbook |

42 of 42 tests pass on seeds 1-3. Log: 27 entries, 13 `(+)` (A3: 47, 10). About a third of the wall time was seven `kio review` runs of ~2 min each.

## Ranked

| # | Shortfall | Cost | Task |
|---|---|---|---|
| 1 | The upgrade's 17 failures pointed nowhere (500, 404, a missing file): `query` refused the writes, the app's error handler hid the host's hint, and `kio check` cannot see that a SQL literal writes | found from the release notes, not the output | L34 |
| 2 | `kio review` takes ~2 min on a 1.2k-line app (7 runs: ~14 min of 40) | a third of the time | T7 |
| 3 | A new `HttpErr` variant fell silently into an existing `_` arm and changed recorded text; the notes warned only about compile errors | 1 min, no test caught it | D9 |
| 4 | No migration guidance in 0.1.8's docs (rebuilding a table, `ALTER`, inside `transaction`); a `--listen` server has no startup step, so migration runs on the first request | 3 min, a scratch run | D7 (done after 0.1.8); startup step: maintainer |
| 5 | Outbox design traps no tool flags: event bodies built from the current row (a retry after a reschedule sends different bytes under one id), and ids reused after a restore make receivers drop new events | 10 min; found only by live simulation | D10 |
| 6 | Foreign keys enforced by the host, undocumented | 1 failed test | D9 |
| 7 | No way to copy or back up a database from kio (`ATTACH`, `VACUUM INTO` fenced); the restore test copied table by table | 3 min | D9 (operator recipe); a backup grant only with evidence |
| 8 | Proving "no behaviour change" for swapped built-ins took throwaway differential property tests | 3 min | none (the method works; R1 could name it) |
| 9 | 10,000-row and 10,000-call defaults forced per-line lookups and a raised `--calls` for the recovery job | 1 min | hints already name the flag; D9 lists the defaults |
| 10 | `kio review --base` compares two trees under the new binary, not old binary against new: what was wanted, but implicit | — | D9 |

## What helped (`(+)`)

The review page's authority lines (`~ db: Db (+exec_rows)`, "no change in authority" for the group refactor), its resources section (caught a +50% fuel regression from a per-request schema check, fixed to -12%), written mutants (named real test gaps), the fake host's `unreachable` answer, property shrinking (found an `Int` overflow), the closure-capture hint, grants files that run unchanged from a scratch directory.

## Against A2 and A3

Upgrade cost was small (5 min) once the cause was known; the cost was finding it. A3's top shortfalls (capability groups, value grant, tests slow, dates, URL decoding) did not recur: L32, H35, T5 and L29 held. New: upgrade diagnosis, review time, and operational patterns (migration, backup, outbox) that the language does not reach.
