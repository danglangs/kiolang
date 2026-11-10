# same.py WANT GOT: equal up to trailing spaces per line and trailing blank lines; else a diff.
import difflib
import sys


def lines(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        ls = [line.rstrip() for line in f.read().split("\n")]
    while ls and ls[-1] == "":
        ls.pop()
    return ls


want, got = lines(sys.argv[1]), lines(sys.argv[2])
if want == got:
    sys.exit(0)
print("FAIL: output differs (- expected, + got)")
diff = list(difflib.unified_diff(want, got, "expected", "got", lineterm="", n=2))
print("\n".join(diff[:60]))
if len(diff) > 60:
    print("... (%d more diff lines)" % (len(diff) - 60))
sys.exit(1)
