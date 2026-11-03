# Design

## Components

```
 Agent writes                 Toolchain                         Runtime
┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────────────────┐
│ .kio source: │──▶│ type checker │──▶│ interpreter  │──▶│ Wasmtime sandbox         │
│ records, fns,│   │ accept/reject│   │ (later: a    │   │                          │
│ cap params   │   │ + authority  │   │  compiler)   │   │ guest can only call ──┐  │
└──────────────┘   │   report     │   └──────────────┘   │ imported host fns     │  │
       ▲           └──────┬───────┘                      └───────────────────────┼──┘
       └──────────────────┘ errors back                                          ▼
                            to agent                     ┌──────────────────────────┐
        ┌──────────────────────────────┐                 │ HOST (Rust)              │
        │ caps.wit                     │                 │ trusted, hand-written:   │
        │ generates the cap table and  │────implements──▶│ dir.read / dir.create    │
        │ the guest and host bindings  │                 │ http.get (allowlisted)   │
        └──────────────────────────────┘                 │ validates every handle   │
                                                         └──────────────────────────┘
```

| Component | Trusted | Role |
|---|---|---|
| `.kio` source | No | Business logic. Capabilities are opaque values that can be passed and called, never created. |
| Checker | Yes, but runs in the sandbox | Accepts or rejects the program against the capability table. Emits the authority report. Generates no code. |
| Interpreter | Yes, but runs in the sandbox | Evaluates the checked program. Rust, compiled to `wasm32`. |
| `caps.wit` | Yes | Declares every function that crosses the sandbox wall. Changing it is a security-review event. |
| Wasmtime | Yes | In-process sandbox. No ambient file, network, env or clock. Fuel, memory and epoch limits. |
| Host functions | Yes | The only code that touches real resources. Each call validates the handle, its scope and quotas. |

Everything runs in one OS process, the `kio` binary. Wasmtime is a library inside it and the guest is a byte blob embedded at build time. The sandbox is a memory wall inside the process. For production, add OS-level isolation as well: a container or VM, plus seccomp.

## Three crates

- `lang`: parser, checker, interpreter. No I/O, no Wasm dependency. All language tests run natively.
- `guest`: `lang` plus a thin adapter, built once to `wasm32`. Exports `check(source)` and `run(source, caps)`. Forwards capability calls to the `caps.wit` imports.
- `host`: the runtime binary. Embeds Wasmtime and the guest bytes, parses grants, mints handles, sets limits, implements the host functions.

## Files and who owns them

| File | Written by | Read by | When |
|---|---|---|---|
| `caps.wit` | you, by hand, rarely | build step | at build |
| capability table (Rust) | generated from `caps.wit` by a `guest` build script | checker, inside the guest | at build |
| guest and host Rust bindings | generated from `caps.wit` by wit-bindgen | rustc | at build |
| `prog.kio` | the agent | checker, then interpreter | every run |
| grants (`--dir name=path`) | the operator | host | every run |

Adding a method to `caps.wit` regenerates the capability table, the guest import stub and the host trait. The host no longer compiles until the method is implemented. The three cannot drift.

`caps.wit` is one file for the whole toolchain. It says what a `Dir` *can* do. A grant says which directory *this run* gets. The program says which roles it *needs*. Three layers, three owners.

## One run, step by step

```
kio run prog.kio --dir input=/srv/jobs/42/in --dir out=/srv/jobs/42/out
```

