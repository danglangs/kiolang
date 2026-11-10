# Sync a change feed into SQLite, resumably

`main` receives `api: Http`, granted `GET` on an inventory change feed, and `db: Db`, a SQLite database that keeps its contents between runs (empty at first; create your schema if it is missing).

`GET /changes?cursor=C` returns one page: `{"changes": [..], "next": "C2"}`, where `next` is the cursor of the following page, or `null` on the last page. The first page's cursor is `0`. A change is `{"op": "upsert", "sku": "..", "name": "..", "qty": N, "version": V}` or `{"op": "delete", "sku": "..", "version": V}` (`sku` and `name` strings, `qty` and `version` integers). Anything else in `changes` is `invalid` and skipped.

Apply each page's changes in order. A change whose `version` is not greater than the last version applied for its `sku` (by an upsert or a delete) is `stale` and ignored: so a delete must be remembered, and an older upsert must not bring the item back. Otherwise it is `applied`.

Each run continues from where the previous one stopped: start at the cursor of the first page not yet fully applied (`0` on the first run). Store a page's changes and the new cursor together, atomically. After the last page (`next` is `null`), the sync is complete; the next run starts again at that last page's cursor, whose changes may by then be stale. If a page cannot be fetched (an error, a status other than 200) or its body is not a JSON object with a `changes` array, stop there without applying it; the next run retries it.

Print `pages P applied A stale S invalid I` (the pages applied in this run and their changes), then `complete`, or `stopped at C (status N)` (a non-200 status), `stopped at C (invalid page)` or `stopped at C (no response)` (an `Err`), where `C` is the cursor of the page that failed. Then every item present now, sorted by sku (byte order): `SKU QTY NAME`.

Grants: `api: Http`, `db: Db`
