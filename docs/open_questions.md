# Open questions

Decisions taken without the owner, cheap to undo. Each: the question, what was chosen, how to undo.

| # | Question | Chosen | Undo |
|---|---|---|---|
| 1 | Runtime depth limit | One evaluation-depth budget of 3_000 nested expressions (~1_400 recursive calls), not a call count. Cut from 5_000 when `Value` grew to 64 bytes with enums; measured 6 MB debug, 3 MB release | Raise the constant, or a bytecode VM turns it into a plain call limit |
| 2 | Parser nesting limit | 64, so debug builds do not overflow | Raise the constant |
| 3 | How `None`, `Ok`, `Err` get their type parameters | Expected-type passing from annotations, parameters, fields, returns and branches; no unification | Add inference later; it only accepts more programs |
