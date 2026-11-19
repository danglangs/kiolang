# T4: a fake host for tests (design, 2026-11-18)

A2 #5: a program that calls a webhook cannot test what it sends without a server running (kio-web's flow keeps `dest.py` for this), and the review page says `tested` without saying against what. The decision a person should make from a test alone: *on these inputs the program sends exactly these requests, and handles these answers (503 then 200, a timeout)*. Established by `kio test` (the host answers from a file the test author wrote, and records what was sent into a `Dir` the test holds) and shown by `kio review` (the file's hash in Identity, the fake named on the page). No change to caps.wit or to the language: the fake sits in the host behind the same `http` interface, so compiled code and the interpreter see nothing new.

## The grant

In a test grants file only (`kio test --grants`, `kio review --test-grants`): `--http hook=fake:hook.http` binds `hook: Http` to the entries of `hook.http` (a path relative to the working directory, as every grant path). `--http-methods hook=POST` and `--http-header hook=..` apply as to a real grant.

```
# hook.http: what `hook: Http` answers. One entry per response, `---` between.
# `METHOD /path`, then a status or `timeout`, response headers, a blank line, the body.
POST /v1/events
503
---
POST /v1/events
200
content-type: application/json

{"ok":true}
---
GET /v1/status?full=1
200

up
---
POST /v1/slow
timeout
```

| Part | Rule |
|---|---|
| Request line | one of `GET HEAD POST PUT PATCH DELETE OPTIONS`, a space, the path exactly as the program passes it (query included) |
| Status | `100`-`599`, or `timeout` (`Err(Timeout)` at once, no waiting); a 3xx is answered as written, never followed |
| Match | method and path equal; the n-th entry for one `METHOD /path` answers the n-th such request, the last one repeats (so `503` then `200` tests a retry); `send_all` counts in list order |
| No entry | `Err(Denied)`, as a real grant refuses a path outside it; the request is kept as *unmatched* and listed under a `FAIL` (`hook: 3 requests, 1 unmatched: POST /v1/evnts`) |
| Before the lookup | the host's own checks run first and alike: a bad path or a method not granted is `Denied`, a bad header or method `Invalid`, a body over 8 MB `TooLarge`; `get` turns a non-2xx into `Err(Status(n))` |
| File | UTF-8, at most 1 MiB, read once before the first test, bodies inline (no includes; a body line cannot be `---`); response header names are lowercased, in order, as a real response's |

## Refused

Every message names the flag and the file line where there is one.

| Where | Message | Hint |
|---|---|---|
| `kio run`, `--listen`, `kio check --grants`, `kio review --grants`/`--entry-grants` | `` --http hook=fake:hook.http: a fake answers tests only `` | `grant a host (--http hook=https://hooks.example); a fake belongs in the grants file kio test reads` |
| `kio run` and the above | `` `--http-record` is not a flag of kio run `` | `it records a fake's requests for kio test` |
| FILE | `hook.http:7: expected METHOD /path (GET, HEAD, POST, PUT, PATCH, DELETE or OPTIONS)`; `hook.http:8: expected a status 100-599 or timeout`; `hook.http:10: expected name: value` | |
| FILE and the grant disagree | `hook.http:3: PUT /x: --http hook grants only GET, POST` | `add PUT to --http-methods hook, or drop the entry` |
| FILE itself | the rules of `--grants`: missing or not a regular file, over 1 MiB, other hard links, inside a writable `--dir` the same file grants (the program could rewrite its next run's answers) | `give kio a fake file of its own` |

## What a test reads back

The open question: a test must assert what it sent, without a new method on `Http` (caps.wit is a one-way door).

| | How the test asserts | Needs | Tradeoff |
|---|---|---|---|
| **1. Record into a `Dir` the test holds** (recommended) | `--http-record hook=sent`: each request on `hook` is written, before its answer returns, as the file `hook-1`, `hook-2`, .. in the test's `sent: Dir`; the test reads it and uses `assert`/`assert_eq` beside the call | one test-only flag; nothing in caps.wit or the language | the test parses a small text file (HTTP's own shape, below); assertions stay in the test code where every other failure is reported; any body, `send_all`, retries |
| 2. Expectations in FILE | an entry carries the headers and body the request must have; a mismatch fails the test from the host with a diff | nothing in caps.wit; a richer FILE | no parsing, but the oracle leaves the test code; partial-match rules (which headers count?) grow; one FILE per grant name, so a scenario per parameter name |
| 3. `--http-expect hook=POST /v1/events` lines | the host checks the list after the test | nothing | a grants-file line cannot carry a body or headers legibly; otherwise option 2, weaker |
| 4. `hook.sent() -> List<HttpRequest>` | a method call | **caps.wit and language change** (a method that exists only in tests: what does `kio run` return?) | cleanest to write; the maintainer's door, not proposed before 1 is in use |

Recommendation: 1 now; 2 can be layered on the same FILE later if parsing turns out to be the friction; 4 only on evidence from 1.

The record is the request as HTTP writes it, bytes exact; an injected header shows its name and `<injected>`, never the value:

```
POST /v1/events
content-type: application/json
authorization: <injected>

{"id":1}
```

```
#[test]
fn deliver_retries(hook: Http, sent: Dir) -> Result<(), IoErr> {
    deliver(hook, Event { id: 1 });                          // hook.http: 503, then 200
    assert_eq(sent.list("")?.len(), 2);
    let second = sent.read("hook-2")?.to_str();
    assert(second.starts_with("POST /v1/events\n"), "method and path");
    assert(second.ends_with("\n\n{\"id\":1}"), "body");
    Ok(())
}
```

