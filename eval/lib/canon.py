# A response body as one comparable line: JSON with sorted keys and no spaces, else the text.
# With --line, the input is "STATUS BODY" and only BODY is canonicalized.
import json
import sys


def canon(text):
    try:
        return json.dumps(json.loads(text), sort_keys=True, separators=(",", ":"))
    except ValueError:
        return text.strip().replace("\n", "\\n")


data = sys.stdin.read()
if len(sys.argv) > 1:
    status, _, body = data.strip().partition(" ")
    print(status, canon(body))
else:
    print(canon(data))
