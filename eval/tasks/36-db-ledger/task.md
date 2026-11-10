# A ledger on SQLite, with an error log

`main` receives `input: Dir`, granted for reading; `db: Db`, a SQLite database that keeps its contents between runs (empty at first; create your schema if it is missing); `log: Log`, an append-only log; and `clock: Clock`, the time of this run.

`input/ops.txt` has one operation per line, words separated by single spaces. Account names and transfer ids are any words without spaces; amounts are integer cents.

- `open NAME CENTS`: open an account with that balance (0 or more).
- `transfer ID FROM TO CENTS`: move `CENTS` (more than 0) from one account to another, atomically.
- `close NAME`: close an account whose balance is 0. A closed account no longer exists for `transfer` and `close`, and its name cannot be opened again.

Transfer ids are unique across all runs: a transfer whose id was already used by a successful transfer is a `duplicate` if `FROM`, `TO` and `CENTS` are the same (nothing happens, nothing is logged), else it fails with `id reused`. A failed transfer does not use up its id.

An operation fails with the first of these that applies, in this order:

| Message | When |
|---|---|
| `malformed` | an unknown operation, a wrong number of words, or an amount that is not an integer |
| `id reused` | (transfer) as above |
| `bad amount` | an `open` below 0, a `transfer` of 0 or less |
| `same account` | `FROM` equals `TO` |
| `account exists` | `open` of a name that is or was ever open |
| `no account NAME` | the account does not exist (for a transfer, check `FROM` first) |
| `insufficient funds` | `FROM` has less than `CENTS` |
| `overflow` | `TO` would pass 9223372036854775807 |
| `balance not zero` | `close` of an account whose balance is not 0 |

Each failure writes one line to `log`: `TIME line N: MESSAGE`, where `TIME` is the run's time as `YYYY-MM-DDTHH:MM:SSZ` (UTC, milliseconds dropped) and `N` the line number in `ops.txt` (from 1). Blank lines are ignored.

Print `ok K failed F duplicate D` for this run, then each open account, sorted by name (byte order), as `NAME CENTS last TIME`, where `TIME` is when its balance last changed (or it was opened), in the same format.

Grants: `input: Dir`, `db: Db`, `log: Log`, `clock: Clock`
