# Inventory moves, all or nothing

`main` receives `input: Dir`, granted for reading, and `stock: Db`, a SQLite database that keeps its contents between runs (empty on the first run; create your schema if it is missing).

`input/moves.txt` holds batches of stock moves:

```
batch receive-17
widget +40
gadget +5
batch ship-9
widget -12
```

A `batch NAME` line starts a batch; each following `SKU +N` or `SKU -N` line (N a positive integer) adds to or removes from that SKU's quantity. Apply each batch atomically: if any move in it would take a SKU below zero (an unknown SKU has 0), or a move line is malformed, the whole batch changes nothing. A batch name already applied in an earlier run or earlier in the file is skipped. Blank lines are ignored.

For each batch, in order, print `NAME applied`, `NAME rejected` or `NAME skipped`. Then print every SKU with a non-zero quantity as `SKU QTY`, sorted by SKU (byte order).

Grants: `input: Dir`, `stock: Db`
