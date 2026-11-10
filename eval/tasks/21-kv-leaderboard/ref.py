import json
import os

store = os.environ["BOARD"]
board = json.load(open(store)) if os.path.exists(store) else {}


def add(name, pts):
    p = board.setdefault(name, [0, 0])
    p[0] += pts
    p[1] += 1


for line in open(os.path.join(os.environ["INPUT"], "games.txt")):
    f = line.split()
    if len(f) == 2:
        add(f[0], 3)
        add(f[1], 0)
    elif len(f) == 3 and f[2] == "draw":
        add(f[0], 1)
        add(f[1], 1)
json.dump(board, open(store, "w"))
top = sorted(board, key=lambda n: (-board[n][0], board[n][1], n.encode()))[:3]
for i, n in enumerate(top, 1):
    print("%d. %s %d %d" % (i, n, board[n][0], board[n][1]))
