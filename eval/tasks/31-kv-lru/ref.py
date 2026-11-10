import json
import os

INPUT, CACHE = os.environ["INPUT"], os.environ["CACHE"]
state = {"order": [], "vals": {}, "hits": 0, "misses": 0, "evictions": 0}
if os.path.exists(CACHE):
    state = json.load(open(CACHE))
order, vals = state["order"], state["vals"]  # order: least recently used first
out = []


def evict():
    k = order.pop(0)
    del vals[k]
    state["evictions"] += 1
    out.append("evict " + k)


def touch(k):
    if k in order:
        order.remove(k)
    order.append(k)


lines = open(os.path.join(INPUT, "ops.txt"), encoding="utf-8").read().split("\n")
cap = int(lines[0].split(" ")[1])
while len(order) > cap:
    evict()
for n, line in enumerate(lines[1:], start=2):
    if line.strip() == "":
        continue
    w = line.split(" ")
    if w[0] == "get" and len(w) == 2 and w[1]:
        k = w[1]
        if k in vals:
            state["hits"] += 1
            touch(k)
            out.append("%s=%s" % (k, vals[k]))
        else:
            state["misses"] += 1
            out.append(k + " miss")
    elif w[0] == "del" and len(w) == 2 and w[1]:
        if w[1] in vals:
            del vals[w[1]]
            order.remove(w[1])
    elif w[0] == "put" and len(w) >= 3 and w[1] and line[len("put ") + len(w[1]) + 1:]:
        k, v = w[1], line[len("put ") + len(w[1]) + 1:]
        if k not in vals and len(order) >= cap:
            evict()
        vals[k] = v
        touch(k)
    else:
        out.append("bad line %d" % n)
out.append("cache: " + ",".join(reversed(order)))
out.append("stats hits %d misses %d evictions %d" % (state["hits"], state["misses"], state["evictions"]))
json.dump(state, open(CACHE, "w"))
print("\n".join(out))
