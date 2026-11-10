import os
import sqlite3

db = sqlite3.connect(os.environ["SHOP"])
db.execute("CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, customer TEXT, product TEXT, qty INTEGER, unit_cents INTEGER)")
imported = skipped = rejected = 0
for line in open(os.path.join(os.environ["INPUT"], "orders.csv")).read().split("\n")[1:]:
    if not line.strip():
        continue
    f = [x.strip() for x in line.split(",")]
    try:
        assert len(f) == 5
        oid, qty, cents = int(f[0]), int(f[3]), int(f[4])
    except (AssertionError, ValueError):
        rejected += 1
        continue
    cur = db.execute("INSERT OR IGNORE INTO orders VALUES (?, ?, ?, ?, ?)", (oid, f[1], f[2], qty, cents))
    if cur.rowcount:
        imported += 1
    else:
        skipped += 1
db.commit()
print("imported %d skipped %d rejected %d" % (imported, skipped, rejected))
q = "SELECT product, SUM(qty * unit_cents) AS r, COUNT(*) FROM orders GROUP BY product ORDER BY r DESC, product LIMIT 3"
for row in db.execute(q):
    print(*row)
