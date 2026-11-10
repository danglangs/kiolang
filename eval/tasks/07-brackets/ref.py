import os

PAIR = {"(": ")", "[": "]", "{": "}"}


def check(line):
    stack = []
    for i, c in enumerate(line, 1):
        if c in PAIR:
            stack.append(PAIR[c])
        elif c in ")]}":
            if not stack:
                return "col %d: unexpected %s" % (i, c)
            want = stack.pop()
            if want != c:
                return "col %d: expected %s, found %s" % (i, want, c)
    return "unclosed " + stack[-1] if stack else "ok"


text = open(os.path.join(os.environ["INPUT"], "snippets.txt")).read()
for line in text.split("\n")[:-1] if text.endswith("\n") else text.split("\n"):
    print(check(line))
