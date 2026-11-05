# Hack 42 submission: kio

Deadline: HACK ticket in **Submitted Project** by Thu Nov 5, 11:59 p.m. PT. Add yourself to **Team Members**. Set Submitted early to get a Science Fair booth. Presentation: Round 2, 1:30-3:30 p.m. PT.

## Jira ticket

**Summary:** kio: a programming language where AI-written code can only touch what you grant it

**Problem.** Agents write code faster than people can review it. In Python, Go or Rust, any line can read a file or open a socket, so checking what agent code can do means reading every line, and a prompt-injected agent can hide one bad line among hundreds.

**What kio is.** A small, statically typed language with Rust syntax where files, network hosts and sockets exist only as capabilities passed in as parameters. A function cannot touch anything not in its signature; the checker rejects the attempt before the program runs, and the host refuses it again at run time.

**Three layers of assurance**
| Layer | Question | Enforced by |
|---|---|---|
| Static: checker and report | What can this program touch? | The kio checker, before running |
| Enforced: host and limits | Can it touch anything else? | A Wasm sandbox (Wasmtime) and a host that confines every path and request |
| Recorded: audit log | What did it touch, this run? | The host, one log line per call |

Each layer holds without the one above it: if the checker has a bug, the sandbox still holds; if the program is hostile, the log is still written, by the host.

**What I built during Hack** (all working, all tested):
- The language: parser, type checker, interpreter; structs, enums, `match`, `Option`/`Result`/`?`, lists, maps, modules, recursive types.
- The authority report: for each function, exactly which capabilities it can use, including through helpers.
- A Wasm sandbox host: `kio run` with directory grants confined by cap-std, an HTTP allowlist (redirects and DNS pinned), and limits on CPU (fuel), time, memory, stack, calls and I/O.
- `kio run --listen`: an HTTP server where each request gets a fresh sandbox, about 5,900 requests/s.
- An audit log, an MCP server so agents can check their own code, and kio-web, a small web framework written in kio.
- Three independent adversarial security reviews (168 attacks); every finding fixed or documented.

**Value to Yelp.** Any team letting agents write code that touches data (pipelines, automation, internal tools) can run it with exactly the access granted and a record of what happened, instead of trusting a review or provisioning a container per task. The authority report makes agent PRs reviewable from the signature.

**Repo:** https://github.com/danglangs/kiolang (web framework: https://github.com/danglangs/kio-web)

**Demo:**
```
rustup target add wasm32-wasip2
cargo install --git https://github.com/danglangs/kiolang kio
git clone https://github.com/danglangs/kiolang && cd kiolang
docs/hack/demo.sh
```

## 60-second teaser (Round 2)

**Slide title:** kio: AI writes the code. You control what it can touch.

**Slide body:**
```
fn summarize(data: Bytes, log: Http) -> Bytes { ... }

$ kio check leaky.kio
summarize : log get        ← this summary makes network calls, visible from its signature

Static  checker + report   what can it touch?
Enforced  Wasm sandbox     can it touch anything else?   (no)
Recorded  audit log        what did it touch?
```

**Script (about 60 s):**
1. (10 s) "Agents write code faster than we can review it. In most languages, any line can read a file or call the internet, so to know what agent code does you read every line."
2. (20 s) "kio is a language where the code names what it needs, like `input: Dir`, and whoever runs it decides what that is, like `--dir input=/data`. Files, hosts and even the server socket are capabilities you grant at run time. `kio check` prints exactly what each function can reach. Here a summary function also makes a network request, and the report shows it."
3. (20 s) "Three layers: the checker rejects code that reaches for authority it wasn't given; a Wasm sandbox refuses it again at run time, so a checker bug isn't a breach; and the host logs every call."
4. (10 s) "It runs today, serves HTTP at about 6,000 requests a second with a fresh sandbox per request, and it's open source. Come see it break out of nothing at the booth."

## Booth demo (5 minutes)

Run `docs/hack/demo.sh`; press Enter between steps:
1. `kio check` on a file program: its whole authority in two lines.
2. Run it with two directory grants; it writes `42`.
3. `leaky.kio`: a summary function that makes a network request, shown by the report.
4. `sneaky.kio`: the helper tries `Http::get(..)`, the equivalent of Python's `urllib` or Rust's `std::net`, which need nothing in the signature. kio has no ambient network, so it is rejected: "no ambient Http access". Steps 3 and 4 together: a function's network use is always in its signature, so the report cannot be fooled.
5. `escape.kio`: `../../etc/passwd` type-checks but the host refuses it, and the audit log records the attempt.
6. `forever.kio`: an infinite loop stopped by fuel.

Optional: `kio run examples/serve/app.kio --dir public=examples/tour/in --listen 127.0.0.1:8080` and `curl http://127.0.0.1:8080/hello/kio`.

## Video (2-3 minutes): motivation and the three layers

Record the screen running `docs/hack/demo.sh`, with this narration. Keep a copy as the booth backup.

1. **(0:00-0:25) The problem.** "We are handing AI agents more and more code to write. In every mainstream language, any line can read a file, open a socket or call an API. Memory safety, like Rust's, doesn't help: a memory-safe program can still send your data anywhere. So reviewing agent code for safety means reading every line, and one injected line is enough."
2. **(0:25-0:50) The idea.** "kio makes authority part of the type. There is no ambient I/O. A file system, an HTTP host, even a listening socket is a capability: a parameter of `main`, granted by whoever runs the program. A function with no capability parameter can't do I/O at all." Show step 1: `main : input read, out create`.
3. **(0:50-1:20) Layer one, static.** "The checker computes what every function can reach and rejects any attempt beyond it." Show steps 3 and 4: the report exposing the phone-home helper, and the sneaky helper rejected with a hint.
4. **(1:20-1:50) Layer two, enforced.** "But checkers have bugs. So programs run in a WebAssembly sandbox whose only exits are a few host functions, and the host enforces every rule itself: paths stay inside the grant, hosts stay on the allowlist, and CPU, memory and time are bounded." Show steps 5 and 6.
5. **(1:50-2:10) Layer three, recorded.** "And the host logs every call it serves. A call the report didn't predict stops the run: the log checks the checker." Show the audit line from step 5.
6. **(2:10-2:30) Close.** "It's a real language: types, modules, a web server with a fresh sandbox per request at about 6,000 requests a second, and an MCP server so agents can check their own code. AI can write the code; the operator controls what it can touch."
