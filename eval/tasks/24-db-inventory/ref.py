import os
import re
import sqlite3

db = sqlite3.connect(os.environ["STOCK"])
db.execute("CREATE TABLE IF NOT EXISTS stock (sku TEXT PRIMARY KEY, qty INTEGER NOT NULL)")
db.execute("CREATE TABLE IF NOT EXISTS batches (name TEXT PRIMARY KEY)")
db.commit()

batches = []
for line in open(os.path.join(os.environ["INPUT"], "moves.txt")):
    f = line.split()
    if not f:
        continue
    if f[0] == "batch" and len(f) == 2:
        batches.append((f[1], []))
    elif batches:
        batches[-1][1].append(f)

for name, moves in batches:
    if db.execute("SELECT 1 FROM batches WHERE name = ?", (name,)).fetchone():
        print(name, "skipped")
        continue
    qty, ok = {}, True
    for m in moves:
        if len(m) != 2 or not re.fullmatch(r"[+-][0-9]+", m[1]) or int(m[1][1:]) == 0:
            ok = False
            break
        if m[0] not in qty:
            row = db.execute("SELECT qty FROM stock WHERE sku = ?", (m[0],)).fetchone()
            qty[m[0]] = row[0] if row else 0
        qty[m[0]] += int(m[1])
        if qty[m[0]] < 0:
            ok = False
            break
    if not ok:
        print(name, "rejected")
        continue
    with db:
        for sku, q in qty.items():
            db.execute("INSERT INTO stock VALUES (?, ?) ON CONFLICT (sku) DO UPDATE SET qty = excluded.qty", (sku, q))
        db.execute("INSERT INTO batches VALUES (?)", (name,))
    print(name, "applied")
for sku, q in db.execute("SELECT sku, qty FROM stock WHERE qty != 0 ORDER BY sku"):
    print(sku, q)
