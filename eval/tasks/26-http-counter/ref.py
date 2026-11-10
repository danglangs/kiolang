import json
import os
import re
from http.server import BaseHTTPRequestHandler, HTTPServer

STORE = os.environ["COUNTS"]


def load():
    return json.load(open(STORE)) if os.path.exists(STORE) else {}


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def route(self):
        counts = load()
        if self.command == "GET" and self.path == "/top":
            top = sorted(counts, key=lambda n: (-counts[n], n.encode()))[:3]
            return self.reply(200, [{"name": n, "count": counts[n]} for n in top])
        m = re.fullmatch(r"/(hit|count)/([^/]*)", self.path)
        if not m or (self.command, m.group(1)) not in (("POST", "hit"), ("GET", "count")):
            return self.reply(404, {"error": "not found"})
        name = m.group(2)
        if not re.fullmatch(r"[a-z0-9-]{1,32}", name):
            return self.reply(400, {"error": "bad name"})
        if m.group(1) == "hit":
            counts[name] = counts.get(name, 0) + 1
            json.dump(counts, open(STORE, "w"))
        self.reply(200, {"name": name, "count": counts.get(name, 0)})

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = route

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", int(os.environ["PORT"])), Handler).serve_forever()
