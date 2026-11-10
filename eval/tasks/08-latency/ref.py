import os

groups = {}
for line in open(os.path.join(os.environ["INPUT"], "timings.log")):
    f = line.split()
    if len(f) != 3:
        continue
    try:
        ms = int(f[2])
    except ValueError:
        continue
    groups.setdefault((f[0], f[1]), []).append(ms)
for (m, p), ts in sorted(groups.items()):
    ts.sort()
    n = len(ts)

    def rank(pct):
        return ts[(pct * n + 99) // 100 - 1]
    print("%s %s count=%d p50=%d p95=%d max=%d" % (m, p, n, rank(50), rank(95), ts[-1]))
