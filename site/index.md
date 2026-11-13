<!-- The landing page. Text is taken from README.md; keep the two in step. Links are relative to the repo root. -->

<div class="hero">
<img src="docs/assets/kio-unicorn.svg" width="120" height="120" alt="kio mascot: a unicorn whose horn is a key">
<h1>kio</h1>
<p class="tagline">A general-purpose language for code AI agents write: review the contracts and the evidence, not every line.</p>
<p class="cta"><a class="button" href="docs/first-job.md">Your first job in 10 minutes</a> <a class="button secondary" href="https://github.com/danglangs/kiolang">Source on GitHub</a></p>
</div>

kio is an experimental (v0) language and runtime for generated software whose important guarantees can be checked independently of the agent that wrote it: people approve intent, authority and evidence; machines handle the implementation. It combines explicit authority, enforced isolation, value semantics and a compiler built for predictable performance.

The first contract is authority, and it works today: what a program can touch is checked from its code, the host enforces it, and `kio check --base` prints what a change adds in one line. The safety you would otherwise assemble around Python (a sandbox, an egress proxy, a secret store, an authority report) is built in.

## One change, reviewed

An agent writes a job: read today's orders, post them to the CRM.

```rust
fn main(orders: Dir, crm: Http) -> Result<Int, Str> {
  let csv = orders.read("today.csv").map_err(|_| "no orders")?;
  let r = crm.send("POST", "/v1/import", [], csv, 5000).map_err(|_| "crm down")?;
  Ok(r.status)
}
```

Three layers, each answering one question:

```
signature   which resources the code receives       orders: Dir, crm: Http
report      which operations it calls on each       main : orders read, crm send POST
grants      which real resources, and limits        --dir orders=./orders:ro --http crm=api.crm.example
```

The operator grants exactly what the job needs: the orders read-only, one host with POST added to GET, and a key the program never sees.

```
$ kio run sync.kio --dir orders=./orders:ro --http crm=api.crm.example \
    --http-methods crm=POST --http-header crm=Authorization:env:CRM_KEY
```

Next week an instruction hidden in a ticket the agent reads (a prompt injection) makes it edit `sync.kio` to "archive the file after import". Before reading the diff, compare the edited file with the version on `main`; the permission change is one line:

```
$ git show main:sync.kio > /tmp/base.kio
$ kio check sync.kio --base /tmp/base.kio
~ orders: Dir (+remove)
authority added (exit 3); --detail lists each function
```

Shipped anyway, it does not start:

```
kio: `orders` is granted `:ro` but the program can write it (`orders remove`)
  hint: drop `:ro`, or remove the write
```

| To get this | kio | Python |
|---|---|---|
| Only `./orders`, read-only | `--dir orders=./orders:ro` | a sandbox profile or container mounts |
| Only `api.crm.example`, GET and POST | `--http`, `--http-methods` | an egress proxy with a method filter |
| A key the code never holds | `--http-header crm=Authorization:env:CRM_KEY` | a proxy that injects it |
| What a change adds | `kio check --base`, exact | read the source diff; scanners guess |

Review permission changes through the authority diff; use code review and tests for what the program does with them.

## Why a new language for generated software?

- **"Can't I just sandbox Python?"** Yes, with work. On 55 prompt-injection attacks ([E3](docs/proposals/e3-scorecard.md)), Python sandboxed by a careful team stopped 52, the same as kio; it took an egress proxy that injects keys and filters methods, a sandbox profile per task and watchdogs. In kio that is flags on `kio run`.
- **"How do I review a change?"** Permissions through the authority diff: 1-2 exact lines in E3, against an 80-90 line source diff where scanners guess. What the program does with those permissions (an SQL statement, a payload, a deletion path) still takes code review and tests.
- **"Can agents write it?"** It is a Rust subset on purpose. Given the language reference (`kio mcp`), Haiku, Sonnet and Opus pass 42-43 of 45 backend tasks first try (Python: 39-45) and all 45 by try 5.
- **"What does it not check yet?"** Whether the code is right inside its grants: a program can stay within every permission and still compute the wrong invoice, or send `customers/` to a granted partner. Today that takes tests and code review; checked correctness evidence is the next stage.

| | kio | Python, careful sandbox | Python, common practice |
|---|---|---|---|
| Attacks stopped at run time ([E3](docs/proposals/e3-scorecard.md)) | 52/55 | 52/55 | 40/55 |
| What it took | flags on `kio run` | a proxy, profiles, watchdogs | a profile and a proxy |
| Authority summary vs source diff (median lines) | 1-2 | 80-90 | 80-90 |

## What is kio

- **Capabilities are parameters.** `main` names each resource it needs (`input: Dir`, `api: Http`, `db: Db`); the operator grants each one by a flag (or a [grants file](docs/guide.md#grants-file)); there is no `import`, global or ambient I/O. A function without a capability parameter cannot do I/O, and the checker proves it.
- **The host is the boundary.** kio compiles to Wasm and runs in Wasmtime; the host enforces every grant again, with fuel, memory, time and call limits, and logs every call.
- **A real language.** Structs, enums, generics, closures, `match`, JSON; files, a key-value store, SQLite, HTTP client and server, WebSockets, HMAC, each a grant. Native code at 1.1-2.3x Rust on compute.

kio runs file jobs, API integrations and stateful web services today; its libraries and tooling are young.

## Install

No Rust needed: [your first job in 10 minutes](docs/first-job.md) downloads a [release](https://github.com/danglangs/kiolang/releases/latest) (macOS Apple silicon, Linux x86_64) and runs a program that POSTs to an API with a key it never sees.

From source:

```
rustup target add wasm32-unknown-unknown
cargo install --git https://github.com/danglangs/kiolang kio
```

Then: [every grant, flag and limit](docs/guide.md), [the language](docs/language.md), [kio by example](docs/primer.md).

## Roadmap

- **Now:** a flagship application, a durable workflow service (webhook ingestion, a job queue, retries, idempotency, an API and a CLI), to prove kio at a larger scale, each change reviewed with `kio review`: its authority diff, `kio test` results (properties included), resource changes and what no check covers, on one page.
- **Next:** packages, and narrowing a capability for a helper (a subdirectory, read-only, one host), as the application needs them; benchmark gates in CI so speed never regresses silently.
- **Later:** contracts the application shows it needs; `par_map` and tasks (pinned).