`--http-record hook=sent` is refused when `hook` is not a fake (`hook is not a fake; a real grant's requests are in the audit log (--audit)`) or when the file also grants `--dir sent=..` (`records go to a fresh temporary Dir; remove --dir sent`): the record always lands in a default temporary `Dir`, so nothing stale from an earlier run is read, and the host writes it through that directory's open handle (cap-std), never by path. A test that has `hook` but no `sent: Dir` runs without a record, as any grant for a parameter a test lacks is ignored. A record's bytes beyond its body (request line, headers, blank line) are charged to `--io` before it is written, so records stay within `--io` (the body is already charged, as sent); a record is not a host call. A record no test can take (no test has both `hook: Http` and `sent: Dir`) is refused at setup.

## Interactions

| Flag or feature | With a fake |
|---|---|
| `--http-methods` | enforced before the lookup, as on a real grant; a method the program's report sends and the grant lacks is refused at startup, as `kio run` does; a FILE entry for an ungranted method is refused (above) |
| `--http-header` | accepted; the secret is read at start under the usual rules (an unset `env:` fails the tests granted it, as `--hmac` does); the fake never receives the value, only the name (`<injected>` in the record); a program header of the same name is `Invalid`; the loopback-only rule for `http://` does not apply, nothing crosses the network |
| Redirects | not followed: a 3xx is the response (`send`) or `Err(Status(3xx))` (`get`) |
| `send_all` | each request matched and recorded in list order; over 64 is `Invalid` for all, nothing recorded; results in list order as today |
| `timeout_ms` | validated as today (negative is `Invalid`); only a `timeout` entry produces `Err(Timeout)` |
| Fuel, limits | each request takes one `--calls` and its bytes count toward `--io`, both ways, as a real one; fuel is unchanged (a host call) |
| Audit log | `kio test` has none and keeps none; the record is the test's own account of what was sent |
| Properties | still may not take `Http` (the message's reason, repeating real requests, no longer holds for a fake: a later task may allow a fake there) |

## `kio review`

- Identity: `test fake hook.http sha256:..` for each fake in T (and `--json` `identity.test_fakes[]`), read again at the end, a change is an `error` as for every other file.
- Tests: the grants line names the fakes, `tested  grants: test.grants (hook answered from hook.http)`. Tests that pass against a fake stay `tested`: they are evidence of the program's logic against the answers written down, and the page now says what answered, which is what A2 #5 asked for.
- Not checked: `not checked  hook: Http: tests ran against hook.http, not a server; the real endpoint's answers are untested`.
- T may give a fake (read-only stimulus) and records go to temporary `Dir`s, so the shared-state rule (no writable `--dir`, no file-backed store in T) is untouched.

## Security

- `fake:` is never parsed into an origin. `Origin::parse` refuses the prefix by name (today it says `bad port`, which teaches nothing). Only `kio test`'s per-test binder (`run_once`, after `test_setup` has taken every grant from the file) turns a `fake:` spec into a bound fake and hands it to `run_program` as an object, not a flag string; `kio run`, `--listen`, `kio check --grants`, `kio review --grants` and `--entry-grants` go through `Origin::parse` and refuse. `--audit`, `--listen`, `--host`, `--ws-origin` stay refused in a test grants file.
- FILE is read once, bounded, inline bodies only: it reaches no other file. The `--grants` identity checks (device and inode, not the path) keep it out of any writable granted `--dir` and refuse other hard links, so a test cannot rewrite the answers for the next run.
- The record is written only into a default temporary `Dir` (mode 0700, removed after the test), through its open handle, under the fixed name `<grant>-<n>` (a grant name is a kio identifier: no path characters). No value from `--http-header` is ever written; nothing from FILE reaches an error message but its line number and request line.
- The interpreter is untouched: the fake is below caps.wit, so compiled and interpreted programs see one `Http`.

## Tasks

- **T4a. The fake grant.** `fake:` in the test binder, FILE parse and matching, `timeout`, unmatched listing, the refusals (`Origin::parse`, FILE rules, methods), guide Tests and Http sections. Done: a POST matched answers its status, headers and body; `get` on a 503 entry is `Err(Status(503))`; the second entry for one `METHOD /path` answers the second request and then repeats; an unmatched path is `Err(Denied)` and listed in the `FAIL` block; `timeout` returns in under 100 ms; `kio run`, `--listen`, `kio check --grants` and `kio review --grants` refuse `fake:` with the message and hint; each FILE error names its line; a FILE inside a writable `--dir` or with another hard link is refused; `--http-methods hook=POST` makes `send("PUT", ..)` `Denied` before any lookup; `--http-header`'s value appears nowhere in output (asserted by grep over stdout, stderr and the temporary directory). host/tests/kio_test.rs.
- **T4b. `--http-record`.** Done: `hook-1`, `hook-2` for a `send_all` of two, in list order; a binary body byte-exact; `authorization: <injected>`; `sent.list("")` counts them; refused for a non-fake, for a `sent` the file grants, and by `kio run`; a test without `sent: Dir` runs without a record.
- **T4c. The review page.** Done: the Identity, grants and Not checked lines above in text and `--json`; a FILE changed during the review is an `error`. host/tests/review.rs. Then kio-web's flow tests replace `dest.py` with a fake and a record.
- Later, on evidence: a property may take a fake `Http`; expectations in FILE (option 2).
