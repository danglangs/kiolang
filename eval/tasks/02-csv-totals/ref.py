import os

lines = open(os.path.join(os.environ["INPUT"], "sales.csv")).read().split("\n")[1:]
qty, rev, skipped = {}, {}, 0
for line in lines:
    if not line.strip():
        continue
    f = [x.strip() for x in line.split(",")]
    try:
        assert len(f) == 5
        q, c = int(f[3]), int(f[4])
    except (AssertionError, ValueError):
        skipped += 1
        continue
    qty[f[1]] = qty.get(f[1], 0) + q
    rev[f[1]] = rev.get(f[1], 0) + q * c
for r in sorted(rev, key=lambda r: (-rev[r], r)):
    sign = "-" if rev[r] < 0 else ""
    print("%s %d %s%d.%02d" % (r, qty[r], sign, abs(rev[r]) // 100, abs(rev[r]) % 100))
print("skipped", skipped)
