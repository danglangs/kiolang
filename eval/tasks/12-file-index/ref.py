import os

root = os.environ["INPUT"]
rows = []
for d, _, files in os.walk(root):
    for name in files:
        p = os.path.join(d, name)
        data = open(p, "rb").read()
        lines = data.count(b"\n") + (1 if data and not data.endswith(b"\n") else 0)
        rows.append((os.path.relpath(p, root), len(data), lines))
rows.sort(key=lambda r: r[0].encode())
with open(os.path.join(os.environ["OUT"], "index.csv"), "w") as f:
    f.write("path,bytes,lines\n")
    for r in rows:
        f.write("%s,%d,%d\n" % r)
print("files %d bytes %d" % (len(rows), sum(r[1] for r in rows)))
