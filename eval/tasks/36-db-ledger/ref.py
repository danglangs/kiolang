import datetime
import os
import re
import sqlite3

INPUT, DB, LOG, NOW = os.environ["INPUT"], os.environ["DB"], os.environ["LOG"], int(os.environ["CLOCK"])
MAX = 2 ** 63 - 1
TIME = datetime.datetime.fromtimestamp(NOW // 1000, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

db = sqlite3.connect(DB)
db.execute("CREATE TABLE IF NOT EXISTS accounts (name TEXT PRIMARY KEY, cents INTEGER, open INTEGER, last INTEGER)")
db.execute("CREATE TABLE IF NOT EXISTS transfers (id TEXT PRIMARY KEY, src TEXT, dst TEXT, cents INTEGER)")


def account(name):
    return db.execute("SELECT cents, open FROM accounts WHERE name = ?", (name,)).fetchone()


def is_int(s):
    return re.fullmatch(r"-?\d+", s) is not None


def apply(w):
    if w[0] == "open" and len(w) == 3 and is_int(w[2]):
        if int(w[2]) < 0:
            return "bad amount"
        if account(w[1]):
            return "account exists"
        db.execute("INSERT INTO accounts VALUES (?, ?, 1, ?)", (w[1], int(w[2]), NOW))
        return None
    if w[0] == "close" and len(w) == 2:
        a = account(w[1])
        if not a or not a[1]:
            return "no account " + w[1]
        if a[0] != 0:
            return "balance not zero"
        db.execute("UPDATE accounts SET open = 0 WHERE name = ?", (w[1],))
        return None
    if w[0] == "transfer" and len(w) == 5 and is_int(w[4]):
        tid, src, dst, cents = w[1], w[2], w[3], int(w[4])
        old = db.execute("SELECT src, dst, cents FROM transfers WHERE id = ?", (tid,)).fetchone()
        if old:
            return "duplicate" if old == (src, dst, cents) else "id reused"
        if cents <= 0:
            return "bad amount"
        if src == dst:
            return "same account"
        a, b = account(src), account(dst)
        if not a or not a[1]:
            return "no account " + src
        if not b or not b[1]:
            return "no account " + dst
        if a[0] < cents:
            return "insufficient funds"
        if b[0] > MAX - cents:
            return "overflow"
        with db:
            db.execute("UPDATE accounts SET cents = cents - ?, last = ? WHERE name = ?", (cents, NOW, src))
            db.execute("UPDATE accounts SET cents = cents + ?, last = ? WHERE name = ?", (cents, NOW, dst))
            db.execute("INSERT INTO transfers VALUES (?, ?, ?, ?)", (tid, src, dst, cents))
        return None
    return "malformed"


ok = failed = dup = 0
lines = open(os.path.join(INPUT, "ops.txt"), encoding="utf-8").read().split("\n")
with open(LOG, "a", encoding="utf-8") as log:
    for n, line in enumerate(lines, start=1):
        if line.strip() == "":
            continue
        err = apply(line.split(" "))
        if err is None:
            ok += 1
        elif err == "duplicate":
            dup += 1
        else:
            failed += 1
            log.write("%s line %d: %s\n" % (TIME, n, err))
db.commit()
out = ["ok %d failed %d duplicate %d" % (ok, failed, dup)]
rows = db.execute("SELECT name, cents, last FROM accounts WHERE open = 1").fetchall()
for name, cents, last in sorted(rows, key=lambda r: r[0].encode()):
    t = datetime.datetime.fromtimestamp(last // 1000, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    out.append("%s %d last %s" % (name, cents, t))
print("\n".join(out))
