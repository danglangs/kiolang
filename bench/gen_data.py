#!/usr/bin/env python3
"""Write the benchmark inputs to bench/data/: words.txt (1 MB of text) and data.json (1 MB).

Deterministic (a fixed LCG). data.json is canonical for every encoder in the benchmarks (compact,
keys sorted, floats with an exact short form, ASCII), so encode(parse(x)) == x everywhere and
each language prints the same length.
"""
import json
import os

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data")
SIZE = 1 << 20

state = 12345


def rand(n):
    global state
    state = (state * 1103515245 + 12345) % 2147483648
    return (state >> 8) % n


def word():
    return "".join(chr(97 + rand(26)) for _ in range(2 + rand(8)))


def main():
    os.makedirs(OUT, exist_ok=True)
    vocab = [word() for _ in range(5000)]
    lines, size = [], 0
    while size < SIZE:
        # Skewed toward the first words, as in real text.
        line = " ".join(vocab[min(rand(5000), rand(5000))] for _ in range(1 + rand(15)))
        lines.append(line)
        size += len(line) + 1
    with open(os.path.join(OUT, "words.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")

    records, size = [], 2
    while size < SIZE:
        i = len(records)
        r = {
            "active": rand(2) == 1,
            "id": i,
            "meta": None if rand(3) == 0 else {"rank": rand(1000), "zone": word()},
            "name": word() + " " + word() + (' "q"' if rand(10) == 0 else "") + ("\n" if rand(20) == 0 else ""),
            "score": rand(400000) / 4,
            "tags": [word() for _ in range(rand(4))],
        }
        records.append(r)
        size += len(json.dumps(r, separators=(",", ":"))) + 1
    with open(os.path.join(OUT, "data.json"), "w") as f:
        f.write(json.dumps(records, separators=(",", ":")))


if __name__ == "__main__":
    main()
