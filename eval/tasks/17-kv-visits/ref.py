import json
import os

store = os.environ["COUNTS"]
counts = json.load(open(store)) if os.path.exists(store) else {}
for line in open(os.path.join(os.environ["INPUT"], "visits.txt")):
    page = line.strip()
    if page:
        counts[page] = counts.get(page, 0) + 1
json.dump(counts, open(store, "w"))
for page in sorted(counts, key=lambda p: (-counts[p], p.encode())):
    print(page, counts[page])
