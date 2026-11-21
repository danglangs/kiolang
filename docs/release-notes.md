# Writing release notes

Each GitHub release's notes, and its section in [upgrading.md](upgrading.md), have these parts:

1. One line: what the release is for.
2. The changes, one bullet each: what a user can now do or must know.
3. **Upgrading**, always present (`None breaking.` when so). One entry per change a program, a grants file or a script could notice:
   - what changed, and how a program finds out: a check error, an `Err` it did not get before, a different exit code, different output;
   - a command that lists every place to look (`grep -rnE '..' --include='*.kio' .`), and what it misses (SQL in a variable, a call split over lines);
   - the fix.

Always check, and write an entry for each that applies:

| Change | Why it needs an entry |
|---|---|
| A new variant of an enum programs match (`HttpErr`, `DbErr`, `IoErr`, ..) | A `match` without `_` stops checking, which the checker names; a `_` arm takes the new case silently, so the cases that used to reach another arm (`Denied`) now reach `_`. Name the old variant and grep for its arms |
| A method that now refuses what it accepted | The program gets an `Err` its error handling may hide (a 500, a 404). Grep for the calls; name the audit line's `result` and `hint` |
| An exit code of `kio check`, `test` or `review` | CI scripts that treat 3 or 1 as failure change verdict |
| Fuel or peak memory moved over 1% on the bench gate | A test or `--listen` request near its `--fuel` or `--memory` may stop |
| A default limit or a grant's meaning | Name the flag that restores the old behaviour, if any |
