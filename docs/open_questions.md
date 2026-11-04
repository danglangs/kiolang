# Open questions

Decisions taken without the owner, cheap to undo. Each: the question, what was chosen, how to undo.

| # | Question | Chosen | Undo |
|---|---|---|---|
| 1 | Runtime depth limit | One evaluation-depth budget of 3_000 nested expressions (~1_400 recursive calls), not a call count. Cut from 5_000 when `Value` grew to 64 bytes with enums; measured 6 MB debug, 3 MB release | Raise the constant, or a bytecode VM turns it into a plain call limit |
| 2 | Parser nesting limit | 64, so debug builds do not overflow | Raise the constant |
| 3 | How `None`, `Ok`, `Err` get their type parameters | Expected-type passing from annotations, parameters, fields, returns and branches; no unification | Add inference later; it only accepts more programs |
| 4 | Which capability uses are allowed | Only a parameter name as a method receiver or whole call argument; everything else, including `if` branches yielding a capability, is rejected | Relax later; it only accepts more programs |
| 5 | `main` return type (rule 3) | Any type for now; the runner prints it. Spec says `()` or `Result<(), E>` | One check in `signature` once `host` defines exit status |
| 6 | Binding a name `None`, `Some`, `Ok` or `Err` | Rejected for parameters and locals, so those names always mean the constructors | Allow shadowing later |
| 7 | How a helper's capability maps to a grant | `Value::Cap(name of the main parameter)`, passed as an ordinary value | Replace with a handle index when `guest` maps names to WIT handles |
| 8 | Host result checking in `lang` | Full: every capability call result is checked against the method's declared type (`Program::fits`), else `RunErr`. Shallow checking let a lying host panic `run`, found by adversarial review | Keep; WIT bindings in `guest` add a second check |
| 9 | Capability table source | `CapTable::v0()` in `check.rs`, written by hand to mirror caps.wit | Replace with the table generated from caps.wit |
| 10 | Temporary runner and real directories | Runner grants none; real `--dir` waits for the host with cap-std | Add a cap-std-backed `Caps` to the runner |
| 11 | Interpreter stack as features grow | `#[inline(never)]` on large eval helpers keeps release at 3 MB; debug needs 8 MB, so tests run on 16 MB for headroom. The guest is a release build | Split `expr_kind` arms into helpers to shrink debug frames, or the bytecode VM |
