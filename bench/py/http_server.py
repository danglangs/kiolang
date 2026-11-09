"""kio-web's hello example with the standard library: `GET /hello/:name` answers
`{"hello": "<name>"}`, HTTP/1.1 keep-alive, a thread per connection. `http_server.py PORT`."""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Hello(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        name = self.path.removeprefix("/hello/")
        if name != self.path and "/" not in name:
            body, status = ('{"hello": ' + json.dumps(name) + "}").encode(), 200
        else:
            body, status = b"not found", 404
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        # ab sends HTTP/1.0 with `Connection: Keep-Alive` and waits for close without this.
        self.send_header("connection", "keep-alive")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Hello).serve_forever()
