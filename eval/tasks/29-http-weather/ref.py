import json
import os
import urllib.request
from urllib.error import HTTPError

API = os.environ["API"]


def get(path):
    with urllib.request.urlopen(API + path, timeout=10) as r:
        return json.load(r)


def deg(v):
    return "%s%d.%d" % ("-" if v < 0 else "", abs(v) // 10, abs(v) % 10)


for s in sorted(get("/stations")["stations"], key=lambda s: s["name"].encode()):
    try:
        temps = [r["temp"] for r in get("/stations/%s/readings" % s["id"])["readings"]]
    except HTTPError:
        print(s["name"], "unavailable")
        continue
    if not temps:
        print(s["name"], "no data")
        continue
    total = sum(temps)
    avg = abs(total) // len(temps) * (-1 if total < 0 else 1)
    print("%s min=%s max=%s avg=%s n=%d" % (s["name"], deg(min(temps)), deg(max(temps)), deg(avg), len(temps)))
