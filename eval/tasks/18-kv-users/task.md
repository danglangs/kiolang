# User records in a key-value store

`main` receives `input: Dir`, granted for reading, and `users: Kv`, a key-value store that keeps its contents between runs (empty on the first run).

`input/commands.txt` has one command per line, applied in order:

- `put ID NAME EMAIL`: store the user `ID` (overwriting any earlier one). Print `ok ID`.
- `rename ID NAME`: change a stored user's name. Print `ok ID`, or `missing ID` when there is no such user.
- `get ID`: print the user as JSON, `{"id":"ID","name":"NAME","email":"EMAIL"}` (keys in this order, no spaces), or `missing ID`.
- `del ID`: remove the user. Print `ok ID`, or `missing ID`.
- `count`: print `count N`, the number of users stored.

`ID`, `NAME` and `EMAIL` contain no spaces. Any other line prints `bad LINE` (the line trimmed); blank lines print nothing. Users persist across runs.

Grants: `input: Dir`, `users: Kv`
