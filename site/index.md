<!-- The landing page. Text is taken from README.md; keep the two in step. Links are relative to the repo root. -->

<div class="hero">
<img src="docs/assets/kio-unicorn.svg" width="120" height="120" alt="kio mascot: a unicorn whose horn is a key">
<h1>kio</h1>
<p class="tagline">A general-purpose language for the agentic world: agents write and review the code; the language enforces authority, side effects and audit, deterministically.</p>
<p class="cta"><a class="button" href="docs/first-job.md">Your first job in 10 minutes</a> <a class="button secondary" href="https://github.com/danglangs/kiolang">Source on GitHub</a></p>
</div>

kio makes agent-written software easier to trust without someone reading every line. Agents write and revise the implementation; you approve explicit authority and independently checked evidence, and read the code where that evidence is not enough.

What kio checks, independently of the agent that wrote the code:

- **Authority:** which resources a program can reach and which operations it may perform, derived from its code (`kio check`).
- **Enforcement:** the host refuses everything outside the operator's grants; designed and tested against hostile code ([E3](docs/proposals/e3-scorecard.md), E3c), not independently audited.
- **Audit and replay:** every host call is logged; a fixed clock and seed reproduce a run when the other inputs and host responses are the same.
- **Behavioural evidence:** compiled tests, generated properties and mutants show which behaviours were exercised and which faults the tests catch (`kio test`, `kio review`).
- **Resource evidence:** fuel and memory measured per test expose regressions on tested workloads.
- **Review:** one labelled page of what changed, what was checked and what remains unknown ([walkthrough](docs/review-a-change.md)).

