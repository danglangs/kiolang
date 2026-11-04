# Open questions

Decisions taken without the owner, cheap to undo. Each: the question, what was chosen, how to undo.

| # | Question | Chosen | Undo |
|---|---|---|---|
| 1 | Runtime depth limit | One evaluation-depth budget of 3_000 nested expressions (~1_400 recursive calls), not a call count. Cut from 5_000 when `Value` grew to 64 bytes with enums; measured 6 MB debug, 3 MB release | Raise the constant, or a bytecode VM turns it into a plain call limit |
| 2 | Parser nesting limit | 64, so debug builds do not overflow | Raise the constant |
| 3 | How `None`, `Ok`, `Err` get their type parameters | Expected-type passing from annotations, parameters, fields, returns and branches; no unification | Add inference later; it only accepts more programs |
| 4 | Which capability uses are allowed | Only a parameter name as a method receiver or whole call argument; everything else, including `if` branches yielding a capability, is rejected | Relax later; it only accepts more programs |
| 5 | `main` return type (rule 3) | Any type; `kio run` prints it. A `Result::Err` from `main` exits 1 (`outcome.failed` in caps.wit) | Restrict `main` to `()` or `Result<(), E>` with one check in `signature` |
| 6 | Binding a name `None`, `Some`, `Ok` or `Err` | Rejected for parameters and locals, so those names always mean the constructors | Allow shadowing later |
| 7 | How a helper's capability maps to a grant | `Value::Cap(name of the main parameter)`, passed as an ordinary value | Replace with a handle index when `guest` maps names to WIT handles |
| 8 | Host result checking in `lang` | Full: every capability call result is checked against the method's declared type (`Program::fits`), else `RunErr`. Shallow checking let a lying host panic `run`, found by adversarial review | Keep; WIT bindings in `guest` add a second check |
| 9 | Capability table source | Generated from caps.wit by `tools/capgen` into `lang/src/caps_gen.rs`; CI fails on drift | Resolved |
| 10 | Temporary runner and real directories | Runner grants none; real `--dir` waits for the host with cap-std | Add a cap-std-backed `Caps` to the runner |
| 11 | Interpreter stack as features grow | `#[inline(never)]` on large eval helpers keeps native release at 3 MB; debug tests run on 16 MB. Measured in the guest (G4): the depth budget needs about 900 KB of Wasm stack and 1.3 MB of linear memory, so `--stack` defaults to 2 MB (a debug host needs over 1 MB) and the bytecode VM is not needed for stack reasons | Re-measure when features grow; the VM's case is speed (web W10) |
| 12 | `main`'s value across the WIT boundary | Printed text in `run`'s result; the host only prints it | Add a typed export later; existing hosts keep working |
| 13 | Grant handles in `run` | `own<dir>` and `own<http>` in the grant records, because wit-bindgen 0.63 generates non-compiling code for `borrow` inside a list of records. Safety is unchanged: only the host mints handles and every call is checked | Switch back to `borrow` once wit-bindgen is fixed |
| 14 | How the host gets the guest bytes | `host/build.rs` builds the guest into `target/guest` and embeds it, so one `cargo build` gives a self-contained `kio` | Build the guest as a separate CI step and load the file at runtime |
| 15 | WASI imported by Rust std in the guest | Linked with nothing granted: no preopens, env, args, network or stdio; frozen clocks and a fixed random seed so runs are deterministic. kio code cannot reach WASI | Build the guest without std WASI imports (no_std or a custom target) to remove them entirely |
| 16 | A grant that matches no `main` parameter | Error before any code runs, like a missing grant, so an operator typo (`--dir inptu=..`) is caught | Allow and ignore unused grants |
| 17 | `Dir.create` on an existing file | Fails with `denied`: no overwrites | Add an explicit `write` method later, or a grant flag allowing overwrite |
| 18 | `Dir.create` when the write fails partway | The host removes the file it just created, so a retry works. A crash mid-write can still leave a partial file | Write a temp file, then `hard_link` it into place (refuses to overwrite) and remove the temp |
| 19 | What the wall clock bounds | Guest execution only: a blocked host call runs to completion and the deadline fires on return. For `Dir`, the 10 MB per-read cap bounds a call; `Http` gets client timeouts in N1 | Run `Dir` calls on a worker thread with a timeout |
