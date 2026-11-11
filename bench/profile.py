#!/usr/bin/env python3
"""Profile one compiled kio run with macOS `sample`, naming JIT frames.

    python3 bench/profile.py [--secs N] [--delay MS] prog.kio [kio run flags...]

`sample` cannot name Wasmtime's JIT code. This runs the program once to fill a private compile
cache, runs it again under `sample`, finds the executable region that holds the cached native
code (`vmmap`), and names each address from the cache entry's ELF symbol table (`objdump -t`).
Prints self samples per function (where time is spent) and inclusive samples (what it is under).
"""
import bisect
import collections
import glob
import os
import re
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KIO = os.environ.get("KIO", os.path.join(ROOT, "target/release/kio"))
INTERP = os.path.join(ROOT, "bench/target/release/kio-interp")
TARGET = os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target"))
# codegen/build.rs leaves the runtime it embedded in its OUT_DIR; the newest is the release kio's.
RUNTIME = max(glob.glob(os.path.join(TARGET, "release/build/codegen-*/out/runtime.wasm")),
              key=os.path.getmtime, default=None)


def leb(d, i):
    n = shift = 0
    while True:
        b = d[i]
        i += 1
        n |= (b & 0x7F) << shift
        shift += 7
        if b < 0x80:
            return n, i


def runtime_funcs():
    """The runtime module's function count, imports included: the program's first function index."""
    if RUNTIME is None:
        sys.exit("no runtime.wasm under target/release/build: run cargo build --release -p kio")
    d, i, n = open(RUNTIME, "rb").read(), 8, 0
    while i < len(d):
        sid, size, i = d[i], *leb(d, i + 1)
        end = i + size
        if sid == 2:  # imports: count the functions
            count, j = leb(d, i)
            for _ in range(count):
                for _ in range(2):
                    ln, j = leb(d, j)
                    j += ln
                kind = d[j]
                j += 1
                if kind == 0:
                    n += 1
                    _, j = leb(d, j)
                else:
                    break  # the runtime imports only functions before anything else
        elif sid == 3:
            n += leb(d, i)[0]
        i = end
    return n


def kio_names(prog):
    """`function[N]` -> the kio function codegen emitted there (bench/interp's `--names`)."""
    if not os.path.exists(INTERP):
        return {}
    out = subprocess.run([INTERP, "--names", prog], capture_output=True, text=True).stdout.split()
    base = runtime_funcs()
    return {base + k: "kio " + n for k, n in enumerate(out)}


def demangle(sym):
    """`wasm[0]::function[45]::_RNv..7runtime4coll10map_insert` -> `runtime::coll::map_insert`."""
    m = re.match(r"wasm\[\d+\]::function\[(\d+)\]::?(.*)", sym)
    if not m:
        return sym
    idx, name = m.groups()
    if not name.startswith("_R"):
        return name or f"function[{idx}]"
    name = re.sub(r"Cs[0-9A-Za-z]+_", "", name)
    parts, i = [], 0
    while i < len(name):
        d = re.match(r"\d+", name[i:])
        if d:
            n, j = int(d.group()), i + len(d.group())
            if name[j : j + 1] == "_":  # an identifier starting with a digit or `_` gets a `_` separator
                j += 1
            ident = name[j : j + n]
            if len(ident) == n and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", ident):
                if not re.fullmatch(r"Cs[0-9A-Za-z_]*|[0-9a-f]{16}", ident):
                    parts.append(ident)
                i = j + n
                continue
        i += 1
    keep = [p for p in parts if p not in ("llvm",)]
    return "::".join(keep[-3:]) if keep else name


def symbols(cwasm, names):
    out = subprocess.run(["objdump", "-t", cwasm], capture_output=True, text=True).stdout
    syms = []
    for line in out.splitlines():
        f = line.split()
        if len(f) >= 6 and f[2] == "F" and f[3] == ".text":
            sym = " ".join(f[5:])
            m = re.match(r"wasm\[\d+\]::function\[(\d+)\]$", sym)
            label = names.get(int(m.group(1)), "kio fn-value wrapper " + m.group(1)) if m else demangle(sym)
            syms.append((int(f[0], 16), int(f[4], 16), label))
    text = subprocess.run(["objdump", "-h", cwasm], capture_output=True, text=True).stdout
    size = int(re.search(r"\.text\s+([0-9a-f]+)", text).group(1), 16)
    return sorted(syms), size


