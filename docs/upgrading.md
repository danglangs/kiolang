# Upgrading

What a program, a grants file or a script must change for each release, newest first, with a command that finds each place to look. Run them from the program's directory. To see what an upgrade changed in a program's tests and authority, `kio review new/app.kio --base old/app.kio` under the new binary compares the two trees ([guide](guide.md#reviewing-a-change)); an old tree failing its own tests there is the upgrade's doing.

## 0.1.12

- **`parse_iso_utc` accepts more** (L35): `2026-11-20T10:00:00Z` (no fraction), 1-9 fraction digits, and `+HH:MM`/`-HH:MM` offsets now give `Some` (UTC), where 0.1.11 gave `None`; a date an offset carries outside years 0000-9999 is `None`. A program that relied on `None` to reject those forms: `grep -rn 'parse_iso_utc' --include='*.kio' .`.
- **`kio review` exit codes** (R1d): a base file missing from an existing directory is now a new entry (exit 3, every grant added), not exit 2; OLD's tests failing to set up is `not checked`, not exit 1. Scripts that treat 2 or 1 from review as "cannot review": `grep -rn 'kio review' --exclude='*.kio' .`.
- `--http-record` binds per test (T8): a grants file whose tests do not take the recorded `Http` now runs instead of being refused.
- `kio review --json`: mutant rows gain `did_not_check_in`, `authority` gains `command_line`; existing fields unchanged. Fuel unchanged.

## 0.1.11

None breaking.

- Fuel is the same or lower on every program (C29: freeing a large value, up to 4% less); no action.

## 0.1.10

- **`kio review`'s page reads differently** (R1c): a caret line (unlabelled, `^` under the edited token) follows each generated survivor; Not checked grant lines lose their `(changed: f, g)` suffix, and each changed function is named once on its own line, `f, g: grant1, grant2 (changed)` (`(new entry)` for an `--entry` the base lacks); a Resources row whose test reaches no changed function ends `; unchanged code`. Scripts that parse the text page: `grep -rn 'kio review' --exclude='*.kio' .`; read `--json` instead, whose Not checked and Resources rows are unchanged (mutant rows gain `caret`).

## 0.1.9

- **`kio check` refuses a literal write passed to `Db.query`** (L34): `db.query("INSERT ..")` (also `UPDATE`, `DELETE`, `REPLACE`, `CREATE`, `DROP`, `ALTER`, and `WITH .. INSERT|UPDATE|DELETE|REPLACE`) is a check error at the SQL, "`query` only reads; this statement writes". Such a call has been refused at run time since 0.1.6; it now fails before running. Find them: `grep -rnE 'query\([[:space:]]*"[[:space:]]*(INSERT|UPDATE|DELETE|REPLACE|CREATE|DROP|ALTER|WITH)' --include='*.kio' .` (misses SQL split over lines or held in a variable, which the host still refuses at run time). Fix: `exec`, or `exec_rows` for `RETURNING`.
- Fuel is back to 0.1.7's on most programs (C27): 0.1 to 1.1% less than 0.1.8; no action.

## 0.1.8

None breaking.

- Programs that do not call the new built-ins use 0.1 to 1.1% more fuel (a larger runtime). A test or a `--listen` request within 1% of its `--fuel` may stop: `grep -rn -- '--fuel' .` lists where a limit is set. The next release wins it back (C27).

## 0.1.7

- **`kio check --base` and `kio review` exit 3 when a function gains a `main` grant it did not hold** (capability groups), printed under `derived  grants unchanged; capability binding changed` as `~ f: d from y (was x)`. A change that swaps same-typed grants between functions exited 0 on 0.1.6. Scripts and CI that read the exit code: `grep -rnE 'kio (check|review)' --exclude='*.kio' .`. Fix: none needed if 3 already means "a person looks"; otherwise read the binding lines.

## 0.1.6

- **`Db.query` only reads.** A statement that writes (`INSERT`, `UPDATE`, `DELETE`, `REPLACE`, `CREATE`, ..., `RETURNING` included) is `Err(DbErr::Denied)`. An app's own error handling can hide it (a 500 or a 404 where the write was); the audit log has it as `"method":"query"`, `"result":"denied"` and the hint "`query` only reads: use `exec` or `exec_rows`". Find the calls:

  ```
  grep -rniE 'query\([[:space:]]*"[[:space:]]*(INSERT|UPDATE|DELETE|REPLACE|CREATE|DROP|ALTER)' --include='*.kio' .
  grep -rni 'RETURNING' --include='*.kio' .
  ```

  SQL held in a variable or a `query(` split over lines is missed: `grep -rn '\.query(' --include='*.kio' .` lists every call to read. Fix: `exec` for a write without rows, `exec_rows` for `INSERT .. RETURNING`, `transaction_rows` for a read after a write in one transaction.

- **`HttpErr::Unreachable`.** A refused or reset connection, a name that does not resolve or a TLS failure was `Err(Denied)` and is now `Err(Unreachable)`. A `match` on `HttpErr` without `_` stops checking ("missing `Err(HttpErr::Unreachable)`"). One with `_` checks, and those failures move silently from the `Denied` arm to `_`: different text, retries or status codes, which no compile error shows. Find each arm that handled them:

  ```
  grep -rn 'HttpErr::Denied\|Denied =>' --include='*.kio' .
  ```

  Fix: an `Unreachable` arm wherever `Denied` meant "the peer is down" (a retry helps); `Denied` now means only the grant refused the request (a retry never helps). A fake host file answers `unreachable` to test it ([guide](guide.md#a-fake-host)).
