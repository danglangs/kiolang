import json
import os
import re
from http.server import BaseHTTPRequestHandler, HTTPServer

STORE = os.environ["STORE"]


def load():
    return json.load(open(STORE)) if os.path.exists(STORE) else {"next": 1, "todos": []}


def save(s):
    json.dump(s, open(STORE, "w"))


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, body=None):
        data = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        if body is not None:
            self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def body(self):
        n = int(self.headers.get("content-length") or 0)
        try:
            return json.loads(self.rfile.read(n))
        except ValueError:
            return None

    def route(self):
        s = load()
        path, _, query = self.path.partition("?")
        if path == "/todos" and self.command == "GET":
            todos = s["todos"]
            if query in ("done=true", "done=false"):
                todos = [t for t in todos if t["done"] == (query == "done=true")]
            return self.reply(200, todos)
        if path == "/todos" and self.command == "POST":
            b = self.body()
            title = b.get("title") if isinstance(b, dict) else None
            if not isinstance(title, str) or not title.strip():
                return self.reply(400, {"error": "title required"})
            t = {"id": s["next"], "title": title.strip(), "done": False}
            s["next"] += 1
            s["todos"].append(t)
            save(s)
            return self.reply(201, t)
        m = re.fullmatch(r"/todos/(\d+)", path)
        todo = next((t for t in s["todos"] if m and t["id"] == int(m.group(1))), None)
        if todo is None or self.command not in ("GET", "PATCH", "DELETE"):
            return self.reply(404, {"error": "not found"})
        if self.command == "GET":
            return self.reply(200, todo)
        if self.command == "DELETE":
            s["todos"].remove(todo)
            save(s)
            return self.reply(204)
        b = self.body()
        ok = isinstance(b, dict)
        if ok and "title" in b:
            ok = isinstance(b["title"], str) and bool(b["title"].strip())
        if ok and "done" in b:
            ok = isinstance(b["done"], bool)
        if not ok:
            return self.reply(400, {"error": "bad update"})
        if "title" in b:
            todo["title"] = b["title"].strip()
        if "done" in b:
            todo["done"] = b["done"]
        save(s)
        self.reply(200, todo)

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = route

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", int(os.environ["PORT"])), Handler).serve_forever()
