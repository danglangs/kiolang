import os
import re

INPUT, OUT = os.environ["INPUT"], os.environ["OUT"]
text = open(os.path.join(INPUT, "sales.csv"), encoding="utf-8", newline="").read()


def records(s):
    """Yields (fields, ok); ok is False for bad quoting or an unterminated quote."""
    i, n = 0, len(s)
    while i < n:
        fields, ok, field = [], True, ""
        while True:
            if i < n and s[i] == '"':
                i += 1
                while True:
                    if i >= n:
                        yield fields + [field], False
                        return
                    if s[i] == '"':
                        if i + 1 < n and s[i + 1] == '"':
                            field += '"'
                            i += 2
                            continue
                        i += 1
                        break
                    field += s[i]
                    i += 1
                if i < n and s[i] not in ",\r\n":
                    ok = False
            while i < n and s[i] not in ",\r\n":
                if s[i] == '"':
                    ok = False
                field += s[i]
                i += 1
            fields.append(field)
            field = ""
            if i < n and s[i] == ",":
                i += 1
                continue
            if s.startswith("\r\n", i):
                i += 2
            elif i < n:
                i += 1
            break
        yield fields, ok


def quote(f):
    return '"' + f.replace('"', '""') + '"' if any(c in f for c in ',"\r\n') else f


rows = [(f, ok) for f, ok in records(text) if not (ok and f == [""])]
clean, bad, regions = [",".join(rows[0][0])], [], {}
for n, (f, ok) in enumerate(rows[1:], start=1):
    good = ok and len(f) == 5 and f[0] and f[1] and re.fullmatch(r"-?\d+", f[2]) \
        and re.fullmatch(r"\d+(\.\d{1,2})?", f[3])
    if not good:
        bad.append(n)
        continue
    whole, _, frac = f[3].partition(".")
    cents = int(whole) * 100 + int((frac + "00")[:2])
    f[3] = "%d.%02d" % (cents // 100, cents % 100)
    clean.append(",".join(quote(x) for x in f))
    r = regions.setdefault(f[0], {"items": 0, "products": {}})
    r["items"] += int(f[2])
    r["products"][f[1]] = r["products"].get(f[1], 0) + int(f[2]) * cents

with open(os.path.join(OUT, "clean.csv"), "w", encoding="utf-8", newline="") as fh:
    fh.write("".join(line + "\n" for line in clean))


def money(c):
    return "%s%d.%02d" % ("-" if c < 0 else "", abs(c) // 100, abs(c) % 100)


out = []
for name in sorted(regions, key=lambda s: s.encode()):
    r = regions[name]
    top = sorted(r["products"], key=lambda p: (-r["products"][p], p.encode()))[0]
    out.append("%s revenue %s items %d top %s" % (name, money(sum(r["products"].values())), r["items"], top))
out.append("records %d bad %d%s" % (len(rows) - 1, len(bad), ": " + ", ".join(map(str, bad)) if bad else ""))
print("\n".join(out))
