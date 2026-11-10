import json
import os


def merge(base, over):
    out = dict(base)
    for k, v in over.items():
        if v is None:
            out.pop(k, None)
        elif isinstance(v, dict) and isinstance(out.get(k), dict):
            out[k] = merge(out[k], v)
        else:
            out[k] = v
    return out


def leaves(path, v, out):
    if isinstance(v, dict) and v:
        for k, x in v.items():
            leaves(path + [k], x, out)
    elif isinstance(v, list) and v:
        for i, x in enumerate(v):
            leaves(path + [str(i)], x, out)
    else:
        out.append((".".join(path), json.dumps(v, separators=(",", ":"))))


d = os.environ["INPUT"]
merged = merge(json.load(open(os.path.join(d, "base.json"))), json.load(open(os.path.join(d, "override.json"))))
out = []
leaves([], merged, out)
for k, v in sorted(out, key=lambda kv: kv[0].encode()):
    print("%s=%s" % (k, v))
