"""A stand-in orders API on 127.0.0.1:8787 (Python standard library only).

POST /orders with `Authorization: $API_KEY` gets 201; a wrong or missing key gets 401.
"""
import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer

KEY = os.environ["API_KEY"]


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        if self.headers.get("authorization") != KEY:
            status = 401
        elif self.path != "/orders":
            status = 404
        else:
            o = json.loads(body)
            print(f"api: got order {o['id']} ({o['customer']}, {o['cents']} cents)", flush=True)
            status = 201
        self.send_response(status)
        self.send_header("content-length", "0")
        self.end_headers()

    def log_message(self, *args):
        pass


server = HTTPServer(("127.0.0.1", 8787), Handler)
print("api: listening on http://127.0.0.1:8787", flush=True)
server.serve_forever()
