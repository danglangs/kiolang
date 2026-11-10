import calendar
import datetime
import os
import re

lines = open(os.path.join(os.environ["INPUT"], "events.log"), encoding="utf-8").read().split("\n")
key = lambda s: s.encode()


def parse_time(t):
    if not re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", t):
        return None
    try:
        d = datetime.datetime.strptime(t, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError:
        return None
    return calendar.timegm(d.timetuple())


events, seen, malformed, dups = {}, set(), 0, 0
for line in lines:
    if line.strip() == "":
        continue
    f = line.split(" ")
    t = parse_time(f[0]) if len(f) == 3 else None
    if t is None:
        malformed += 1
        continue
    if line in seen:
        dups += 1
        continue
    seen.add(line)
    events.setdefault(f[1], []).append((t, f[2]))

out, durations, pages = [], [], {}
for user in sorted(events, key=key):
    evs = sorted(events[user])
    sessions = [[evs[0][0], evs[0][0]]]
    for t, _ in evs[1:]:
        if t - sessions[-1][1] > 1800:
            sessions.append([t, t])
        else:
            sessions[-1][1] = t
    ds = [e - s for s, e in sessions]
    durations += ds
    for _, p in evs:
        pages[p] = pages.get(p, 0) + 1
    m = max(ds)
    out.append("%s sessions %d events %d longest %02d:%02d:%02d pages %d" % (
        user, len(sessions), len(evs), m // 3600, m // 60 % 60, m % 60, len({p for _, p in evs})))
if durations:
    durations.sort()
    out.append("median %d" % durations[(len(durations) - 1) // 2])
    top = sorted(pages, key=lambda p: (-pages[p], key(p)))[:3]
    out.append("top: " + ", ".join("%s (%d)" % (p, pages[p]) for p in top))
out.append("malformed %d duplicates %d" % (malformed, dups))
print("\n".join(out))
