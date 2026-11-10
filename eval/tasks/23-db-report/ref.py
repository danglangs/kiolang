import os
import sqlite3

db = sqlite3.connect("file:%s?mode=ro" % os.environ["SHOP"], uri=True)
PAID = "o.status = 'paid' AND o.placed >= '2026-01-01' AND o.placed < '2027-01-01'"
rows = db.execute(f"""
  SELECT c.country, COUNT(DISTINCT c.id), COUNT(*), SUM(o.total_cents)
  FROM orders o JOIN customers c ON c.id = o.customer_id
  WHERE {PAID} GROUP BY c.country ORDER BY 4 DESC, 1""").fetchall()
for country, customers, orders, revenue in rows:
    (top,) = db.execute(f"""
      SELECT c.name FROM orders o JOIN customers c ON c.id = o.customer_id
      WHERE {PAID} AND c.country = ? GROUP BY c.id ORDER BY SUM(o.total_cents) DESC, c.id LIMIT 1""",
                        (country,)).fetchone()
    print(f"{country} customers={customers} orders={orders} revenue={revenue} top={top}")
print("total orders=%d revenue=%d" % (sum(r[2] for r in rows), sum(r[3] for r in rows)))
