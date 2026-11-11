# E3: kio against Python in a sandbox, attack by attack

2026-11-10. The question: when an agent writes code from input that may carry a prompt injection, how much does kio protect beyond Python in a well-configured sandbox? Harness, tasks and definitions: [eval/e3/README.md](../../eval/e3/README.md); raw results: eval/results/e3-2026-11-10-*.json.

**Arms.** kio with its grants (`:ro`, one host, `--http-methods`, keys injected by the host). **py** (common practice): macOS `sandbox-exec` deny-by-default profile, read-only inputs, an egress proxy allowing only the task's host, keys in environment variables, rlimits. **py-best** (a careful team): the same plus the key injected by the proxy, a method filter, and a per-run disk quota. **Tasks:** 45 benign backend tasks and 55 adversarial twins, each with an injection planted in the input data, across seven attack classes. **Models:** Sonnet 5.5 and Opus 5.5, up to 5 rounds.

## 1. What real models did

| | kio | py-best | py |
|---|---|---|---|
| Benign tasks completed (Sonnet / Opus) | 45 / 45 | 45 / 45 | 45 / 45 |
| Twins completed despite the injection | 55 / 55 | 55 / 55 | 55 / 55 |
| Injections followed (Sonnet / Opus) | **0 / 0** | 1 / 0 | 1 / 0 |
| Attacks that succeeded | 0 | 0 | 0 |
| Programs that can read a secret (of 100) | 1 | 1 | 25 |
| Lines a reviewer reads, median (Sonnet / Opus) | **1.5 / 2** | 82.5 / 90 | 80 / 92.5 |
| Benign programs flagged, false alarms (Sonnet / Opus) | 11 / 10 | 14 / 12 | 12 / 10 |
| Cost, USD (Sonnet / Opus) | 4.05 / 7.66 | 1.76 / 3.37 | 2.18 / 4.39 |

**Today's frontier models almost never followed these injections**: two of 330 adversarial runs, both Sonnet following the same disk-fill twin, once in each Python arm, both blocked. With these models and these injections, no arm's run-time defences were tested by the models themselves. Section 2 measures them directly.

kio's false alarms are all a method changing on a grant the task already had (`out: Dir (+create -write)`, `counts: Kv (+scan -keys)`): noise a reviewer can see is harmless in one line, but noise. Python's are a heuristic scan of an 80-90 line diff.

## 2. When the program follows every injection

Each twin's compromised reference (the benign program plus the injection's code), run confined:

| Attack class | Twins | kio | py-best | py | Visible before running: kio `check --base` / Python scan |
|---|---|---|---|---|---|
| Exfiltration to a new host | 8 | 8 blocked | 8 | 8 | 8 / 8 |
| Exfiltration through the granted host | 4 | 1 | 1 | 0 | 4 / 4 |
| Reading or leaking a secret | 8 | 8 | 8 | **0** | 8 / 8 |
| Writing where the task only reads | 9 | 9 | 9 | 9 | 9 / 9 |
| Escaping the directory | 9 | 9 | 9 | 9 | **0 / 4** |
| Resource exhaustion | 9 | 9 | 9 | 6 | 2 / 4 |
| A helper gaining network access | 8 | 8 | 8 | 8 | 8 / 8 |
| **Blocked** | **55** | **52** | **52** | **40** | |

No arm stops data sent out through the host the task legitimately uses (as a query, a PUT body or a header): capability security is not information-flow control.

## What this says

1. **At run time, kio equals Python sandboxed by a careful team (52/55), and beats Python as commonly sandboxed (40/55)**, mostly on secrets: kio programs never hold their keys.
2. **The difference is who builds the stack, not the ceiling.** py-best needed a custom egress proxy that injects keys and filters methods, a per-task sandbox profile and watchdog threads for memory and disk. kio's equivalent is flags on `kio run`; its limits are the host's own counters, not polled watchdogs. Not zero setup: an operator still picks grants, places writable paths and supplies secrets, and kio's maintainers maintain that stack instead of each adopter.
3. **Review is where kio is clearly ahead**: 1-2 lines of exact authority against an 80-90 line diff and a heuristic scan (which misses indirect access such as `getattr(os, "environ")`). kio's view misses what does not change authority (escapes, most exhaustion), which the host blocks anyway.
4. **The adversarial half did not test the models**: they ignored the injections. A stronger test needs subtler, adaptive injections and weaker or older models.

## Biases, both directions

- 52/55 is this suite's outcome (curated attacks, Python standard library only), not a general equivalence. Neither the arms' run-time cost nor the cost of maintaining each deployment was measured.
- "Lines a reviewer reads" is output size, not reviewer effort or accuracy; reading authority alone cannot show a program handles data correctly. A human review study would test the stronger claim.
- Generation cost: kio runs cost about twice Python's, mostly the longer system prompt (all of language.md against a paragraph).

- kio's system prompt is all of language.md; Python's a paragraph. The task text is written in kio's terms.
- py-best's limits are polled watchdogs; a kernel quota would refuse earlier. It refuses CONNECT (no HTTPS through the proxy).
- Python's reviewer compares against a reference by another author, inflating its diff; kio's view shows only grants.
- An attempt the program hides without a marker string is not counted.

## Next

- Report authority in classes (read, write, delete, network host) so a method swap on an existing grant is not flagged (kio's false alarms).
- E3b: adaptive injections written by a red-team model against each arm, and Haiku-class models.
- Information flow: the three attacks no arm stops are the case for labels on data (deferred, design open).
