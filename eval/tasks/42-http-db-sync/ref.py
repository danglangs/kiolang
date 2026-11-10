import json
import os
import sqlite3
import urllib.error
import urllib.request

API = os.environ["API"]
db = sqlite3.connect(os.environ["DB"])
db.execute("CREATE TABLE IF NOT EXISTS items (sku TEXT PRIMARY KEY, name TEXT, qty INTEGER, version INTEGER, live INTEGER)")
db.execute("CREATE TABLE IF NOT EXISTS sync (id INTEGER PRIMARY KEY CHECK (id = 1), cursor TEXT)")
db.commit()
row = db.execute("SELECT cursor FROM sync").fetchone()
cursor = row[0] if row else "0"


def is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def valid(c):
    if not isinstance(c, dict) or not isinstance(c.get("sku"), str) or not is_int(c.get("version")):
        return False
    if c.get("op") == "delete":
        return True
    return c.get("op") == "upsert" and isinstance(c.get("name"), str) and is_int(c.get("qty"))


pages = applied = stale = invalid = 0
stop = None
while True:
    try:
        with urllib.request.urlopen("%s/changes?cursor=%s" % (API, cursor), timeout=10) as r:
            status, body = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, body = e.code, b""
    except OSError:
        stop = "no response"
        break
    if status != 200:
        stop = "status %d" % status
        break
    try:
        page = json.loads(body)
    except ValueError:
        page = None
    if not isinstance(page, dict) or not isinstance(page.get("changes"), list):
        stop = "invalid page"
        break
    with db:
        for c in page["changes"]:
            if not valid(c):
                invalid += 1
                continue
            old = db.execute("SELECT version FROM items WHERE sku = ?", (c["sku"],)).fetchone()
            if old and c["version"] <= old[0]:
                stale += 1
                continue
            applied += 1
            if c["op"] == "delete":
                db.execute("INSERT OR REPLACE INTO items VALUES (?, '', 0, ?, 0)", (c["sku"], c["version"]))
            else:
                db.execute("INSERT OR REPLACE INTO items VALUES (?, ?, ?, ?, 1)",
                           (c["sku"], c["name"], c["qty"], c["version"]))
        nxt = page.get("next")
        if isinstance(nxt, str):
            cursor = nxt
        db.execute("INSERT OR REPLACE INTO sync VALUES (1, ?)", (cursor,))
    pages += 1
    if not isinstance(nxt, str):
        break

out = ["pages %d applied %d stale %d invalid %d" % (pages, applied, stale, invalid),
       "stopped at %s (%s)" % (cursor, stop) if stop else "complete"]
rows = db.execute("SELECT sku, qty, name FROM items WHERE live = 1").fetchall()
for sku, qty, name in sorted(rows, key=lambda r: r[0].encode()):
    out.append("%s %d %s" % (sku, qty, name))
print("\n".join(out))
