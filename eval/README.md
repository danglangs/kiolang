# Eval: can models write kio?

30 backend tasks (E1, docs/product-todo.md), each solved in kio and, as a baseline, in Python.

| Tasks | Grants |
|---|---|
| 01-10 compute | `input: Dir` |
| 11-16 files | `input`/`out: Dir`, `src`/`dst: Dir` |
| 17-21 key-value | `input: Dir`, a `Kv` (two runs, state carried) |
| 22-25 SQLite | `input: Dir`, a `Db` (23: read-only) |
| 26-28 HTTP handlers | `req: Incoming`, `res: Outgoing`, a `Kv` or `Db` |
| 29-30 HTTP clients | `api: Http` against a local fixture server |

A task is `tasks/NN-name/`: `task.md` (the prompt), `check.sh PROGRAM` (kio), `check_py.sh PROGRAM` (the same check for Python; grants become environment variables, `INPUT=/path`), `ref.kio` and `ref.py`, and fixtures. Shared check code is `lib/`.

## Run

```
cargo build --release -p kio
eval/check_refs.sh                                   # every reference passes (CI runs this)
eval/run.py --model haiku --lang kio --kio target/release/kio
eval/run.py --model haiku --lang py
eval/run.py --model sonnet --lang kio --tasks 01,26 --jobs 2
```

Flags: `--model` (a `claude --model` name), `--lang kio|py` (default kio), `--tasks` (numbers or names, default all), `--jobs` (default 4), `--rounds` (default 5), `--kio` (default `kio` on PATH, else `target/release/kio`), `--out` (result file).

`run.py` needs Python 3 and a logged-in `claude` CLI (no API key; open_questions #165). Per task, up to 5 rounds: the model's program is checked with `kio check` (diagnostics are fed back), then with `check.sh` (its failure output is fed back). It writes `results/<date>-<model>-<lang>.json` (every round's program and feedback) and prints:

| Column | Meaning |
|---|---|
| check1 | the first program passed `kio check` |
| pass1, pass5 | passed the check in round 1, by round 5 |
| rounds | rounds used |
| over-asked | `main` parameters beyond the task's grants |

and the ten `kio check` diagnostics hit most.
