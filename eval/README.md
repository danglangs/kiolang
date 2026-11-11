# Eval: can models write kio?

45 backend tasks (E1 and E2, docs/product-todo.md), each solved in kio and, as a baseline, in Python. 31-45 are harder: several grants, state across runs, error paths, data structures. E3 (kio against Python in a hardened sandbox, with attacks planted in the data) reuses them: [e3/README.md](e3/README.md).

| Tasks | Grants |
|---|---|
| 01-10 compute | `input: Dir` |
| 11-16 files | `input`/`out: Dir`, `src`/`dst: Dir` |
| 17-21 key-value | `input: Dir`, a `Kv` (two runs, state carried) |
| 22-25 SQLite | `input: Dir`, a `Db` (23: read-only) |
| 26-28 HTTP handlers | `req: Incoming`, `res: Outgoing`, a `Kv` or `Db` |
| 29-30 HTTP clients | `api: Http` against a local fixture server |
| 31 LRU cache, 41 incremental index, 43 job queue (retries, backoff) | `Dir`, a `Kv` (2-5 runs); 43 a fixed `Clock` per run |
| 32 build stages and cycles, 33 interval merge, 34 JSON schema, 39 sessions, 40 quoted CSV, 44 expression parser | `input: Dir` (40: and `out: Dir`) |
| 35 push with retries | `input: Dir`, `state: Kv`, `api: Http` (PUT) against a flaky upstream |
| 36 ledger | `input: Dir`, `db: Db`, `log: Log`, `clock: Clock` (3 runs) |
| 37 sessions, 38 idempotent payments, 45 signed ordered webhooks | HTTP handlers with `Kv` + `Clock` + `Rng`, `Db`, `Db` + `Dir` + `Log`; restarted mid-check |
| 42 resumable change feed | `api: Http`, `db: Db` (4 runs; pages fail, then succeed) |

A task is `tasks/NN-name/`: `task.md` (the prompt), `check.sh PROGRAM` (kio), `check_py.sh PROGRAM` (the same check for Python; grants become environment variables, `INPUT=/path`, a fixed clock `CLOCK=<ms>`), `ref.kio` and `ref.py`, and fixtures. Shared check code is `lib/`; its fixture API (`lib/api.py`) logs every request and can answer a route with a list of responses, one per call (a flaky upstream).

## Run

```
cargo build --release -p kio
eval/check_refs.sh                                   # every reference passes (CI runs this)
eval/run.py --model haiku --lang kio --kio target/release/kio
eval/run.py --model haiku --lang py
eval/run.py --model sonnet --lang kio --tasks 01,26 --jobs 2
eval/run.py --model sonnet --lang kio --spec none    # no language.md: what Rust syntax alone carries
```

Flags: `--model` (a `claude --model` name), `--lang kio|py` (default kio), `--spec full|none` (kio's system prompt: docs/language.md, or only "You write programs in kio, a subset of Rust with capabilities as parameters."; the result file ends `-nospec`); E4 adds `cookbook`, docs/primer.md then language.md, and `terse`, language.md then "Answer with the program only, in one code block.", each file ending `-SPEC`; for `--lang py`, `--spec e3` is E3's Python system prompt), `--budget USD --ledger FILE` (stop a dollar or two short of a dollar limit shared through a JSONL file, writing partial results), `--tasks` (numbers or names, default all), `--jobs` (default 4), `--rounds` (default 5), `--kio` (default `kio` on PATH, else `target/release/kio`), `--out` (result file).

`run.py` needs Python 3 and a logged-in `claude` CLI (no API key; open_questions #165). Per task, up to 5 rounds: the model's program is checked with `kio check` (diagnostics are fed back), then with `check.sh` (its failure output is fed back). It writes `results/<date>-<model>-<lang>[-nospec].json` (every round's program, feedback, output and thinking tokens, and the reply's prose outside the code block) and prints:

| Column | Meaning |
|---|---|
| check1 | the first program passed `kio check` |
| pass1, pass5 | passed the check in round 1, by round 5 |
| rounds | rounds used |
| over-asked | `main` parameters beyond the task's grants |

and the ten `kio check` diagnostics hit most.
