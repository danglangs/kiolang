"""A local stand-in for an authenticated rates API (job set 2).

GET /v1/rates?base=USD answers only with `Authorization: <$KIO_DEMO_RATES_KEY>`; every
request is logged (method, path, whether the key matched; never the header's value).
"""

from __future__ import annotations

import json
import os
import secrets
import socket
import socketserver
import subprocess
import sys
import time
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Iterator

from .jobs import RATES

VAR = "KIO_DEMO_RATES_KEY"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802
        ok = secrets.compare_digest(self.headers.get("Authorization", ""), os.environ[VAR])
        with open(self.server.log_path, "a") as f:  # type: ignore[attr-defined]
            f.write(json.dumps({"method": "GET", "path": self.path, "auth_ok": ok}) + "\n")
        if not ok:
            return self._send(401, {"error": "missing or wrong Authorization"})
        if self.path.split("?")[0] != "/v1/rates":
            return self._send(404, {"error": "not found"})
        self._send(200, {"base": "USD", "rates": RATES})

    def _send(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


@contextmanager
def running(log_path: Path) -> Iterator[str]:
    """Start the stand-in in a child process with a fresh fake key; yield its origin.
    The key is put in this process's environment (not a shell's) so the `kio` child, which
    injects it, inherits it; it is removed afterwards."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    os.environ[VAR] = "Bearer fake-" + secrets.token_hex(8)
    p = subprocess.Popen([sys.executable, "-m", "agent_demo.standin", str(port), str(log_path)],
                         cwd=Path(__file__).resolve().parents[1])
    try:
        for _ in range(50):
            try:
                socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
                break
            except OSError:
                time.sleep(0.1)
        yield f"http://127.0.0.1:{port}"
    finally:
        p.terminate()
        p.wait()
        del os.environ[VAR]


class HTTPServerNoDns(HTTPServer):
    # HTTPServer.server_bind names the server with socket.getfqdn, a reverse DNS lookup that
    # can stall for tens of seconds. The name is unused.
    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


if __name__ == "__main__":
    srv = HTTPServerNoDns(("127.0.0.1", int(sys.argv[1])), Handler)
    srv.log_path = sys.argv[2]  # type: ignore[attr-defined]
    srv.serve_forever()
