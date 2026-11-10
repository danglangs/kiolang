# A job queue with retries and backoff

`main` receives `input: Dir`, granted for reading; `queue: Kv`, a key-value store that keeps its contents between runs (empty on the first run); and `clock: Clock`, the time of this run (milliseconds).

Each run does two things.

**Enqueue.** `input/enqueue.jsonl` has one job per line: `{"id": "j1", "priority": 0}` (`id` a string, `priority` an integer; other fields ignored). A new job is due now, with 0 attempts. A job whose id was ever enqueued before, in any run or earlier in this file, prints `duplicate ID` and is ignored. A non-blank line that is not such an object prints `invalid line N` (lines count from 1).

**Work.** Then run up to 5 jobs that are due (due time at or before now), highest `priority` first, then earliest due time, then id (byte order). Each job runs at most once per run. Running a job is simulated by `input/script.json`, which maps a job id to the outcomes of its attempts: `{"j3": ["fail", "ok"]}` means its first attempt fails and its second succeeds; past the end of its list, the last outcome repeats; a job not listed succeeds. Attempts count across runs.

- On `ok`, the job is done: print `done ID`.
- On `fail`, if this was its 4th attempt the job is dead: print `dead ID`. Otherwise it is due again after a backoff of 60 seconds after the 1st failed attempt, 120 after the 2nd, 240 after the 3rd (from now): print `retry ID in S` (seconds).

Done and dead jobs never run again. Last, print `pending P done D dead X`: the jobs in the queue in each state, over all runs.

Grants: `input: Dir`, `queue: Kv`, `clock: Clock`
