# Counter API

Write an HTTP handler. `main` receives `req: Incoming` and `res: Outgoing` for each request, and `counts: Kv`, a key-value store shared by all requests.

Routes (responses are JSON with `content-type: application/json`):

- `POST /hit/NAME` adds one to counter `NAME` and answers `200 {"name": "NAME", "count": N}` with the new count.
- `GET /count/NAME` answers `200 {"name": "NAME", "count": N}`, with 0 for a counter never hit.
- `GET /top` answers `200 [{"name": .., "count": ..}, ..]`: the three counters with the highest counts, highest first, equal counts by name (byte order).
- A `NAME` must be 1 to 32 characters from `a`-`z`, `0`-`9` and `-`; any other answers `400 {"error": "bad name"}`.
- Any other method or path answers `404 {"error": "not found"}`.

Paths have no query string.

Grants: `req: Incoming`, `res: Outgoing`, `counts: Kv`
