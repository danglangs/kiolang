import json
import os

INPUT = os.environ["INPUT"]
schema = json.load(open(os.path.join(INPUT, "schema.json"), encoding="utf-8"))

TYPES = {
    "object": lambda v: isinstance(v, dict),
    "array": lambda v: isinstance(v, list),
    "string": lambda v: isinstance(v, str),
    "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
    "boolean": lambda v: isinstance(v, bool),
    "null": lambda v: v is None,
}


def check(s, v, path, errs):
    t = s["type"]
    if not TYPES[t](v):
        errs.append((path, "expected " + t))
        return
    if t == "object":
        props = s.get("properties", {})
        for f in s.get("required", []):
            if f not in v:
                errs.append((path + "." + f, "missing required field"))
        for k, x in v.items():
            if k in props:
                check(props[k], x, path + "." + k, errs)
            elif s.get("additional", True) is False:
                errs.append((path + "." + k, "unexpected field"))
    elif t == "array":
        if "min_items" in s and len(v) < s["min_items"]:
            errs.append((path, "fewer than %d items" % s["min_items"]))
        if "items" in s:
            for i, x in enumerate(v):
                check(s["items"], x, "%s[%d]" % (path, i), errs)
    elif t == "string":
        if "min_length" in s and len(v) < s["min_length"]:
            errs.append((path, "shorter than %d" % s["min_length"]))
        elif "max_length" in s and len(v) > s["max_length"]:
            errs.append((path, "longer than %d" % s["max_length"]))
        elif "enum" in s and v not in s["enum"]:
            errs.append((path, "not in enum"))
    elif t in ("integer", "number"):
        if "minimum" in s and v < s["minimum"]:
            errs.append((path, "less than %d" % s["minimum"]))
        elif "maximum" in s and v > s["maximum"]:
            errs.append((path, "greater than %d" % s["maximum"]))


out, valid, total = [], 0, 0
lines = open(os.path.join(INPUT, "records.jsonl"), encoding="utf-8").read().split("\n")
for n, line in enumerate(lines, start=1):
    if line.strip() == "":
        continue
    total += 1
    try:
        rec = json.loads(line)
    except ValueError:
        out.append("line %d: invalid JSON" % n)
        continue
    errs = []
    check(schema, rec, "$", errs)
    if not errs:
        valid += 1
        out.append("line %d: ok" % n)
    for p, m in sorted(errs, key=lambda e: (e[0].encode(), e[1].encode())):
        out.append("line %d: %s: %s" % (n, p, m))
out.append("valid %d of %d" % (valid, total))
print("\n".join(out))