| Step | What happens | Where |
|---|---|---|
| 1 | Host opens each granted directory with cap-std and builds the handle table `{0: input, 1: out}` | host, native |
| 2 | Host instantiates the guest with fuel, memory and epoch limits, and binds the WIT imports to its host functions | Wasmtime |
| 3 | Host calls `run(source, [0, 1])` | wall crossed, into the guest |
| 4 | Guest parses and checks against the embedded capability table. On reject it returns the errors. No I/O has happened. | guest |
| 5 | Guest checks that `main`'s cap parameters match the grants by name and kind | guest |
| 6 | Interpreter starts `main` with `Cap(Dir, 0)` and `Cap(Dir, 1)` | guest |
| 7 | On `input.read("invoices.csv")` the interpreter calls the import `dir.read(0, "invoices.csv")` | wall crossed, out of the guest |
| 8 | Host looks up handle 0, checks its kind, charges the call quota, resolves the path beneath the root, enforces the size limit, reads, returns bytes | host, native |
| 9 | Interpreter continues. Steps 7 and 8 repeat per capability call. | guest |
| 10 | `main` returns. Host prints the exit status and the authority report, and drops the handle table. | host |

`kio check prog.kio` runs steps 3 to 5 only, with an empty handle table. The MCP tool calls the same export.

## The host

**Handle table.** A per-run vector of opened resources. The guest sees indices. An unknown index or a wrong kind is a denied call. Because the table holds only what was granted, guessing a different index gains nothing.

**Dir.** Opened once with cap-std at grant time. Every path from the guest is resolved relative to that open directory, which refuses absolute paths, `..` above the root and symlinks that lead out. On Linux this is `openat2` with `RESOLVE_BENEATH`.

**Http.** A grant names host and methods. The host checks the allowlist on the first request and on every redirect hop, refuses IP-literal hosts unless explicitly granted, and pins DNS resolution for the run. Response bodies are capped.

**Limits.** Wasmtime fuel bounds instructions. Epoch interruption bounds wall-clock, because fuel does not tick while a host call waits on a slow server. Each host call also has its own timeout and the run has a host-call count and total I/O byte budget.

## Why the checker runs in the guest

The checker is part of `lang`, and `lang` is what gets compiled to `wasm32`. Running it natively would mean building `lang` twice and parsing adversarial input outside the sandbox. Inside the guest, a parser that overflows or spins costs fuel, not the host.

The consequence: the host never relies on the checker's verdict for safety. The handle table and host functions enforce every guarantee on their own. The checker exists for early, readable rejection and for the authority report.

## Memory model

Value semantics without a garbage collector or lifetimes. The v0 interpreter clones eagerly and is the reference: later optimizations must not change what a program observes, and the tests that pin copy behavior are what make them safe.

| Step | Change | Observable behavior |
|---|---|---|
| v0 | every assignment, argument and container read clones | reference |
| copy on write | values share storage with a reference count; any write to shared storage, including `x.f = v` and `xs[i] = v`, copies it first | unchanged |
| last-use moves | a variable's last use moves its storage instead of sharing it | unchanged |
| `&mut` parameters | a callee changes the caller's value; shared storage is still copied first | new feature |

Each step lands in the interpreter before any compiler. No stored references and no shared mutable objects means no cycles, so reference counting never leaks.

`&mut` rules:
- **No escape.** Parameters only: never stored, returned or put in a container. It cannot outlive the call.
- **No overlap.** First version: an `&mut` argument is a whole local or a forwarded `&mut` parameter, and all arguments in one call have distinct roots. Field and index arguments (`&mut xs[i]`) wait until overlap checking is defined.

Linked structures use indexes into a `List`. Copies stay implicit, as in Swift's value types. Hylo's explicit-copy default is not adopted, because switching later would change existing programs.

`Program` holds resolved names and types and nothing from the interpreter, so a compiler can consume it.

## Open questions

- **Attenuation.** `dir.sub("invoices")` would let `main` hand a helper less than it has. It conflicts with "caps are parameters only" because it returns a cap. Options: allow it only in argument position, or make the host mint sub-handles. Deferred to v1.
- **Relaxing second-class caps.** Storing caps in records would need flow analysis in the checker. Not until there is a program that needs it.
- **Compiler.** The interpreter is the fastest path to a working v0. A compiler to Wasm removes the interpreter from the trusted set and runs near native: the interpreter costs roughly 30 to 100 times. Language and interfaces stay the same.
- **Grant format.** CLI flags for v0. A manifest file once there are more than a handful of grants per run.