The advantage is integration: authority is part of the programming model, and running, testing and review share it. And it is practical: agents write it from its reference, and it compiles to native code ([agents](#use-from-an-agent), [performance](#performance)).

The strongest guarantee today is authority over resources. The broader correctness claim is promising and still being validated: passing tests and killed mutants do not show that the requirements are right or every behaviour correct. The aim is a general-purpose language where trust scales better than manual code review, not one that makes review or judgment unnecessary ([trust](docs/proposals/trust.md)).

kio is experimental (v0). It runs file jobs, API integrations and stateful web services today; its libraries and tooling are young ([maturity and limits](#maturity-and-limits)).

## An agent refactors your code. The tests pass.

`tally` sums the cents column of a CSV of orders ([examples/review](examples/review)). An agent pulls row parsing into a helper, `cents`, and adds one condition on the way:

```rust
fn cents(line: Str) -> Option<Int> {
  let f = line.split(",");
  if f.len() != 2 { return None; }
  let c = f[1].trim().parse_int()?;
  if c > 0 { Some(c) } else { None }
}
```

`c > 0` drops refunds (negative amounts) from the total. All three tests pass and the authority is unchanged, so the page exits 0. But both mutants that survive sit on the amount condition: no test pins which amounts count. (Output shortened; the full page is in [the walkthrough](docs/review-a-change.md#c-a-change-that-stays-inside-its-authority-and-is-wrong).)

```
$ kio review c/tally.kio --base v1/tally.kio --requirements README.md --grants c/tally.grants
...
Authority
  enforced     no change in authority
...
Tests
  ...
  tested       new: 3 passed, 0 failed
  ...
  tested       mutants: 8 of 10 killed (generated at seed 1: 11 drawn of 11 sites, 1 did not check)
  tested       killed by: main_writes_the_total 8
  not checked  survived: generated: c/tally.kio:7:8: cents: `>` -> `>=` in `if c > 0 { Some(c) } else { No...`
  not checked  survived: generated: c/tally.kio:7:10: cents: `0` -> `1` in `if c > 0 { Some(c) } else { None...`
...
$ echo $?
0
```

The reviewer asks for one example: 100 cents of sales minus a 30-cent refund (and a zero row) totals 70.

```rust
#[test]
fn refunds_count_against_the_total() {
  assert_eq(total("id,cents\n1,100\n2,-30\n3,0\n"), 70);
}
```

```
$ kio review c2/tally.kio --base v1/tally.kio --requirements README.md
...
  error        FAIL refunds_count_against_the_total
                 c2/tally.kio:47:3: assert_eq failed: left `100`, right `70`
...
$ echo $?
1
```

The change is rejected, with a counterexample anyone can rerun. kio did not prove the program correct; it showed what the passing tests had not established and made the missing evidence executable.

When a change adds authority, the diff says so first. An edit to the [orders job below](#a-small-kio-program) that deletes its input after import is one line in `kio check --base` (`~ orders: Dir (+remove)`, exit 3), and under the read-only grant it does not start: ``kio: `orders` is granted `:ro` but the program can write it (`orders remove`)``.

## Quickstart

macOS on Apple silicon (Linux x86_64 and checksums: [first job in 10 minutes](docs/first-job.md)):

```
$ curl -L https://github.com/danglangs/kiolang/releases/latest/download/kio-aarch64-apple-darwin.tar.gz | tar xz && cd kio-aarch64-apple-darwin && ./kio run hello.kio
hello, kio
```

Every flag, limit and output: [docs/guide.md](docs/guide.md); building from source: [its last section](docs/guide.md#from-source).

## A small kio program

A job reads today's orders and posts them to a CRM ([examples/glance](examples/glance)):

```rust
fn main(orders: Dir, crm: Http) -> Result<Int, Str> {
  let csv = orders.read("today.csv").map_err(|_| "no orders")?;
  let r = crm.send("POST", "/v1/import", [], csv, 5000).map_err(|_| "crm down")?;
  Ok(r.status)
}
```

```
signature   which resources the code receives       orders: Dir, crm: Http
report      which operations it calls on each       main : orders read, crm send POST
grants      which real resources, and limits        --dir orders=./orders:ro --http crm=api.crm.example
```

The operator grants exactly that: the orders read-only, one host with POST added to GET, and a key the host adds and the program never sees.

```
$ kio run sync.kio --dir orders=./orders:ro --http crm=api.crm.example \
    --http-methods crm=POST --http-header crm=Authorization:env:CRM_KEY
```

There is no `import`, global or ambient I/O: a function without a capability parameter cannot do I/O, and the checker proves it. kio compiles to Wasm and runs in Wasmtime; the host enforces every grant again, with fuel, memory, time and call limits, and logs every call.

## Real applications

### Flow: a durable workflow service

[flow](https://github.com/danglangs/kio-web/tree/main/examples/flow) (agent-built, in [kio-web](https://github.com/danglangs/kio-web)) accepts signed events (`POST /events`, HMAC-verified, stored once per event id in SQLite) and stores each as a job. A worker claims due jobs with a lease, POSTs each to one destination with a key the host adds, retries with exponential backoff, and marks a job dead after 5 attempts. A crash after the POST and before recording it repeats the delivery, so every attempt carries the same `Idempotency-Key` and the destination can deduplicate.

What the worker can reach (`kio check worker.kio`, shortened):

```
main            : db exec query, now now_ms, dest send POST
deliver         : dest send POST
flow::claim     : db exec query
flow::finish    : db exec
flow::init      : db exec
flow::try_claim : db exec query
...
```

18 tests, five of them properties, exercise stale workers, expired leases, attempt limits and crashes before and after the POST against a reference model. Four written mutants each break one rule, and the tests catch all four:

| Mutant ([flow.mutants](https://github.com/danglangs/kio-web/blob/main/examples/flow/flow.mutants)) | What it breaks |
|---|---|
| the done update ignores the lease | a stale worker completes a job it lost |
| no `Idempotency-Key` header | the destination cannot deduplicate a retry |
| the claim `UPDATE` without its conditions | two workers take one job |
| `>` for `>=` on the attempt limit | one attempt too many |

```
$ FLOW_KEY=test-key kio review worker.kio --base worker.kio --test-grants test.grants --mutations flow.mutants --mutants 0
...
  tested       mutants: 4 of 4 killed (flow.mutants (inside NEW's tree) 4 of 4)
...
```

Still for a reader: the SQL text (the report says `db exec`, not which rows) and the destination's deduplication, which kio cannot see.

### More in kio-web

| App | What it demonstrates |
|---|---|
| [payhook](https://github.com/danglangs/kio-web/tree/main/examples/payhook) (agent-built) | signed Stripe-style webhooks, replays dropped in SQLite, a chat webhook with retries; `Hmac` that can verify, never sign |
| [links](https://github.com/danglangs/kio-web/tree/main/examples/links) | a link shortener in 379 lines: hashed bearer tokens, a `Kv` that survives restarts, expiry, random slugs, a log, a webhook, HTML and static files |
| [notes](https://github.com/danglangs/kio-web/tree/main/examples/notes), [kvnotes](https://github.com/danglangs/kio-web/tree/main/examples/kvnotes), [sqlnotes](https://github.com/danglangs/kio-web/tree/main/examples/sqlnotes) | one JSON API over a `Dir`, a `Kv` and a `Db`: the same routes, three different authority reports |
| [ws](https://github.com/danglangs/kio-web/tree/main/examples/ws) | WebSockets, open only to clients holding a token stored in a `Kv` |

Two further held-out agent-built apps, built from the public docs alone, are local and unpublished; what they hit became diagnostics, review and docs fixes.

## Why a new language?

Python can be sandboxed as tightly, with work. On 55 prompt-injection attacks ([E3](docs/proposals/e3-scorecard.md)):

| | kio | Python, careful sandbox | Python, common practice |
|---|---|---|---|
| Attacks stopped at run time | 52/55 | 52/55 | 40/55 |
| What it took | flags on `kio run` | an egress proxy that injects keys and filters methods, a profile per task, watchdogs | a profile and a proxy |
| What a reviewer reads for a change's permissions (median lines) | 1-2, exact | 80-90 of source diff; scanners guess | 80-93 |

kio restricts how capabilities can appear in programs, which makes authority directly checkable. With Python, comparable run-time restrictions can be enforced from outside, but a complete authority summary needs further restrictions on the language or more tooling. Neither checks what a program does inside its grants: one that may read `customers/` and call `partner.example` can send the customers to the partner.

## What works

v0, working end to end.

| | |
|---|---|
| Language | structs, enums, generics, closures, `match` with nested and `\|` patterns, `let`-else, constants, `Float`, `HashMap`, built-in JSON, SHA-256 |
| Resources, each a grant | files (`Dir`), a key-value store (`Kv`), SQLite (`Db`), an HTTP client (`Http`: any granted method, headers, parallel `send_all`), `Rng`, `Clock`, `Log`, `Hmac` (sign and verify under a key the program never holds), `Secret` (a value the program can compare, never read); serving HTTP and WebSockets |
| Safety | the authority report before a run; a Wasm sandbox with fuel, memory, time, call and I/O limits; an audit log |
| Review | `kio review`: a change's authority diff, tests on both sides, fuel and memory per test, mutants of the changed code (generated, or written by the reviewer) and the tests that caught them, and what no check covers, on one page; dependencies listed in `kio.deps`, pinned by sha256 |
| Tests | `#[test]` functions with `assert`/`assert_eq`; `kio test` runs each compiled, with throwaway grants (a temp dir, in-memory stores, a fixed clock, a seeded random source). A test with data parameters is a property: 100 generated inputs, a failure shrunk to a minimal input. An `Http` grant can be answered from a file (a fake host) |

## Maturity and limits

- **A good fit:** agent-written jobs and services where you want authority you can review and enforce: file processing, API integrations, webhooks, web services with state.
- **Not yet:** work that leans on a library ecosystem (local dependencies pinned by hash, no registry; no FFI by design), or that already runs in a maintained hardened sandbox. Agents spend about 2x the tokens writing kio as Python today.
- **Reviewed by authority, not proof.** A coding agent that read only kio's docs wrote [payhook](https://github.com/danglangs/kio-web/tree/main/examples/payhook) in five steps; a reviewer approved each from its authority diff and the tests (124 checks), without reading the code. The diffs said what it could touch (verify, never sign; POST to one host), not whether the signature result is acted on, what text goes to the chat, or which rows the SQL touches; the tests carried those, and one gap neither caught (a run that dies during an event's third attempt leaves it pending, never retried).

## Use from an agent

`kio mcp` is an MCP server on stdio with two tools: `kio_primer` (idioms, then the language reference) and `kio_check` (errors come back as `{file, line, message, hint}`). There is no run tool: grants are the operator's decision.

```json
{ "mcpServers": { "kio": { "command": "kio", "args": ["mcp"] } } }
```

Given the language reference, Haiku, Sonnet and Opus 5.5 pass 42-43 of 45 backend tasks first try (Python: 39-45) and all 45 by try 5 ([eval table](docs/roadmap.md#can-models-write-kio)); without it, models write full Rust and rarely finish.

## Performance

Compiled to native code through Wasm, steady execution runs at 1.1-2x Rust on most work and 2-5x where it allocates heavily; each `kio run` also pays a compile and startup cost (about 26 ms for an empty program on an Apple M3 Max, with the runtime precompiled into the binary; [speed plan](docs/proposals/speed-plan.md)).

## Roadmap

- **Available today:** everything under [What works](#what-works), in the [latest release](https://github.com/danglangs/kiolang/releases/latest).
- **Being evaluated:** a deterministic gate that rejects changes with no reader, generator and reviewer agents against Python at equal evidence, and outside developers reviewing real changes (tasks G1, G2 and W1 in [product-todo](docs/product-todo.md)).
- **Proposed:** [grouping capabilities](docs/proposals/cap-groups.md), so a helper takes one group instead of five parameters.

Docs: [review a change](docs/review-a-change.md), [every grant, flag and limit](docs/guide.md), [the language](docs/language.md), [kio by example](docs/primer.md).
