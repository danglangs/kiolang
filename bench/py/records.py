# kio/records.kio: json.load, then accessors over the records.
import sys, json
doc = json.load(open(sys.argv[1]))
active = tags = 0; score = 0.0; zones = {}
for r in doc:
    if r.get("active") is True: active += 1
    s = r.get("score"); score += float(s) if isinstance(s, (int, float)) and not isinstance(s, bool) else 0.0
    t = r.get("tags"); tags += len(t) if isinstance(t, list) else 0
    m = r.get("meta")
    if isinstance(m, dict):
        z = m.get("zone"); z = z if isinstance(z, str) else ""
        rank = m.get("rank"); rank = rank if isinstance(rank, int) and not isinstance(rank, bool) else 0
        if rank > 100: zones[z] = zones.get(z, 0) + 1
print(active * 1000000000 + tags * 100000 + len(zones) + int(score) % 1000)
