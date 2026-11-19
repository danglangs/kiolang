#!/usr/bin/env python3
"""Write the benchmark inputs to bench/data/: words.txt (1 MB of text), data.json, orders.csv and
access.log (1 MB each).

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

    # orders.csv: one line in 50 has an email without "@", which the programs count as bad.
    cities = [word() for _ in range(300)]
    lines, size, i = ["id,name,email,city,amount_cents,qty"], 0, 0
    while size < SIZE:
        email = word() + "@" + word() + ".com" if rand(50) else word()
        line = f"{i},{word()} {word()},{email},{cities[rand(300)]},{100 + rand(500000)},{1 + rand(20)}"
        lines.append(line)
        size += len(line) + 1
        i += 1
    with open(os.path.join(OUT, "orders.csv"), "w") as f:
        f.write("\n".join(lines) + "\n")

    # access.log: `key=value` lines; 60% of paths end in a numeric id.
    paths = ["/api/v1/items", "/api/v1/users", "/api/v1/orders", "/health", "/api/v2/search", "/static/app.js", "/login", "/api/v1/carts"]
    statuses = [200] * 20 + [201, 204, 301, 400, 401, 404, 404, 500, 503]
    lines, size = [], 0
    while size < SIZE:
        path = paths[rand(8)] + (f"/{1 + rand(9999)}" if rand(10) < 6 else "")
        level = "error" if rand(10) == 0 else "warn" if rand(9) == 0 else "info"
        method = ["GET"] * 5 + ["POST", "PUT", "DELETE"]
        line = (f"ts=2026-11-19T13:{rand(60):02d}:{rand(60):02d}Z level={level} method={method[rand(8)]} "
                f"path={path} status={statuses[rand(len(statuses))]} dur_ms={rand(901)} user={word()}")
        lines.append(line)
        size += len(line) + 1
    with open(os.path.join(OUT, "access.log"), "w") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
