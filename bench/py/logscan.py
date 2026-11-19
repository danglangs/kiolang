# kio/logscan.kio: a character-by-character `key=value` parser, counts and sums per route.
import sys
def toint(s):
    try: return int(s)
    except ValueError: return None
def route(path):
    out, seg = "", ""
    for c in path:
        if c == "/":
            if seg != "" and toint(seg) is None: out += "/" + seg
            seg = ""
        else: seg += c
    if seg != "" and toint(seg) is None: out += "/" + seg
    return out
text = open(sys.argv[1]).read()
statuses, dur, errors = {}, {}, 0
for line in text.splitlines():
    key = val = path = status = ""; in_val = False; ms = 0
    for c in line + " ":
        if c == " ":
            if key == "path": path = val
            elif key == "status": status = val
            elif key == "dur_ms": ms = toint(val) or 0
            elif key == "level" and val == "error": errors += 1
            key = val = ""; in_val = False
        elif c == "=" and not in_val: in_val = True
        elif in_val: val += c
        else: key += c
    statuses[status] = statuses.get(status, 0) + 1
    r = route(path); dur[r] = dur.get(r, 0) + ms
check = errors
for k in sorted(statuses): check = (check * 31 + statuses[k]) % 1000000007
for k in sorted(dur, key=lambda s: s.encode()): check = (check * 31 + dur[k] + len(k)) % 1000000007
print(check)
