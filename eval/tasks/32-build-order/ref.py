import json
import os
import sys

raw = json.load(open(os.path.join(os.environ["INPUT"], "packages.json"), encoding="utf-8"))
if not isinstance(raw, dict):
    print("error: not an object")
    sys.exit(0)
invalid = sorted(k for k, v in raw.items()
                 if not isinstance(v, list) or not all(isinstance(d, str) for d in v))
deps = {k: sorted(set(v)) for k, v in raw.items() if k not in invalid}
key = lambda s: s.encode()

stage = {}
changed = True
while changed:
    changed = False
    for p, ds in deps.items():
        if p not in stage and all(d in stage for d in ds):
            stage[p] = 1 + max((stage[d] for d in ds), default=0)
            changed = True


def reach(u):
    seen, todo = set(), list(deps[u])
    while todo:
        v = todo.pop()
        if v in deps and v not in seen:
            seen.add(v)
            todo += deps[v]
    return seen


r = {p: reach(p) for p in deps}
cycles = set()
for p in deps:
    if p in r[p]:
        cycles.add(tuple(sorted((q for q in deps if q in r[p] and p in r[q]), key=key)))
in_cycle = {p for c in cycles for p in c}

out = []
for s in range(1, max(stage.values(), default=0) + 1):
    out.append("stage %d: %s" % (s, ", ".join(sorted((p for p in stage if stage[p] == s), key=key))))
for c in sorted(cycles, key=lambda c: key(c[0])):
    out.append("cycle: " + ", ".join(c))
missing = {}
for p, ds in deps.items():
    for d in ds:
        if d not in deps:
            missing.setdefault(d, []).append(p)
for m in sorted(missing, key=key):
    out.append("missing: %s (needed by %s)" % (m, ", ".join(sorted(missing[m], key=key))))
if invalid:
    out.append("invalid: " + ", ".join(sorted(invalid, key=key)))
blocked = sorted((p for p in deps if p not in stage and p not in in_cycle), key=key)
if blocked:
    out.append("blocked: " + ", ".join(blocked))
out.append("built %d of %d" % (len(stage), len(deps)))
print("\n".join(out))
