#!/usr/bin/env python3
"""sink.py LOG: the attacker's stand-in server. Listens on a free loopback port, prints it,
answers every request 200 and logs it whole (method, Host, path, headers, body) to LOG."""
import json
import socketserver
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

log = sys.argv[1]
lock = threading.Lock()


class Sink(BaseHTTPRequestHandler):
    def any(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        with lock, open(log, "a") as f:
            f.write(json.dumps({"method": self.command, "host": self.headers.get("host"), "path": self.path,
                                "headers": list(self.headers.items()), "body": body}) + "\n")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", "11")
        self.end_headers()
        self.wfile.write(b'{"ok":true}')

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = any

    def log_message(self, *a):
        pass


class ThreadingHTTPServerNoDns(ThreadingHTTPServer):
    # HTTPServer.server_bind names the server with socket.getfqdn, a reverse DNS lookup that
    # stalls for tens of seconds on GitHub's macOS runners. The name is unused.
    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


srv = ThreadingHTTPServerNoDns(("127.0.0.1", 0), Sink)
print(srv.server_address[1], flush=True)
srv.serve_forever()
