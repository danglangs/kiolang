# agent-demo: a Python agent that delegates file jobs to kio

A Pydantic AI agent (Claude Sonnet 5.5) with one tool, `run_kio`. The model writes a kio
program; the app checks it (errors go back to the model) and runs it with grants **the app**
fixes: your input directory read-only, one output directory, nothing else. The model never
picks a path, host or key.

**Install** (Python 3.10+, a `kio` binary on `PATH` or in `$KIO`):

    cd tools/agent-demo
    python3 -m venv .venv && .venv/bin/pip install -r requirements.txt -e ../kio-py
    mkdir -p ~/.config/kio-demo && (umask 077; cat > ~/.config/kio-demo/anthropic_key)  # paste key, Ctrl-D

The key is read from that file and handed to the client; it is never printed or logged.

**Run one request:**

    .venv/bin/python -m agent_demo "Total paid orders per customer; write totals.csv" --input data

**Output:**

- `== Answer`: the model's summary of what it did.
- `== Authority`: the grants the app gave (`input: Dir (read-only)`, `out: Dir`), kio's report of
  what the program can do with them (`main : input read, out write`), and the audit of what it
  did (`input.read x1 ok, out.write x1 ok`). A write to `input`, or any other host or file, is
  refused by kio, not by the model's good will.
- `== Outputs`: the output directory (a new temp dir unless `--out DIR`) and its files.
- The last line: model calls, kio runs, tokens and cost.

`--rates-api` adds one grant, `api: Http` (GET only), to a local stand-in API; kio injects its
`Authorization` header, so the program never holds the key.

**The 10 sample jobs** (`agent_demo/jobs.py`, over `data/`) each have a Python checker, so a
run is pass or fail: `.venv/bin/python run_all.py [ids..]` writes `eval/results/d5-<date>.json`
(pass, rounds, tokens, cost) and stops before the shared ledger (`d5-ledger.jsonl`) passes $10.
Offline tests (no model): `KIO=.. .venv/bin/python -m unittest discover tests`.

Not here: the same jobs through a Python executor (W1's comparison) and a UI.
