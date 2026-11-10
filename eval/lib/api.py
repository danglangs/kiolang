# api.py PORT ROUTES.json [LOG]: a fixture JSON API for the Http client tasks.
# ROUTES maps "METHOD /path?query" to {"status": N, "body": <JSON>}; anything else is 404.
# A POST, PUT or PATCH is answered from its route and its body appended to LOG as
# "METHOD /path BODY" (one line).
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, routes = int(sys.argv[1]), json.load(open(sys.argv[2]))
log = sys.argv[3] if len(sys.argv) > 3 else None


class Handler(BaseHTTPRequestHandler):
    def handle_any(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        if log and self.command in ("POST", "PUT", "PATCH"):
            with open(log, "a") as f:
                f.write("%s %s %s\n" % (self.command, self.path, body.replace("\n", " ")))
        r = routes.get("%s %s" % (self.command, self.path), {"status": 404, "body": {"error": "not found"}})
        out = json.dumps(r["body"]).encode()
        self.send_response(r["status"])
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_any

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
