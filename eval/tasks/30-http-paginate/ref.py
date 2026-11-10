import json
import os
import sys
import urllib.request

API = os.environ["API"]
orders, page = [], 1
while page is not None:
    with urllib.request.urlopen("%s/orders?page=%d" % (API, page), timeout=10) as r:
        doc = json.load(r)
    orders += doc["orders"]
    page = doc["next"]
paid = [o for o in orders if o["status"] == "paid"]
cents = {}
for o in paid:
    cents[o["currency"]] = cents.get(o["currency"], 0) + o["cents"]
summary = {"orders": len(orders), "paid_cents": cents,
           "refunded": sum(o["status"] == "refunded" for o in orders),
           "largest": min(paid, key=lambda o: (-o["cents"], o["id"]))["id"] if paid else None}
req = urllib.request.Request(API + "/summaries", data=json.dumps(summary).encode(), method="POST",
                             headers={"content-type": "application/json"})
with urllib.request.urlopen(req, timeout=10) as r:
    if r.status != 201:
        sys.exit("status %d" % r.status)
    print(json.load(r)["id"])
