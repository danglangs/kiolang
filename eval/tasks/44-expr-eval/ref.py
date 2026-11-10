import os
import re

LO, HI = -2 ** 63, 2 ** 63 - 1
FUNCS = {"min": 2, "max": 2, "abs": 1}


class Fail(Exception):
    pass


def tokenize(line):
    toks, i = [], 0
    while i < len(line):
        if line[i] == " ":
            i += 1
            continue
        m = re.match(r"[0-9]+|[A-Za-z_][A-Za-z0-9_]*|[-+*/%^(),=]", line[i:])
        if not m or not m.group().isascii():
            raise Fail("syntax error")
        toks.append(m.group())
        i += len(m.group())
    return toks


class Parser:
    def __init__(self, toks):
        self.toks, self.i = toks, 0

    def peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else None

    def take(self, want=None):
        t = self.peek()
        if t is None or (want is not None and t != want):
            raise Fail("syntax error")
        self.i += 1
        return t

    def expr(self):
        e = self.term()
        while self.peek() in ("+", "-"):
            e = (self.take(), e, self.term())
        return e

    def term(self):
        e = self.factor()
        while self.peek() in ("*", "/", "%"):
            e = (self.take(), e, self.factor())
        return e

    def factor(self):
        if self.peek() == "-":
            self.take()
            return ("neg", self.factor())
        return self.power()

    def power(self):
        a = self.atom()
        if self.peek() == "^":
            self.take()
            return ("^", a, self.factor())
        return a

    def atom(self):
        t = self.take()
        if t.isdigit():
            return ("int", t)
        if re.fullmatch(r"[A-Za-z_]\w*", t):
            if self.peek() == "(":
                self.take()
                args = [self.expr()]
                while self.peek() == ",":
                    self.take()
                    args.append(self.expr())
                self.take(")")
                return ("call", t, args)
            return ("var", t)
        if t == "(":
            e = self.expr()
            self.take(")")
            return e
        raise Fail("syntax error")


def fit(v):
    if not LO <= v <= HI:
        raise Fail("overflow")
    return v


def ev(e, env):
    op = e[0]
    if op == "int":
        return fit(int(e[1]))
    if op == "var":
        if e[1] not in env:
            raise Fail("unknown variable " + e[1])
        return env[e[1]]
    if op == "neg":
        return fit(-ev(e[1], env))
    if op == "call":
        if e[1] not in FUNCS:
            raise Fail("unknown function " + e[1])
        if len(e[2]) != FUNCS[e[1]]:
            raise Fail("wrong number of arguments to " + e[1])
        args = [ev(a, env) for a in e[2]]
        return fit({"min": min, "max": max, "abs": abs}[e[1]](*args))
    a, b = ev(e[1], env), ev(e[2], env)
    if op in "/%" and b == 0:
        raise Fail("division by zero")
    if op == "^" and b < 0:
        raise Fail("negative exponent")
    if op == "^":
        r = 1
        for _ in range(b):
            r = fit(r * a)
            if r in (0, 1) and a in (0, 1):
                break
        return r
    if op in "/%":
        q = abs(a) // abs(b) * (1 if (a < 0) == (b < 0) else -1)
        return fit(q if op == "/" else a - b * q)
    return fit({"+": a + b, "-": a - b, "*": a * b}[op])


env, out = {}, []
lines = open(os.path.join(os.environ["INPUT"], "program.txt"), encoding="utf-8").read().split("\n")
for n, line in enumerate(lines, start=1):
    if line.strip() == "":
        continue
    try:
        toks = tokenize(line)
        name = None
        if toks and toks[0] == "let":
            if len(toks) < 3 or not re.fullmatch(r"[A-Za-z_]\w*", toks[1]) or toks[2] != "=":
                raise Fail("syntax error")
            name, toks = toks[1], toks[3:]
        p = Parser(toks)
        e = p.expr()
        if p.peek() is not None:
            raise Fail("syntax error")
        v = ev(e, env)
        if name:
            env[name] = v
            out.append("%d: %s = %d" % (n, name, v))
        else:
            out.append("%d: %d" % (n, v))
    except Fail as f:
        out.append("%d: error: %s" % (n, f))
print("\n".join(out))
