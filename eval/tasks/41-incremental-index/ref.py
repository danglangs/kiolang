import hashlib
import json
import os
import re

DOCS, QUERIES, INDEX = os.environ["DOCS"], os.environ["QUERIES"], os.environ["INDEX"]
state = json.load(open(INDEX)) if os.path.exists(INDEX) else {"hashes": {}, "words": {}}
hashes, words = state["hashes"], state["words"]  # path -> sha256; word -> sorted paths


def tokens(b):
    return [w.decode().lower() for w in re.split(rb"[\x00-\x2f\x3a-\x40\x5b-\x60\x7b-\x7f]+", b) if w]


def unindex(p):
    for w in list(words):
        if p in words[w]:
            words[w].remove(p)
            if not words[w]:
                del words[w]


now = {}
for root, _, files in os.walk(DOCS):
    for f in files:
        full = os.path.join(root, f)
        now[os.path.relpath(full, DOCS).replace(os.sep, "/")] = full

out, unchanged = [], 0
for p in sorted(set(now) | set(hashes), key=lambda s: s.encode()):
    if p not in now:
        out.append("removed " + p)
        unindex(p)
        del hashes[p]
        continue
    data = open(now[p], "rb").read()
    h = hashlib.sha256(data).hexdigest()
    if hashes.get(p) == h:
        unchanged += 1
        continue
    out.append(("changed " if p in hashes else "added ") + p)
    unindex(p)
    hashes[p] = h
    for w in set(tokens(data)):
        words.setdefault(w, []).append(p)
out.append("unchanged %d" % unchanged)
json.dump(state, open(INDEX, "w"))

for line in open(os.path.join(QUERIES, "queries.txt"), encoding="utf-8").read().split("\n"):
    if line.strip() == "":
        continue
    hits = None
    for w in tokens(line.encode()):
        hits = set(words.get(w, [])) if hits is None else hits & set(words.get(w, []))
    hits = sorted(hits or [], key=lambda s: s.encode())
    out.append("%s: %s" % (line, ", ".join(hits) if hits else "none"))
print("\n".join(out))
