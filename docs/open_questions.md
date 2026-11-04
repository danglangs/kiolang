# Open questions

Decisions taken without the owner, cheap to undo. Each: the question, what was chosen, how to undo.

| # | Question | Chosen | Undo |
|---|---|---|---|
| 1 | Runtime depth limit | One evaluation-depth budget of 5_000 nested expressions (~2_000 recursive calls), not a call count | Raise the constant, or a bytecode VM turns it into a plain call limit |
| 2 | Parser nesting limit | 64, so debug builds do not overflow | Raise the constant |
