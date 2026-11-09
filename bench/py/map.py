import sys


def main():
    with open(sys.argv[1]) as f:
        text = f.read()
    counts = {}
    total = 0
    for line in text.splitlines():
        for w in line.split(" "):
            if w:
                counts[w] = counts.get(w, 0) + 1
                total += 1
    most = max(counts.values(), default=0)
    print(len(counts) * 1_000_000_000 + most * 1_000_000 + total)


main()
