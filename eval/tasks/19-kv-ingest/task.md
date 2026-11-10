# Idempotent event ingest

`main` receives `input: Dir`, granted for reading, and `state: Kv`, a key-value store that keeps its contents between runs (empty on the first run).

`input/events.jsonl` has one JSON object per line: `{"id": "e-17", "account": "acme", "cents": 1250}`. Delivery is at-least-once, so an event may appear again, in the same file or in a later run's file. Each run must apply every event exactly once across all runs: add its `cents` to its account's balance, unless an event with that `id` was already applied. `cents` may be negative. A line that is not a JSON object with a string `id`, a string `account` and an integer `cents` is rejected; blank lines are ignored.

Print `applied N duplicate D rejected R` for this run, then the balance of every account seen in any run, `ACCOUNT CENTS`, sorted by account (byte order).

Grants: `input: Dir`, `state: Kv`
