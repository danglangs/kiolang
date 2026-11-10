# Signed, ordered webhooks into SQLite

Write an HTTP handler. `main` receives `req: Incoming` and `res: Outgoing` for each request; `db: Db`, a SQLite database shared by all requests that keeps its contents when the server restarts (empty at first; create your schema if it is missing); `secrets: Dir`, granted for reading; and `log: Log`, an append-only log. Responses are JSON with `content-type: application/json`.

**`POST /webhooks`** receives subscription events.

1. Signature. The secret is the contents of `secrets/webhook.key`, without a trailing newline. The request must have a header `x-signature: sha256=HEX`, where `HEX` is the lowercase hex SHA-256 of the secret's bytes, then `.`, then the request body's bytes. Compare it in constant time. A missing or wrong signature: `401 {"error": "bad signature"}`, and write `rejected signature` to `log`.
2. The body must be a JSON object with string `id`, string `type` (one of `subscription.created`, `subscription.activated`, `payment.failed`, `subscription.canceled`), string `subscription` and integer `seq` of at least 1. Else `400 {"error": "invalid event"}`.
3. An event whose `id` was received before (applied, queued or stale) answers `200 {"status": "duplicate"}` and changes nothing.
4. Each subscription's events must be applied in `seq` order, 1, 2, 3, ... An event with `seq` lower than or equal to the last applied one is `200 {"status": "stale"}`. One beyond the next expected `seq` is kept for later: `202 {"status": "queued"}`. The next expected one is applied, then every kept event that now follows in sequence: `200 {"status": "applied", "applied": K}`, where `K` counts them all.

Applying an event moves the subscription's state (`none` before its first event):

| type | from | to |
|---|---|---|
| `subscription.created` | `none` | `created` |
| `subscription.activated` | `created`, `past_due` | `active` |
| `payment.failed` | `active` | `past_due` |
| `subscription.canceled` | `created`, `active`, `past_due` | `canceled` |

Any other combination still counts as applied (its `seq` is used up) but leaves the state as it is, and writes `invalid transition EVENT_ID` to `log`.

**`GET /subscriptions/ID`**: `200 {"id": ID, "state": STATE, "seq": LAST_APPLIED_SEQ, "history": [STATE, ..]}`, `history` being the states it entered, in order. A subscription with no applied event: `404 {"error": "not found"}`.

Anything else: `404 {"error": "not found"}`.

Grants: `req: Incoming`, `res: Outgoing`, `db: Db`, `secrets: Dir`, `log: Log`
