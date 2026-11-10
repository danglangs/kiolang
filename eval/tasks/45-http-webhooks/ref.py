import hashlib
import hmac
import json
import os
import re
import sqlite3
from http.server import BaseHTTPRequestHandler, HTTPServer

SECRETS, LOG = os.environ["SECRETS"], os.environ["LOG"]
KEY = open(os.path.join(SECRETS, "webhook.key"), "rb").read()
KEY = KEY[:-1] if KEY.endswith(b"\n") else KEY
TYPES = ("subscription.created", "subscription.activated", "payment.failed", "subscription.canceled")
MOVES = {"subscription.created": (("none",), "created"),
         "subscription.activated": (("created", "past_due"), "active"),
         "payment.failed": (("active",), "past_due"),
         "subscription.canceled": (("created", "active", "past_due"), "canceled")}

db = sqlite3.connect(os.environ["DB"])
db.execute("CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, sub TEXT, seq INTEGER, type TEXT, status TEXT)")
db.execute("CREATE TABLE IF NOT EXISTS subs (id TEXT PRIMARY KEY, state TEXT, seq INTEGER, history TEXT)")
db.commit()


def log(line):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def receive(headers, body):
    sig = headers.get("x-signature") or ""
    want = "sha256=" + hashlib.sha256(KEY + b"." + body).hexdigest()
    if not hmac.compare_digest(sig.encode(), want.encode()):
        log("rejected signature")
        return 401, {"error": "bad signature"}
    try:
        e = json.loads(body)
    except ValueError:
        e = None
    if (not isinstance(e, dict) or not isinstance(e.get("id"), str) or e.get("type") not in TYPES
            or not isinstance(e.get("subscription"), str) or not isinstance(e.get("seq"), int)
            or isinstance(e.get("seq"), bool) or e["seq"] < 1):
        return 400, {"error": "invalid event"}
    if db.execute("SELECT 1 FROM events WHERE id = ?", (e["id"],)).fetchone():
        return 200, {"status": "duplicate"}
    sub = db.execute("SELECT state, seq, history FROM subs WHERE id = ?", (e["subscription"],)).fetchone()
    state, last, history = (sub[0], sub[1], json.loads(sub[2])) if sub else ("none", 0, [])
    if e["seq"] <= last:
        db.execute("INSERT INTO events VALUES (?, ?, ?, ?, 'stale')", (e["id"], e["subscription"], e["seq"], e["type"]))
        return 200, {"status": "stale"}
    db.execute("INSERT INTO events VALUES (?, ?, ?, ?, 'queued')", (e["id"], e["subscription"], e["seq"], e["type"]))
    if e["seq"] > last + 1:
        return 202, {"status": "queued"}
    applied = 0
    while True:
        nxt = db.execute("SELECT id, type FROM events WHERE sub = ? AND seq = ? AND status = 'queued'",
                         (e["subscription"], last + 1)).fetchone()
        if not nxt:
            break
        froms, to = MOVES[nxt[1]]
        if state in froms:
            state = to
            history.append(to)
        else:
            log("invalid transition " + nxt[0])
        last += 1
        applied += 1
        db.execute("UPDATE events SET status = 'applied' WHERE id = ?", (nxt[0],))
    db.execute("INSERT OR REPLACE INTO subs VALUES (?, ?, ?, ?)", (e["subscription"], state, last, json.dumps(history)))
    return 200, {"status": "applied", "applied": applied}


def handle(method, path, headers, body):
    if method == "POST" and path == "/webhooks":
        return receive(headers, body)
    m = re.fullmatch(r"/subscriptions/([^/?]+)", path)
    if method == "GET" and m:
        sub = db.execute("SELECT state, seq, history FROM subs WHERE id = ?", (m.group(1),)).fetchone()
        if sub:
            return 200, {"id": m.group(1), "state": sub[0], "seq": sub[1], "history": json.loads(sub[2])}
    return 404, {"error": "not found"}


class Handler(BaseHTTPRequestHandler):
    def any(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n)
        status, out = handle(self.command, self.path, {k.lower(): v for k, v in self.headers.items()}, body)
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
