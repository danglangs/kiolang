import json
import os

orders = json.load(open(os.path.join(os.environ["INPUT"], "orders.json")))
paid, refunded, pending = {}, [0, 0], 0
for o in orders:
    total = sum(i["qty"] * i["price_cents"] for i in o["items"])
    if o["status"] == "paid":
        n, t = paid.get(o["customer"], (0, 0))
        paid[o["customer"]] = (n + 1, t + total)
    elif o["status"] == "refunded":
        refunded = [refunded[0] + 1, refunded[1] + total]
    elif o["status"] == "pending":
        pending += 1
for c in sorted(paid):
    print(c, *paid[c])
print("refunded", *refunded)
print("pending", pending)
