# A3: a third app, built held-out on 0.1.5 (2026-11-19)

Same method as [A2](a2-findings.md): a fresh Opus agent, the released v0.1.5 binary and its docs only (README, guide, language, design, review-a-change, first-job), four stages, a friction log. App: appointment booking (Db API with no double booking, admin `Secret`, cancel tokens, audit append, HMAC-signed notifications through a Db outbox with retries, a reminder job as a second entry). 1,128 kio lines, 28 tests (8 properties), all passing; `kio review` of stage 4: exit 3 (the new entry's authority), 16 of 19 mutants killed, one survivor a real gap (an unchecked summary field). The app and log stay local (~/kio-a3/app).

## Against A2

| | A2 (0.1.3) | A3 (0.1.5) |
|---|---|---|
| Log entries (headed), of them `(+)` | 39, 8 | 47, 10 |
| Four stages, first doc read to last commit | not recorded | 17 min; failed check/test runs 0, 2, 1, 4 |
| A2's top ten, hit again | | #6 (no plain value for `main`), #10 (dates, URL decoding) |
| A2's top ten, now `(+)` | | #5 the fake host ("made the notifier fully testable without a server") |
| A2's top ten, not seen | | #1-4, #7-9 (`chars()` cost, test fuel, secrets, diagnostics, append, docs, review grants) |

Counts are not comparable app to app (A3 has more moving parts and logged review findings A2 had no page for); recurrence is. The fixes from A2 held; what remains is structural.

## Ranked

| # | Shortfall (A3 entries) | Cost | Task |
|---|---|---|---|
| 1 | Capabilities cannot be grouped: every handler, helper and test carries `db, admin, rng, now, audit`; one new grant touched ~30 lines, and helpers that pass everything through blur the report (F15) | 10 min, 40 lines | **one-way door**: a struct of capabilities |
| 2 | No dates (F2) | 8 min, 105 lines | L29 |
| 3 | No run-time value for `main` (F31; A2 #6 again: 2 of 2 apps) | 6 min, 25 lines | **one-way door**: a value grant |
| 4 | Tests slow: each property case recompiles; 28 tests 49.6 s wall, 289 s CPU (F11, F20, F40); review 1 min 56 s (F42) | ~1 min per run | T5 |
| 5 | Db: no migration guidance (F18), `INSERT .. RETURNING` through `query` works but the report says `query` for a write (F4), no rows from a transaction (F29), two processes on one file (F39) | 11 min | D7; **one-way door**: rows from a write |
| 6 | A test clock cannot advance; one fake grant per scenario (F25, F26) | 6 min | T6 |
| 7 | No URL decoding (F3) | 4 min, 30 lines | L29 |
| 8 | Review page: an ambiguous survivor snippet (F43), a new entry's functions listed four times (F44), fuel moving 5-11% on untouched tests unexplained (F45) | 6 min | R1c |
| 9 | Diagnostics: `Ok(())` as a pattern says "tuples" (F22); the rerun hint drops `--grants` (F32); a new `Secret`'s test error does not name the flag (F16) | 3 min each | L30 |
| 10 | A refused connection is `HttpErr::Denied`, the same as outside the grant (F23) | 3 min | **one-way door** (caps.wit) |

F1 (primer.md not in the docs given) is the harness's: the agent got a copy of docs/ without it.
