import json
import os
import urllib.error
import urllib.request

INPUT, STATE, API = os.environ["INPUT"], os.environ["STATE"], os.environ["API"]
synced = json.load(open(STATE)) if os.path.exists(STATE) else {}

out, records = [], {}
lines = open(os.path.join(INPUT, "records.jsonl"), encoding="utf-8").read().split("\n")
for n, line in enumerate(lines, start=1):
    if line.strip() == "":
        continue
    try:
        r = json.loads(line)
    except ValueError:
        r = None
    if (not isinstance(r, dict) or not isinstance(r.get("id"), str)
            or not isinstance(r.get("version"), int) or isinstance(r.get("version"), bool)):
        out.append("line %d invalid" % n)
        continue
    records[r["id"]] = r


def put(r):
    req = urllib.request.Request("%s/records/%s" % (API, r["id"]), data=json.dumps(r).encode(), method="PUT",
                                 headers={"content-type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status
    except urllib.error.HTTPError as e:
        return e.code
    except OSError:
        return None


result = {}
for i, r in records.items():
    if r["version"] <= synced.get(i, -1):
        result[i] = "unchanged"
        continue
    for attempt in range(3):
        status = put(r)
        if status is not None and 200 <= status < 300:
            result[i] = "synced"
            synced[i] = r["version"]
            break
        if status is not None and 400 <= status < 500:
            result[i] = "rejected %d" % status
            break
    else:
        result[i] = "failed"

json.dump(synced, open(STATE, "w"))
for i in sorted(result, key=lambda s: s.encode()):
    out.append("%s %s" % (i, result[i]))
count = lambda w: sum(1 for v in result.values() if v.startswith(w))
out.append("synced %d unchanged %d rejected %d failed %d" % (
    count("synced"), count("unchanged"), count("rejected"), count("failed")))
print("\n".join(out))
