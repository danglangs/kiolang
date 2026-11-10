# Product tasks: approving AI-written code by its authority

Direction A (maintainer, 2026-11-09): a team lets agents write backend code and approves what the code can touch, not every line. The host enforces it and logs it. Three tasks make that demonstrable; each is pinned as in the other `*-todo.md` files.

```
agent writes app.kio ──> PR ──> kio check --base: authority diff ──> human approves the diff
                                                                    │
operator runs it:  kio run app.kio --http stripe=… --http-header stripe=Authorization:env:KEY
                   (host enforces grants, injects the secret, writes the audit log)
```

## Tasks

- [ ] **E1. Eval: can models write kio?** The first question for anything built on kio.
  - `eval/tasks/NN-name/`: `task.md` (what to build, the grants it gets, sample I/O), `check.sh PROGRAM` (runs it with those grants and exits 0 when the output is right; HTTP tasks start `--listen` and curl it), and `check_py.sh PROGRAM` for the Python baseline (same checks, same inputs; network and files are open to Python).
  - 30 tasks: 10 compute (input from a granted `Dir`), 6 `Dir`, 5 `Kv`, 4 `Db`, 3 HTTP handlers, 2 `Http` clients (against a local server the check starts).
  - `eval/run.py --model M --lang kio|py` (stdlib only): the system prompt is docs/language.md (kio) or nothing (Python); the model returns one program; up to 5 rounds, each feeding back `kio check`'s diagnostics, then the check's failure output; Python gets its errors and the check's output. Models are called through `claude -p --model M` (no tools), so the eval needs no API key.
  - Measures per task: checks on the first try (kio), passes on the first try, passes by round 5, rounds used, and the main parameters asked for beyond the task's grants (over-asked authority).
  - Output: `eval/results/<date>-<model>-<lang>.json`; a table in roadmap.md (Haiku, Sonnet and Opus 5.5, kio vs Python) and the ten diagnostics hit most, each becoming an L12-style task.
  - Done: the 30 tasks, each with a reference kio and Python solution that pass their checks (CI runs the references, not the models); one full run per model recorded.

- [ ] **P1. `kio check NEW --base OLD`: the authority diff.** What a reviewer approves.
  - `lang::report_diff(old, new) -> Vec<String>`: per function, the capability methods added (`+ create: hook send`) and removed (`- ...`); `main`'s parameters first, as a new parameter is a new grant: `+ main: grant hook: Http`. Functions present in one side only count as all-added or all-removed. Unchanged functions are not shown; nothing changed prints `no change in authority`.
  - Exit codes: 0 when no authority was added (removals only, or nothing); 3 when any was added; 1 when either side fails to check (the message names which side).
  - `tools/authority-diff.sh BASE_REF ENTRY.kio`: checks out `BASE_REF` with `git worktree add` into a temporary directory, runs `kio check ENTRY --base <tmp>/ENTRY`, removes it. guide.md gains a section with a CI example (a job step, not a published action).
  - Done: tests for an added method, a new `main` parameter, a removed method (exit 0), a new function using a capability, a function moved to a module, no change, a base that fails to check; the links app diffed against a version without the webhook shows exactly `hook send` added.

- [ ] **S1. Secrets as headers the host adds.** The program never holds the key, so it cannot leak it.
  - `--http-header name=Header:SOURCE`, `SOURCE` = `env:VAR` or `file:PATH`, read once at start (missing or empty: refused, with a hint). Repeatable; only on a granted `Http` name.
  - The host adds the header to every request on that grant: `get`, `send`, `send_all`, and redirects (which stay in the origin). A program sending a header of the same name gets `HttpErr::Invalid`. Refused at start on an `http://` origin that is not loopback (the key would cross the network in clear).
  - Never in audit, `--log`, errors or the report: the audit line lists `"injected":["authorization"]`. Under `--listen`, the same for every run.
  - Threat model, documented: an origin that echoes request headers back (as httpbin's `/headers`) hands the key to the program; grant only origins that do not.
  - Done: a local server receives the header on every method and on a redirect; the program's same-name header refused; the value appears in no output (audit, log, stderr, response to the client); `env:` missing and cleartext refusals with hints; kio-web links signs its webhook with an injected `Authorization`. Independent security review.

## Order

E1 and P1 in parallel (eval/ and the report touch different code); S1 after P1 (both edit host/src/main.rs's flags).
