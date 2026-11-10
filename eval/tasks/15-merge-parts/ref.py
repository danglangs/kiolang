import os
import re

src = os.environ["INPUT"]
parts = [n for n in os.listdir(src) if re.fullmatch(r"part-\d{4}\.txt", n)]
values, corrupt = set(), 0
for name in parts:
    try:
        xs = [int(x) for x in open(os.path.join(src, name)).read().split("\n") if x.strip()]
    except ValueError:
        corrupt += 1
        continue
    values.update(xs)
with open(os.path.join(os.environ["OUT"], "merged.txt"), "w") as f:
    f.write("".join("%d\n" % v for v in sorted(values)))
print("parts %d corrupt %d values %d" % (len(parts), corrupt, len(values)))
