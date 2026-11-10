# Push changed records, with retries

`main` receives `input: Dir`, granted for reading; `state: Kv`, a key-value store that keeps its contents between runs (empty on the first run); and `api: Http`, granted `PUT` (and `GET`) on an upstream records API.

`input/records.jsonl` has one record per line: a JSON object with a string `id`, an integer `version` and any other fields, such as `{"id": "r1", "name": "alpha", "version": 1}`. Any other non-blank line is invalid. If an id appears on several lines, only its last line counts.

Each run pushes every record whose `version` is greater than the last version of that id pushed successfully in any earlier run (none: always push); a record not pushed is `unchanged`. To push, send `PUT /records/ID` with header `content-type: application/json` and the record's line, as a JSON object, as the body. The upstream answers:

- a 2xx status: `synced`; remember the version;
- a 4xx status: `rejected` permanently; do not retry it in this run (a later run tries again);
- a 5xx status, or no response at all (an `Err` from the request): retry it. Each record gets at most 3 attempts per run; after the third failure it is `failed`.

Send no more requests than these rules need: the check counts them. You may send a round of attempts at once (`send_all`) or one by one.

Print `line N invalid` for each invalid line (in file order; lines count from 1), then one line per record id, sorted by id (byte order): `ID synced`, `ID unchanged`, `ID rejected STATUS` (the 4xx status) or `ID failed`. Last, `synced S unchanged U rejected R failed F`.

Grants: `input: Dir`, `state: Kv`, `api: Http` (GET, PUT)
