# Proposal: concurrency

For the maintainer to decide. Every option here changes what programs rely on or touches caps.wit, so each is a one-way door. Recommendation first, in section 5.

## 0. Today

```
--listen: request 1 ─► sandbox 1 ─► main ─► response       (parallel across requests, N workers)
          request 2 ─► sandbox 2 ─► main ─► response
inside one run:   main ─► api.get(a) ─wait─► api.get(b) ─wait─► ..   (one thread, one call at a time)
```

A run is one Wasm instance on one thread. Each host call already runs on its own host thread (`State::within`, H5), but the guest waits for it. Refcounts are non-atomic, the allocator is single-threaded, Wasmtime async is off (#79). Decided with the owner for `--listen`: synchronous handlers, no async in the language, the host does concurrency (web-support-todo "Decided").

## 1. What programs need

| Use case | Who | Today | Smallest thing that gives it |
|---|---|---|---|
| Two or more HTTP calls at once (an API that aggregates upstreams; an agent task fetching N pages) | links-style services, agent data jobs | serial `get`, each waiting; a run of 20 calls at 200 ms is 4 s | a batched host call: N requests issued together, one wait |
| CPU fan-out over a list (parse 10,000 files, score N candidates) | data jobs | serial `for`; compiled code is 1.1-3x Rust single-core | `par_map(f)` for a pure `f`, in separate instances |
| Work after the response (count the hit, write the log, notify) | links `follow`: the client waits for `add` and `log.write` | the response is written after the instance drops (web decision 3), so the client waits for all of `main` | `send` flushes at once; `main` keeps running within the request's limits |
| Timeouts on one slow upstream | services | the run's `--timeout`, and `Http`'s fixed 10 s per call | a deadline argument on the call or batch |
| Streaming a response (large file, SSE, model output) | services, agent UIs | buffered bodies, 1 MB in | an `Outgoing` chunk method in caps.wit: orthogonal to tasks |

The first row is what the links app and every "call two APIs" agent task hit first. Rows 3 to 5 need no tasks at all.

## 2. Options

Each example aggregates two upstreams. `api.send` is H16's.

**(a) Structured concurrency.** Tasks spawned in a scope, joined at its end, capabilities only as arguments to a named function.

```rust
fn fetch(api: Http, path: Str) -> Result<HttpResponse, HttpErr> { api.send("GET", path, [], []) }
fn both(api: Http) -> List<Result<HttpResponse, HttpErr>> {
  scope {                                   // every spawn below is joined here
    spawn fetch(api, "/a");                 // a task is a call of a named fn
    spawn fetch(api, "/b");
  }                                         // the scope's value: results in spawn order
}
```

**(b) Async/await.**

```rust
async fn fetch(api: Http, path: Str) -> Result<HttpResponse, HttpErr> { api.send("GET", path, [], []).await }
async fn both(api: Http) -> .. { let a = fetch(api, "/a"); let b = fetch(api, "/b"); join(a, b).await }
```

**(c) Host-level parallel calls only.** No tasks; a few methods and built-ins fan out inside the host.

```rust
let rs = api.send_all([HttpRequest { method: "GET", path: "/a", .. }, HttpRequest { method: "GET", path: "/b", .. }]);
// rs: List<Result<HttpResponse, HttpErr>>, in list order, whatever finished first
let scores = docs.par_map(score);           // score: fn(Doc) -> Int, pure by rule 1: no capability can reach it
```

**(d) Actors: message passing between sandboxes**, each its own instance, each with its own capability parameters, a mailbox per actor.

```rust
fn worker(inbox: Inbox<Job>, out: Sender<Done>, api: Http) { loop { let j = inbox.recv(); out.send(do(api, j)); } }
fn main(api: Http) { let w = spawn_actor(worker, api); w.send(Job { .. }); .. }
```

**(e) Nothing in the language.** `--listen` workers for services; an operator fans a data job out by running `kio run` N times.

### Costs

| | (a) structured | (b) async/await | (c) host calls | (d) actors | (e) nothing |
|---|---|---|---|---|---|
| Language | `scope`, `spawn`, task results | `async`, `await`, futures, `join`/`select`: colors every function | none (two methods, one built-in) | actor types, channels, `spawn_actor` | none |
| Checker | spawn's callee is a named fn; its caps are arguments (rule 1 as is) | async types everywhere; `?` across awaits | a `par_map` argument is a `fn(T) -> U`, cap-free already | channel types; what may cross a channel (no caps) | none |
| Runtime, codegen | tasks as separate instances: a value codec by descriptor (args in, result out), no runtime change. In one instance: stack switching or a CPS transform; atomic refcounts if threads | state machines per `async fn`, or Wasmtime fibers (async on, #79) | the codec for `par_map` only; `send_all` is one import | as (a) plus mailboxes | none |
| Host, Wasm | instantiate per task from `InstancePre` (as a request); `--tasks` bound; sum fuel | async host calls, an executor in the host | `send_all`: N `within` workers, one wait; `par_map`: N instances | long-lived instances, scheduling, backpressure | done |
| Capability model | exact: a task's authority is its fn's report line | exact, but async signatures spread through helpers | exact; `par_map`'s function provably has none | exact per actor; channels must refuse caps | exact |
| Determinism | results in spawn order: values never depend on scheduling (no `select`) | `join` fine; `select`/first-wins makes values timing-dependent | results in list order: as deterministic as `get` is today | receive order is timing: inherently nondeterministic | yes |
| Audit log | each line gains `task`; `seq` stays issue order | as (a) | N call lines sharing a `batch` id | as (a) plus message lines | as today |
| Fuel, limits | one budget per run, split over instances; `--tasks` caps width | one instance: unchanged, but fuel does not tick while awaiting (epoch covers it) | `--calls` counts each sub-call; a batch is at most 64 (today's `MAX_WORKERS`) | per actor, as `--listen` workers | as today |
| Models write it right | mostly: no colors, no handles to leak; the join is implicit | poorly: forgotten `await`, blocking in async, colored helpers | yes: a list in, a list out | poorly: deadlocks, unbounded mailboxes | yes |
| Gives | I/O overlap and CPU parallelism | I/O overlap only | I/O overlap (`send_all`), CPU (`par_map`) | everything, including long-lived pipelines | request-level only |

### Shared state, data races, Wasm threads

kio has no shared mutable objects, so the only things two threads could race on are the **refcount** (two tasks holding one storage block) and the **allocator**. A copy-on-write write is safe once counts are exact: `rt_unique` sees rc > 1 and copies. Two ways to run two tasks at once:

| | Wasm threads (one instance, shared memory) | Separate instances (one memory each) |
|---|---|---|
| Needs | the threads proposal: `shared` memory, atomics; Wasmtime `wasm_threads`; not in the stable component model | nothing new: `InstancePre::instantiate` per task, as `--listen` does per request |
| Runtime | atomic `rt_inc`/`rt_dec` (on the inlined fast path: measurable on tree and closure benchmarks), a lock in the allocator, every `Vec` push behind a unique check | unchanged, single-threaded |
| Passing a value to a task | share storage, inc the count | copy: encode by descriptor, decode in the task (a `Str` or `Bytes` is a memcpy; a tree walks) |
| Fuel | one counter racing | one counter per instance, summed at join |
| Trust | a runtime bug in one task corrupts the other's memory | a task cannot touch another's memory |

Separate instances cost a copy per argument and an instantiation from `InstancePre` (what every request pays today); they keep the runtime as it is and the host's per-instance limits as they are. **Recommend separate instances for every option, and never Wasm threads.** Under that choice (a) and (d) are the same mechanism, a task is an instance, and differ only in lifetime and in whether tasks talk mid-flight.

## 3. How a task gets a capability

Rule 1 holds unchanged: a capability is a parameter, passed as an argument. A task is a **call of a named function**, so its arguments are checked as any call's are; a closure still cannot capture one, and `par_map`'s `fn(T) -> U` cannot name one, so that fan-out is provably pure.

```
spawn fetch(api, "/a")      report: fetch : api send         main : api send   (through fetch, as any helper)
docs.par_map(score)         report: score : none             main unchanged
```

Nothing new is minted: the task's instance receives the same handle table (the host holds it; the guest sees indices). The report stays exact because it is read off signatures, and a task's calls are charged to its function's line, as a helper's are today. Attenuation stays deferred.

## 4. Determinism and replay

The guarantee to keep: **a program's value never depends on scheduling.** Only timing-dependent host results (`Timeout`, a `Clock` value) do, and those are already so today.

| Rule | (c) `send_all` | (a) `spawn`/`scope` |
|---|---|---|
| Result order | list order | spawn order |
| Effects inside one task | program order | program order |
| Effects across tasks | one host call each: unordered among themselves | unordered until the join; a `Kv` call is atomic as today, so `swap`/`add` are the coordination |
| Audit `seq` | issue order; each line adds `"batch":k` | issue order; each line adds `"task":t`, `"parent":p` |
| First-wins (`select`, in-language `timeout(ms, task)`) | not offered | not offered: a deadline is an argument the host enforces (`HttpRequest.timeout_ms`), so a late call is `Err(Timeout)` at a known position, not a different control path |

Replay today means rerunning with the same grants and fixed `Rng`/`Clock`, since the log records lengths, not values; that stays true. The log can state, per task, the calls it made and when; a reader can reconstruct what overlapped from `ms`.

## 5. Recommendation and plan

Build **(c) first, then (a) as isolated instances** when a program needs CPU fan-out or tasks with capabilities; never (b), (d) as a general model, or Wasm threads.

| Stage | Builds | Unlocks | Door |
|---|---|---|---|
| 1 (with H16) | caps.wit `http.send-all: func(reqs: list<http-request>) -> list<result<http-response, http-err>>`; host issues each on a `within` worker, waits for all until the deadline; at most 64 a batch; each counts toward `--calls` and `--io`; `HttpRequest.timeout_ms` | parallel HTTP, the first need; a deadline per call | caps.wit: maintainer |
| 1b (host only) | `Outgoing.send` hands the response to the connection at once; `main` continues under the request's fuel and timeout; the audit trailer marks calls after `send` | background work after a response (links `follow` counts after redirecting) | web decision 3 amended; a two-way door |
| 2 (when a data job needs it) | `List.par_map(f: fn(T) -> U) -> List<U>`: N instances from the same `InstancePre`, args and results through a descriptor codec in the runtime, width `--tasks` (default: cores), fuel summed | CPU parallelism with no runtime or checker change beyond the codec | a built-in: pinned in lang-todo, interpreter runs it serially |
| 3 (deferred) | `scope { spawn f(caps.., args..); }` over the stage-2 mechanism: an instance per spawn sharing the parent's handle table, joined at scope end, `task` in audit lines | tasks with capabilities: fetch-then-write fan-out | language: maintainer |
| Not planned | async/await; actors with mailboxes; `select`; Wasm threads; streaming as a task feature (its own caps.wit `Outgoing` decision) | | |

Stage 1 is one method and no language change, and it covers the links app and most agent tasks. Stage 2 is the smallest step that makes the compiled language parallel, and its codec is what stage 3 reuses.

### Decisions

1. Add `send_all` to caps.wit with H16, with `timeout_ms` per request, and no concurrency construct in the language for now. **Recommend yes.**
2. Values independent of scheduling: results in list or spawn order; no `select` or first-wins ever. **Recommend yes.**
3. Parallel tasks are separate instances; the runtime stays single-threaded and non-atomic; Wasm threads are ruled out. **Recommend yes.**
4. A task is a call of a named function; capabilities reach it only as arguments; closures stay capability-free; the report is unchanged in form. **Recommend yes.**
5. `send` flushes the response at once and `main` may continue (stage 1b). **Recommend yes**, logged in open_questions as a two-way door.
6. Timeouts are host-enforced arguments and flags, never a language construct. **Recommend yes.**
7. Fuel for tasks: one run budget, each instance given the remainder and the sum checked at join; `--tasks` bounds width and so memory (`--tasks` x `--memory`). **Recommend yes.**
8. Async/await: refuse, keep `async`/`await` reserved. **Recommend yes.**
