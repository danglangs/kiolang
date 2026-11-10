# Rate limiter replay

`main` receives `input: Dir`, granted for reading, and `limits: Kv`, a key-value store that keeps its contents between runs (empty on the first run).

`input/requests.txt` has one request per line, `UNIX_SECONDS CLIENT`, in time order (a later run's file continues where the earlier one ended). Apply a fixed-window limit: each client may make at most 3 requests per minute window, where the window is `UNIX_SECONDS / 60` (integer division). A denied request does not count against the limit. Counts must persist between runs, since a window may span two files.

For each request print `UNIX_SECONDS CLIENT allow` or `UNIX_SECONDS CLIENT deny`. Ignore blank lines and lines that are not two fields with an integer first.

Grants: `input: Dir`, `limits: Kv`
