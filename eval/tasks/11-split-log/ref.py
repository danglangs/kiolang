import os

files = {"error": [], "warn": [], "info": []}
other = 0
for line in open(os.path.join(os.environ["INPUT"], "app.log")).read().split("\n"):
    if not line.strip():
        continue
    f = line.split(" ")
    level = f[1].lower() if len(f) >= 3 else ""
    level = "warn" if level == "warning" else level
    if level in files:
        files[level].append(line)
    else:
        other += 1
for k, lines in files.items():
    with open(os.path.join(os.environ["OUT"], k + ".log"), "w") as f:
        f.write("".join(line + "\n" for line in lines))
for k in files:
    print(k, len(files[k]))
print("other", other)
