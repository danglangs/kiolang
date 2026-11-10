# Visit counter that survives restarts

`main` receives `input: Dir`, granted for reading, and `counts: Kv`, a key-value store that keeps its contents between runs (empty on the first run).

`input/visits.txt` has one page path per line (`/home`, `/pricing`). Each run adds this file's visits to the running totals kept in `counts`: one key per page.

Then print every page seen so far, in any run, as `PAGE TOTAL`, sorted by total, highest first, then by page (byte order).

Grants: `input: Dir`, `counts: Kv`

Example: if `visits.txt` is `/a`, `/b`, `/a` (three lines), the first run prints `/a 2` and `/b 1`, and a second run with the same file prints `/a 4` and `/b 2`.
