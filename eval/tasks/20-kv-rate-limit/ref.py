import json
import os

store = os.environ["LIMITS"]
counts = json.load(open(store)) if os.path.exists(store) else {}
for line in open(os.path.join(os.environ["INPUT"], "requests.txt")):
    f = line.split()
    if len(f) != 2:
        continue
    try:
        ts = int(f[0])
    except ValueError:
        continue
    key = "%s:%d" % (f[1], ts // 60)
    if counts.get(key, 0) < 3:
        counts[key] = counts.get(key, 0) + 1
        print(ts, f[1], "allow")
    else:
        print(ts, f[1], "deny")
json.dump(counts, open(store, "w"))
