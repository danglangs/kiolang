import json
import os

store = os.environ["STATE"]
state = json.load(open(store)) if os.path.exists(store) else {"seen": [], "balances": {}}
seen = set(state["seen"])
applied = dup = rejected = 0
for line in open(os.path.join(os.environ["INPUT"], "events.jsonl")):
    if not line.strip():
        continue
    try:
        e = json.loads(line)
        ok = (isinstance(e, dict) and isinstance(e.get("id"), str) and isinstance(e.get("account"), str)
              and isinstance(e.get("cents"), int) and not isinstance(e.get("cents"), bool))
    except ValueError:
        ok = False
    if not ok:
        rejected += 1
    elif e["id"] in seen:
        dup += 1
    else:
        seen.add(e["id"])
        state["balances"][e["account"]] = state["balances"].get(e["account"], 0) + e["cents"]
        applied += 1
state["seen"] = sorted(seen)
json.dump(state, open(store, "w"))
print("applied %d duplicate %d rejected %d" % (applied, dup, rejected))
for acct in sorted(state["balances"], key=str.encode):
    print(acct, state["balances"][acct])
