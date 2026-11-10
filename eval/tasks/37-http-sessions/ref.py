import hashlib
import json
import os
import re
import secrets
from http.server import BaseHTTPRequestHandler, HTTPServer

STORE, NOW = os.environ["STORE"], int(os.environ["CLOCK"])
TTL, LOCK = 30 * 60000, 15 * 60000


def load():
    return json.load(open(STORE)) if os.path.exists(STORE) else {"users": {}, "sessions": {}}


def digest(name, pw):
    return hashlib.sha256((name + ":" + pw).encode()).hexdigest()


def creds(body):
    try:
        b = json.loads(body)
    except ValueError:
        return None
    if isinstance(b, dict) and isinstance(b.get("name"), str) and isinstance(b.get("password"), str):
        return b["name"], b["password"]
    return None


def handle(method, path, headers, body):
    db = load()
    users, sessions = db["users"], db["sessions"]
    auth = headers.get("authorization") or ""
    token = auth[len("Bearer "):] if auth.startswith("Bearer ") else None
    if method == "POST" and path == "/users":
        c = creds(body)
        if not c or not re.fullmatch(r"[a-z0-9_]{3,20}", c[0]) or len(c[1].encode()) < 8:
            return 400, {"error": "invalid"}, db
        if c[0] in users:
            return 409, {"error": "name taken"}, db
        users[c[0]] = {"hash": digest(*c), "fails": 0, "locked_until": 0, "logins": 0}
        return 201, {"name": c[0]}, db
    if method == "POST" and path == "/login":
        c = creds(body)
        u = users.get(c[0]) if c else None
        if not u:
            return 401, {"error": "bad credentials"}, db
        if NOW < u["locked_until"]:
            return 429, {"error": "locked"}, db
        if u["hash"] != digest(*c):
            u["fails"] += 1
            if u["fails"] >= 3:
                u["fails"], u["locked_until"] = 0, NOW + LOCK
            return 401, {"error": "bad credentials"}, db
        u["fails"] = 0
        u["logins"] += 1
        t = secrets.token_hex(16)
        sessions[t] = {"name": c[0], "expires": NOW + TTL}
        return 200, {"token": t, "expires_in": 1800}, db
    if path in ("/me", "/logout") and method == ("GET" if path == "/me" else "POST"):
        s = sessions.get(token) if token else None
        if not s or NOW >= s["expires"]:
            return 401, {"error": "unauthorized"}, db
        if path == "/logout":
            del sessions[token]
            return 204, None, db
        s["expires"] = NOW + TTL
        return 200, {"name": s["name"], "logins": users[s["name"]]["logins"]}, db
    return 404, {"error": "not found"}, db


class Handler(BaseHTTPRequestHandler):
    def any(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace")
        status, out, db = handle(self.command, self.path, {k.lower(): v for k, v in self.headers.items()}, body)
        json.dump(db, open(STORE, "w"))
        data = b"" if out is None else json.dumps(out).encode()
        self.send_response(status)
        if out is not None:
            self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = any

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", int(os.environ["PORT"])), Handler).serve_forever()
