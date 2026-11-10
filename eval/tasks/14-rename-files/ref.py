import os
import re

src, dst = os.environ["INPUT"], os.environ["OUT"]
taken = set()
for name in sorted(os.listdir(src), key=lambda n: n.encode()):
    new = re.sub(r"[^a-z0-9.]+", "-", name.lower()).strip("-")
    if new in taken:
        print(name, "skipped")
        continue
    taken.add(new)
    with open(os.path.join(src, name), "rb") as f, open(os.path.join(dst, new), "wb") as g:
        g.write(f.read())
    print(name, "->", new)
