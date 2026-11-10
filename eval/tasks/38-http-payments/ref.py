import json
import os
import re
import sqlite3
from http.server import BaseHTTPRequestHandler, HTTPServer

db = sqlite3.connect(os.environ["DB"])
db.execute("CREATE TABLE IF NOT EXISTS payments (n INTEGER PRIMARY KEY, account TEXT, cents INTEGER,"
           " currency TEXT, status TEXT, refunded INTEGER)")
db.execute("CREATE TABLE IF NOT EXISTS keys (k TEXT PRIMARY KEY, request TEXT, status INTEGER, body TEXT)")
db.commit()


def payment(n):
    r = db.execute("SELECT n, account, cents, currency, status, refunded FROM payments WHERE n = ?", (n,)).fetchone()
    return r and {"id": "pay_%d" % r[0], "account": r[1], "cents": r[2], "currency": r[3], "status": r[4],
                  "refunded": r[5]}


def is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def body_json(body):
    try:
        return json.loads(body)
    except ValueError:
        return None


def create(b):
    if (not isinstance(b, dict) or set(b) != {"account", "cents", "currency"} or not isinstance(b["account"], str)
            or not b["account"] or not is_int(b["cents"]) or b["cents"] <= 0 or b["currency"] not in ("EUR", "USD")):
        return 422, {"error": "invalid payment"}
    cur = db.execute("INSERT INTO payments (account, cents, currency, status, refunded) VALUES (?, ?, ?, 'pending', 0)",
                     (b["account"], b["cents"], b["currency"]))
    return 201, payment(cur.lastrowid)


def act(n, verb, b):
    p = payment(n)
    if not p:
        return 404, {"error": "not found"}
    if verb == "capture":
        if p["status"] not in ("pending", "captured"):
            return 409, {"error": "invalid transition"}
        db.execute("UPDATE payments SET status = 'captured' WHERE n = ?", (n,))
    elif verb == "cancel":
        if p["status"] != "pending":
            return 409, {"error": "invalid transition"}
        db.execute("UPDATE payments SET status = 'canceled' WHERE n = ?", (n,))
    else:
        if not isinstance(b, dict) or not is_int(b.get("cents")) or b["cents"] <= 0:
            return 422, {"error": "invalid refund"}
        if p["status"] != "captured":
            return 409, {"error": "invalid transition"}
        total = p["refunded"] + b["cents"]
        if total > p["cents"]:
            return 409, {"error": "exceeds captured"}
        db.execute("UPDATE payments SET refunded = ?, status = ? WHERE n = ?",
                   (total, "refunded" if total == p["cents"] else "captured", n))
    return 200, payment(n)


def handle(method, path, key, body):
    m = re.fullmatch(r"/payments/pay_([1-9][0-9]*)(?:/(capture|cancel|refund))?", path)
    if method == "GET" and m and not m.group(2):
        p = payment(int(m.group(1)))
        return (200, p) if p else (404, {"error": "not found"})
    m2 = re.fullmatch(r"/payments\?account=(.*)", path)
    if method == "GET" and m2:
        rows = db.execute("SELECT n FROM payments WHERE account = ? ORDER BY n", (m2.group(1),)).fetchall()
        return 200, [payment(r[0]) for r in rows]
    keyed = method == "POST" and (path == "/payments" or (m and m.group(2) == "refund"))
    if not keyed:
        if method == "POST" and m and m.group(2):
            return act(int(m.group(1)), m.group(2), None)
        return 404, {"error": "not found"}
    if not key:
        return 400, {"error": "missing idempotency key"}
    b = body_json(body)
    request = path + " " + json.dumps(b, sort_keys=True)
    old = db.execute("SELECT request, status, body FROM keys WHERE k = ?", (key,)).fetchone()
    if old:
        return (old[1], json.loads(old[2])) if old[0] == request else (409, {"error": "key reused"})
    status, out = create(b) if path == "/payments" else act(int(m.group(1)), "refund", b)
    if 200 <= status < 300:
        db.execute("INSERT INTO keys VALUES (?, ?, ?, ?)", (key, request, status, json.dumps(out)))
    return status, out


class Handler(BaseHTTPRequestHandler):
    def any(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace")
        status, out = handle(self.command, self.path, self.headers.get("idempotency-key"), body)
        db.commit()
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = any

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", int(os.environ["PORT"])), Handler).serve_forever()
