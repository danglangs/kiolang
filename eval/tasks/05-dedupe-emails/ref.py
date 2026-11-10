import os

seen, invalid = set(), 0
for line in open(os.path.join(os.environ["INPUT"], "emails.txt")).read().split("\n"):
    e = line.strip().lower()
    if not e:
        continue
    parts = e.split("@")
    if len(parts) != 2 or not parts[0] or "." not in parts[1] or parts[1][0] == "." or parts[1][-1] == ".":
        invalid += 1
        continue
    local, domain = parts
    if domain == "gmail.com":
        local = local.split("+")[0].replace(".", "")
    seen.add(local + "@" + domain)
for e in sorted(seen):
    print(e)
print("invalid", invalid)
