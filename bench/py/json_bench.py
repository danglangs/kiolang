import json
import sys

with open(sys.argv[1]) as f:
    text = f.read()
print(len(json.dumps(json.loads(text), separators=(",", ":"))))
