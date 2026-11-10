import json
import os

INPUT, QUEUE, NOW = os.environ["INPUT"], os.environ["QUEUE"], int(os.environ["CLOCK"])
jobs = json.load(open(QUEUE)) if os.path.exists(QUEUE) else {}
script = json.load(open(os.path.join(INPUT, "script.json")))
out = []

lines = open(os.path.join(INPUT, "enqueue.jsonl"), encoding="utf-8").read().split("\n")
for n, line in enumerate(lines, start=1):
    if line.strip() == "":
        continue
    try:
        j = json.loads(line)
    except ValueError:
        j = None
    if (not isinstance(j, dict) or not isinstance(j.get("id"), str) or not isinstance(j.get("priority"), int)
            or isinstance(j.get("priority"), bool)):
        out.append("invalid line %d" % n)
        continue
    if j["id"] in jobs:
        out.append("duplicate " + j["id"])
        continue
    jobs[j["id"]] = {"priority": j["priority"], "due": NOW, "attempts": 0, "state": "pending"}

due = [i for i, j in jobs.items() if j["state"] == "pending" and j["due"] <= NOW]
due.sort(key=lambda i: (-jobs[i]["priority"], jobs[i]["due"], i.encode()))
for i in due[:5]:
    j = jobs[i]
    j["attempts"] += 1
    outcomes = script.get(i, ["ok"])
    if outcomes[min(j["attempts"], len(outcomes)) - 1] == "ok":
        j["state"] = "done"
        out.append("done " + i)
    elif j["attempts"] == 4:
        j["state"] = "dead"
        out.append("dead " + i)
    else:
        wait = 60 * 2 ** (j["attempts"] - 1)
        j["due"] = NOW + wait * 1000
        out.append("retry %s in %d" % (i, wait))

json.dump(jobs, open(QUEUE, "w"))
count = lambda s: sum(1 for j in jobs.values() if j["state"] == s)
out.append("pending %d done %d dead %d" % (count("pending"), count("done"), count("dead")))
print("\n".join(out))
