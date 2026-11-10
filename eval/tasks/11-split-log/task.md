# Split a log by level

`main` receives `input: Dir`, granted for reading, and `out: Dir`, an empty directory to write to. `input/app.log` has lines `TIMESTAMP LEVEL MESSAGE`, for example `2026-11-09T10:00:00Z ERROR db timeout`. `LEVEL` is matched without regard to case.

Write `out/error.log`, `out/warn.log` and `out/info.log`, each holding the lines of that level (`ERROR`, `WARN` or `WARNING`, `INFO`) in input order, unchanged, each ending in a newline. Write all three files, even when one is empty. Lines of any other level, or with fewer than three space-separated fields, go to no file.

Print `error N`, `warn N`, `info N` and `other N` on four lines, the last counting every other non-blank line.

Grants: `input: Dir`, `out: Dir`

Example: for

```
2026-11-09T10:00:00Z ERROR db timeout
2026-11-09T10:00:01Z info started
2026-11-09T10:00:02Z DEBUG cache warm
```

write `out/error.log` with the first line, `out/info.log` with the second, an empty `out/warn.log`, and print

```
error 1
warn 0
info 1
other 1
```
