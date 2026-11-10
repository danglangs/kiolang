import os
import re

pat = re.compile(r'^(\S+) \S+ \S+ \[[^\]]*\] "(\S+) (\S+) [^"]*" (\d{3}) (\d+|-)$')
status, paths, errors, total = {}, {}, {}, 0
for line in open(os.path.join(os.environ["INPUT"], "access.log")):
    m = pat.match(line.rstrip("\n"))
    if not m:
        continue
    ip, _, path, code, size = m.groups()
    path = path.split("?")[0]
    status[code] = status.get(code, 0) + 1
    paths[path] = paths.get(path, 0) + 1
    total += 0 if size == "-" else int(size)
    if code.startswith("5"):
        errors[ip] = errors.get(ip, 0) + 1
for code in sorted(status):
    print(code, status[code])
print("bytes", total)
top = sorted(paths, key=lambda p: (-paths[p], p))[0]
print("top", top, paths[top])
for ip in sorted(errors, key=lambda i: (-errors[i], i)):
    print("errors", ip, errors[ip])
