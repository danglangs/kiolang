import json
import os
import re
import sqlite3
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs

db = sqlite3.connect(os.environ["DB"])
db.execute("CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT, body TEXT, tags TEXT)")
db.commit()


def note(row):
    return {"id": row[0], "title": row[1], "body": row[2], "tags": json.loads(row[3])}


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def route(self):
        path, _, query = self.path.partition("?")
        if path == "/notes" and self.command == "GET":
            q = {k: v[0] for k, v in parse_qs(query).items()}
            notes = [note(r) for r in db.execute("SELECT * FROM notes ORDER BY id DESC")]
            if "tag" in q:
                notes = [n for n in notes if q["tag"].lower() in n["tags"]]
            if "q" in q:
                notes = [n for n in notes if q["q"] in n["title"] or q["q"] in n["body"]]
            return self.reply(200, [{"id": n["id"], "title": n["title"]} for n in notes])
        if path == "/notes" and self.command == "POST":
            try:
                b = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)))
            except ValueError:
                b = None
            tags = b.get("tags", []) if isinstance(b, dict) else None
            if (not isinstance(b, dict) or not isinstance(b.get("title"), str) or not b["title"]
                    or not isinstance(b.get("body"), str) or not isinstance(tags, list)
                    or not all(isinstance(t, str) for t in tags)):
                return self.reply(400, {"error": "invalid note"})
            clean = []
            for t in tags:
                if t.lower() not in clean:
                    clean.append(t.lower())
            cur = db.execute("INSERT INTO notes (title, body, tags) VALUES (?, ?, ?)", (b["title"], b["body"], json.dumps(clean)))
            db.commit()
            return self.reply(201, {"id": cur.lastrowid})
        m = re.fullmatch(r"/notes/(\d+)", path)
        row = db.execute("SELECT * FROM notes WHERE id = ?", (int(m.group(1)),)).fetchone() if m else None
        if row is None or self.command not in ("GET", "DELETE"):
            return self.reply(404, {"error": "not found"})
        if self.command == "GET":
            return self.reply(200, note(row))
        db.execute("DELETE FROM notes WHERE id = ?", (row[0],))
        db.commit()
        self.reply(200, {"deleted": row[0]})

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = route

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", int(os.environ["PORT"])), Handler).serve_forever()
