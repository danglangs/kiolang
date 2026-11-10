# api.py PORT ROUTES.json [LOG]: a fixture JSON API for the Http client tasks.
# ROUTES maps "METHOD /path?query" to {"status": N, "body": <JSON>}; anything else is 404.
# "raw": "text" in place of "body" sends that text as is. A route may map to a list of
# responses instead: the n-th request gets the n-th, the last repeating (a flaky upstream).
# Every request is appended to LOG as "METHOD /path BODY" (one line; GET bodies are
# empty), whatever the answer.
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, routes = int(sys.argv[1]), json.load(open(sys.argv[2]))
log = sys.argv[3] if len(sys.argv) > 3 else None
seen, lock = {}, threading.Lock()


class Handler(BaseHTTPRequestHandler):
    def handle_any(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        if log:
            with open(log, "a", encoding="utf-8") as f:
                f.write("%s %s %s\n" % (self.command, self.path, body.replace("\n", " ")))
        key = "%s %s" % (self.command, self.path)
        r = routes.get(key, {"status": 404, "body": {"error": "not found"}})
        if isinstance(r, list):
            with lock:
                n = seen[key] = seen.get(key, 0) + 1
            r = r[min(n, len(r)) - 1]
        out = r["raw"].encode() if "raw" in r else json.dumps(r["body"]).encode()
        self.send_response(r["status"])
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_any

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
