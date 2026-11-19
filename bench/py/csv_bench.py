# kio/csv.kio: CSV lines into records, an email check, totals per city.
import sys
def valid_email(e):
    p = e.split("@")
    return len(p) == 2 and p[0] != "" and "." in p[1]
def parse(line):
    f = line.split(",")
    if len(f) != 6: return None
    try: i, cents, qty = int(f[0]), int(f[4]), int(f[5])
    except ValueError: return None
    if not valid_email(f[2]) or qty <= 0: return None
    return dict(id=i, name=f[1].strip(), email=f[2].lower(), city=f[3], cents=cents, qty=qty)
text = open(sys.argv[1]).read()
orders, bad = [], 0
for line in text.splitlines()[1:]:
    o = parse(line)
    if o is None: bad += 1
    else: orders.append(o)
by = {}
for o in orders:
    a = by.setdefault(o["city"], [0, 0, 0, 0])
    a[0] += 1; a[1] += o["cents"] * o["qty"]; a[2] += o["qty"]; a[3] = max(a[3], o["cents"])
check = 0
for k in sorted(by):
    a = by[k]
    check = (check * 31 + a[1] % 1000003 + a[0] + a[2] + a[3]) % 1000000007
print(check * 1000 + bad)
