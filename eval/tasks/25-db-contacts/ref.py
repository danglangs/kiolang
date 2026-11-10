import os
import sqlite3

db = sqlite3.connect(os.environ["CRM"])
db.execute("CREATE TABLE IF NOT EXISTS contacts (email TEXT PRIMARY KEY, name TEXT, phone TEXT)")
counts = {"inserted": 0, "updated": 0, "unchanged": 0, "rejected": 0}
for line in open(os.path.join(os.environ["INPUT"], "contacts.csv")).read().split("\n")[1:]:
    if not line.strip():
        continue
    f = [x.strip() for x in line.split(",")]
    if len(f) != 3 or "@" not in f[0]:
        counts["rejected"] += 1
        continue
    email, name, phone = f[0].lower(), f[1], f[2]
    row = db.execute("SELECT name, phone FROM contacts WHERE email = ?", (email,)).fetchone()
    if row is None:
        db.execute("INSERT INTO contacts VALUES (?, ?, ?)", (email, name, phone))
        counts["inserted"] += 1
    elif row != (name, phone):
        db.execute("UPDATE contacts SET name = ?, phone = ? WHERE email = ?", (name, phone, email))
        counts["updated"] += 1
    else:
        counts["unchanged"] += 1
db.commit()
print(" ".join("%s %d" % kv for kv in counts.items()))
print("contacts", db.execute("SELECT COUNT(*) FROM contacts").fetchone()[0])
for (email,) in db.execute("SELECT email FROM contacts WHERE phone = '' ORDER BY email"):
    print("missing phone:", email)
