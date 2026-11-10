import os
import re

lines = open(os.path.join(os.environ["INPUT"], "bookings.csv"), encoding="utf-8").read().split("\n")


def minutes(t):
    if not re.fullmatch(r"\d\d:\d\d", t):
        return None
    h, m = int(t[:2]), int(t[3:])
    if t == "24:00":
        return 1440
    return h * 60 + m if h < 24 and m < 60 else None


def fmt(m):
    return "%02d:%02d" % (m // 60, m % 60)


groups, skipped, seen = {}, [], set()
for n, line in enumerate(lines[1:], start=2):
    if line.strip() == "":
        continue
    f = line.split(",")
    if len(f) != 5:
        skipped.append(n)
        continue
    s, e = minutes(f[2]), minutes(f[3])
    if s is None or e is None or e <= s or s == 1440:
        skipped.append(n)
        continue
    if line in seen:
        continue
    seen.add(line)
    groups.setdefault((f[0].encode(), f[1].encode()), []).append((s, e, f[4]))

out = []
for room, day in sorted(groups):
    bs = groups[(room, day)]
    out.append("%s %s" % (room.decode(), day.decode()))
    merged = []
    for s, e, _ in sorted(bs):
        if merged and s <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], e)
        else:
            merged.append([s, e])
    out.append("  busy " + ", ".join("%s-%s" % (fmt(s), fmt(e)) for s, e in merged))
    free, at = [], 540
    for s, e in merged + [[1080, 1080]]:
        if min(s, 1080) - at >= 30:
            free.append("%s-%s" % (fmt(at), fmt(min(s, 1080))))
        at = max(at, e)
    out.append("  free " + (", ".join(free) if free else "none"))
    conf = []
    for i in range(len(bs)):
        for j in range(i + 1, len(bs)):
            s, e = max(bs[i][0], bs[j][0]), min(bs[i][1], bs[j][1])
            if s < e:
                who = "/".join(sorted([bs[i][2], bs[j][2]], key=lambda w: w.encode()))
                conf.append((s, ("  conflict %s %s-%s" % (who, fmt(s), fmt(e))).encode()))
    out += [t.decode() for _, t in sorted(conf)]
out.append("skipped: " + (", ".join(map(str, skipped)) if skipped else "none"))
print("\n".join(out))