def main():
    args = sys.argv[1:]
    secs, delay = 3, 0.0
    while args[0] in ("--secs", "--delay"):
        if args[0] == "--secs":
            secs = int(args[1])
        else:
            delay = int(args[1]) / 1000
        args = args[2:]
    cache = tempfile.mkdtemp(prefix="kio-prof-")
    env = dict(os.environ, XDG_CACHE_HOME=cache)
    cmd = [KIO, "run"] + args
    subprocess.run(cmd, env=env, check=True, capture_output=True)
    t = time.perf_counter()
    subprocess.run(cmd, env=env, check=True, capture_output=True)
    run_secs = time.perf_counter() - t
    entries = os.listdir(os.path.join(cache, "kio"))
    prog = next(a for a in args if a.endswith(".kio"))
    syms, size = symbols(os.path.join(cache, "kio", entries[0]), kio_names(prog))
    out = os.path.join(cache, "sample.txt")
    p = subprocess.Popen(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(delay)
    sampler = subprocess.Popen(["sample", str(p.pid), str(secs), "1", "-file", out], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # The native code's region: anonymous, executable, the size of `.text` in 16 KB pages.
    pages = -(-size // 0x4000) * 0x4000
    base = None
    while base is None and p.poll() is None:
        vm = subprocess.run(["vmmap", str(p.pid)], capture_output=True, text=True).stdout
        for line in vm.splitlines():
            m = re.match(r"VM_ALLOCATE\s+([0-9a-f]+)-([0-9a-f]+)\s.*r-x/", line)
            if m and int(m.group(2), 16) - int(m.group(1), 16) == pages:
                base = int(m.group(1), 16)
    p.wait()
    sampler.wait()
    starts = [s[0] for s in syms]

    def name(frame):
        a = re.search(r"\[0x([0-9a-f]+)\]", frame)
        if "???" in frame and a and base is not None:
            off = int(a.group(1), 16) - base
            k = bisect.bisect_right(starts, off) - 1
            if k >= 0 and off < syms[k][0] + syms[k][1]:
                return "jit " + syms[k][2]
            return "jit ?"
        m = re.match(r"\s*(\S+)\s+\(in ([^)]+)\)", frame)
        if not m:
            return frame.strip()
        f = m.group(1)
        f = demangle("wasm[0]::function[0]::" + f) if f.startswith("_R") else f
        return f"{f} ({m.group(2)})"

    # The call graph: "<indent><count> <frame>" lines, one node per line; a node's self samples
    # are its count minus its children's.
    nodes = []  # [depth, count, name, address, parent index]
    stack, in_graph = [], False
    for line in open(out, errors="replace"):
        if line.startswith("Call graph:"):
            in_graph = True
            continue
        if in_graph and not line.strip():
            break
        m = in_graph and re.match(r"^([\s+!:|]*)(\d+) (.*)$", line.rstrip("\n"))
        if not m:
            continue
        depth = len(m.group(1))
        while stack and nodes[stack[-1]][0] >= depth:
            stack.pop()
        a = re.search(r"\[0x([0-9a-f]+)\]", m.group(3))
        nodes.append([depth, int(m.group(2)), name(m.group(3)), a and a.group(1), stack[-1] if stack else None])
        stack.append(len(nodes) - 1)
    self_of = [n[1] for n in nodes]
    for n in nodes:
        if n[4] is not None:
            self_of[n[4]] -= n[1]

    def jit_root(i):
        """The outermost compiled frame above node i, or None."""
        root = None
        while i is not None:
            if nodes[i][2].startswith("jit"):
                root = i
            i = nodes[i][4]
        return root

    # `sample` stops the process for about 0.3 s as it attaches, some tens of ms into the run,
    # and records all of it at the one instruction the thread was stopped on. Drop that
    # instruction's samples: the profile is of the run after the attach.
    roots = {jit_root(i) for i in range(len(nodes))} - {None}
    total = sum(nodes[r][1] for r in roots)
    under = [i for i in range(len(nodes)) if jit_root(i) is not None]
    stall = max(under, key=lambda i: self_of[i]) if under else None
    if stall is not None and total > 1000 * run_secs:
        cut = self_of[stall]
        self_of[stall] = 0
        i = stall
        while i is not None:
            nodes[i][1] -= cut
            i = nodes[i][4]
        total -= cut
        print(f"(dropped {cut} samples at one instruction in {nodes[stall][2]}: the attach stall)")
    self_n, incl = collections.Counter(), collections.Counter()
    for i, n in enumerate(nodes):
        if jit_root(i) is None:
            continue
        self_n[n[2]] += self_of[i]
        # Inclusive: count a frame once per stack, at its outermost occurrence.
        j, outer = n[4], True
        while j is not None:
            if nodes[j][2] == n[2]:
                outer = False
            j = nodes[j][4]
        if outer:
            incl[n[2]] += n[1]
    print(f"{total} samples in compiled code and below (1 ms each; unsampled run {1000 * run_secs:.0f} ms)")
    print("self:")
    for f, n in self_n.most_common(15):
        if n > 0 and total:
            print(f"  {100 * n / total:5.1f}%  {f}")
    print("inclusive:")
    for f, n in incl.most_common(25):
        if total:
            print(f"  {100 * n / total:5.1f}%  {f}")


if __name__ == "__main__":
    main()
