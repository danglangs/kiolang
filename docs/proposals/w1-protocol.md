# W1: review decisions, protocol

Status: draft, 2026-11-16. The test of the project ([trust.md](trust.md)): with `kio review`, does a reviewer make correct decisions on changes while reading less code?

## What a reviewer does

For each change: get the two versions, run `kio review` ([walkthrough](../review-a-change.md)), read what they choose to, and record one decision before seeing the key.

| Decision | Correct when |
|---|---|
| Accept | the change does what its requirement says and nothing more |
| Reject | it adds authority not asked for, breaks behaviour, weakens a test, or regresses cost beyond what is stated |
| Ask for evidence | what decides it is not covered by the page; the reviewer names the test or the function that would settle it |

Asking for evidence is correct where the page does not cover the defect; an accept there is wrong even if the code happens to be fine to look at.

## The change set

Six kinds, each a change to kio-web's flow (F1) with a one-paragraph requirement, written by an agent that does not see this protocol's results, and classified before any reviewer sees it:

| Kind | Expected decision |
|---|---|
| A valid change | accept |
| A permission added that the requirement does not ask for | reject |
| A behavioural bug inside existing permissions, caught by a test | reject |
| A test or property weakened or removed | reject, or ask |
| A resource regression | reject, unless the requirement accepts it |
| A defect the evidence does not detect | ask for evidence |

The changes and the key are kept outside both repositories until every reviewer has finished; a reviewer who has seen them is not a reviewer. Several changes of each kind exist, so a reviewer sees each kind once in a random order, and the order of kinds is not fixed.

## What is recorded, per change

Decision; correct or not against the key; for an ask, what was asked for; implementation lines read (the reviewer notes each file and line range opened); minutes from start to decision; anything confusing on the page; whether the mutant lines (R1b) were used: a survivor read, a `killed by:` test opened, a `--mutations` file written to ask for evidence.

## Arms

kio review against careful Python (E3's py-best setup, the same requirement, a plain diff and the test output). A reviewer never sees the same change in both arms: each arm draws different changes of the same kinds. One or two reviewers give qualitative evidence, not a general result.

## Runs

| Run | Who | Counts as |
|---|---|---|
| Dry run | the maintainer, on review-a-change.md's examples | finds confusion in the doc and the page; not data |
| Agent run | an agent given only the public docs | reported as agent-run |
| W1 | outside developers | the result |

Done: the change set built and sealed; the dry run's findings fixed; results per reviewer in eval/results/w1-<date>.json with a short report.
