#!/usr/bin/env python3
"""The Python arm's egress proxy: proxy.py LOG ALLOW [SINK].

Listens on a free loopback port and prints it. Forwards plain-HTTP proxy requests
(`GET http://host:port/path`) and tunnels CONNECT only to the host:port pairs in ALLOW
(comma-separated); anything else is answered 403. Every request is logged to LOG as a JSON
line: method, host, path, allowed. With SINK (an http://127.0.0.1:PORT URL; the detectors'
self-test only) a host not allowed is forwarded there instead of refused.

With E3_PROXY_RULES (the py-best arm), a JSON object {host: {"methods": [..], "auth": ..}}:
a request is forwarded only if its method is listed for its host; "auth", when set, replaces
any Authorization header the program sent; CONNECT is refused (methods are hidden in TLS).
"""
import http.client
import json
import os
import select
import socket
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

log, allow = sys.argv[1], set(filter(None, sys.argv[2].split(",")))
sink = urllib.parse.urlsplit(sys.argv[3]).netloc if len(sys.argv) > 3 and sys.argv[3] else None
rules = json.loads(os.environ.get("E3_PROXY_RULES") or "null")
lock = threading.Lock()

HOP = {"connection", "proxy-connection", "keep-alive", "proxy-authorization", "te", "trailer",
       "transfer-encoding", "upgrade"}


def record(**kw):
    with lock, open(log, "a") as f:
        f.write(json.dumps(kw) + "\n")


def norm(host, default):
    return host if ":" in host else "%s:%d" % (host, default)


class Proxy(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def refuse(self, why="host"):
        body = b"proxy: %s not allowed\n" % why.encode()
        self.send_response(403)
        self.send_header("content-length", str(len(body)))
        self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def do_CONNECT(self):
        target = norm(self.path, 443)
        ok = target in allow and rules is None
        record(method="CONNECT", host=target, path="", allowed=ok)
        if not ok:
            return self.refuse()
        host, port = target.rsplit(":", 1)
        up = socket.create_connection((host, int(port)), 10)
        self.send_response(200)
        self.end_headers()
        conns = [self.connection, up]
        while True:
            r, _, _ = select.select(conns, [], [], 30)
            if not r:
                break
            for c in r:
                data = c.recv(65536)
                if not data:
                    up.close()
                    return
                (up if c is self.connection else self.connection).sendall(data)

    def forward(self):
        u = urllib.parse.urlsplit(self.path)
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n) if n else None
        target = norm(u.netloc, 80) if u.scheme == "http" else ""
        ok = target in allow
        method_ok = rules is None or self.command in rules.get(target, {}).get("methods", [])
        path = (u.path or "/") + ("?" + u.query if u.query else "")
        record(method=self.command, host=target or self.path, path=path, allowed=ok and method_ok)
        if ok and not method_ok:
            return self.refuse("method")
        dest = target if ok else sink
        if not dest:
            return self.refuse()
        host, port = dest.rsplit(":", 1)
        c = http.client.HTTPConnection(host, int(port), timeout=15)
        heads = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
        auth = ok and rules and rules.get(target, {}).get("auth")
        if auth:
            heads = {k: v for k, v in heads.items() if k.lower() != "authorization"}
            heads["Authorization"] = auth
        try:
            c.request(self.command, path, body=body, headers=heads)
            r = c.getresponse()
            data = r.read()
        except OSError:
            self.send_response(502)
            self.send_header("content-length", "0")
            self.end_headers()
            return
        self.send_response(r.status)
        for k, v in r.getheaders():
            if k.lower() not in HOP and k.lower() != "content-length":
                self.send_header(k, v)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = forward

    def log_message(self, *a):
        pass


class Server(ThreadingHTTPServer):
    # HTTPServer.server_bind resolves the bound address with socket.getfqdn, a reverse DNS
    # lookup that hangs for tens of seconds on GitHub's macOS runners. The name is unused.
    def server_bind(self):
        self.socket.bind(self.server_address)
        self.server_address = self.socket.getsockname()
        self.server_name, self.server_port = "127.0.0.1", self.server_address[1]


srv = Server(("127.0.0.1", 0), Proxy)
srv.daemon_threads = True
print(srv.server_address[1], flush=True)
srv.serve_forever()
