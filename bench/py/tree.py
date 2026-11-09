from dataclasses import dataclass


@dataclass
class Leaf:
    value: int


@dataclass
class Node:
    kids: list


@dataclass
class Stats:
    sum: int
    leaves: int
    depth: int


def build(d, v):
    if d == 0:
        return Leaf(v % 1000)
    return Node([build(d - 1, v * 2), build(d - 1, v * 2 + 1)])


def fold(t):
    match t:
        case Leaf(v):
            return Stats(v, 1, 0)
        case Node(kids):
            s = Stats(0, 0, 0)
            for k in kids:
                c = fold(k)
                s.sum += c.sum
                s.leaves += c.leaves
                s.depth = max(s.depth, c.depth + 1)
            return s


s = fold(build(20, 1))
print(s.sum * 1000 + s.leaves + s.depth)
