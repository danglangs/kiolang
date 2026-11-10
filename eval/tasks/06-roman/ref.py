import os
import re

VALS = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")]


def to_roman(n):
    out = ""
    for v, s in VALS:
        while n >= v:
            out, n = out + s, n - v
    return out


ROMAN = {to_roman(n): n for n in range(1, 4000)}
for line in open(os.path.join(os.environ["INPUT"], "numbers.txt")).read().split("\n"):
    v = line.strip()
    if not v:
        continue
    if re.fullmatch(r"-?[0-9]+", v):
        n = int(v)
        print(v, "=", to_roman(n) if 1 <= n <= 3999 else "invalid")
    else:
        print(v, "=", ROMAN.get(v, "invalid"))
