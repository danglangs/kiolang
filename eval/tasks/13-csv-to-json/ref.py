import csv
import json
import os

rows, skipped = [], 0
with open(os.path.join(os.environ["INPUT"], "users.csv"), newline="") as f:
    lines = [line for line in f.read().split("\n")[1:] if line.strip()]
for f in csv.reader(lines):
    try:
        assert len(f) == 5
        uid = int(f[0])
    except (AssertionError, ValueError):
        skipped += 1
        continue
    rows.append({"id": uid, "name": f[1], "email": f[2],
                 "active": f[3].lower() in ("yes", "true", "1"),
                 "tags": [t.strip() for t in f[4].split(";") if t.strip()]})
with open(os.path.join(os.environ["OUT"], "users.json"), "w") as out:
    json.dump(rows, out, indent=1)
print("rows %d skipped %d" % (len(rows), skipped))
