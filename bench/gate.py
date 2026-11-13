#!/usr/bin/env python3
"""The benchmark regression gate (PG1, docs/perf.md "Regression gate").

Runs every benchmark once under `kio run --json` and compares fuel and peak memory with
bench/baseline.json. Fuel is counted exactly, so it does not depend on the machine's load.

  bench/gate.py [--update] [--kio PATH] [--message TEXT]

--message is the commit message (CI passes the push's head commit, or a pull request's title and
body); `perf-ok: fib, tree` or `perf-ok: all` in it waives growth on those rows. Needs
bench/data (bench/gen_data.py). Exit 0 when every row passes, 1 otherwise.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

from run import BENCHES

BENCH = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(BENCH)
DATA = os.path.join(BENCH, "data")
BASELINE = os.path.join(BENCH, "baseline.json")
LIMITS = ["--fuel", str(10**15), "--timeout", "3600", "--memory", "4096"]
TOLERANCE = 0.02

# (row, program in bench/kio, needs data/); the empty program is the startup row.
ROWS = [("startup", "empty", False)] + [
    (name, "jsonb" if name == "json" else name, data is not None) for name, _, data in BENCHES
]


def measure(kio, build, prog, data):
    cmd = [kio, "run", os.path.join(build, prog + ".kio"), "--json"] + LIMITS
    if data:
        cmd += ["--dir", "data=" + DATA + ":ro"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    try:
        out = json.loads(r.stdout)
    except ValueError:
        sys.exit(f"{prog}: not JSON (exit {r.returncode}): {r.stdout[-500:]}{r.stderr[-500:]}")
    if not out["ok"]:
        sys.exit(f"{prog}: {out['errors']}")
    u = out["used"]
    return {"value": out["value"], "fuel": u["fuel"], "peak_memory": u["peak_memory"], "ms": u["ms"]}


def waived(message):
    rows = set()
    for m in re.finditer(r"perf-ok:[ \t]*(\w+(?:[ \t]*,[ \t]*\w+)*)", message):
        rows |= {w.strip() for w in m.group(1).split(",")}
    return rows


def pct(old, new):
    return (new - old) / old if old else (0.0 if new == old else float("inf"))


def cell(old, new):
    if old is None:
        return f"- -> {new}"
    return f"{old} -> {new} ({pct(old, new) * 100:+.1f}%)"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--update", action="store_true", help="rewrite bench/baseline.json")
    ap.add_argument("--kio", default=os.path.join(ROOT, "target/release/kio"))
    ap.add_argument("--message", default="", help="the commit message, for perf-ok:")
    a = ap.parse_args()
    if not os.path.exists(os.path.join(DATA, "words.txt")):
        sys.exit("bench/data missing: run bench/gen_data.py")
    progs = {f[:-4] for f in os.listdir(os.path.join(BENCH, "kio")) if f.endswith(".kio")}
    unrun = progs - {p for _, p, _ in ROWS} - {"json_kio"}
    if unrun:
        sys.exit(f"bench/kio/{sorted(unrun)[0]}.kio is not a row: add it to BENCHES in bench/run.py")
    version = subprocess.run([a.kio, "--version"], capture_output=True, text=True).stdout.strip()

    # As run.sh: json.kio beside jsonb.kio is the vendored json_kio.kio.
    build = tempfile.mkdtemp(prefix="kio-gate-")
    try:
        for f in os.listdir(os.path.join(BENCH, "kio")):
            shutil.copy(os.path.join(BENCH, "kio", f), build)
        shutil.copy(os.path.join(BENCH, "kio", "json_kio.kio"), os.path.join(build, "json.kio"))
        new = {row: measure(a.kio, build, prog, data) for row, prog, data in ROWS}
    finally:
        shutil.rmtree(build)

    if a.update:
        rows = {r: {k: v[k] for k in ("value", "fuel", "peak_memory")} for r, v in new.items()}
        with open(BASELINE, "w") as f:
            json.dump({"kio": version, "rows": rows}, f, indent=2)
            f.write("\n")
        print(f"wrote {os.path.relpath(BASELINE, ROOT)} ({version})")
        return 0

    with open(BASELINE) as f:
        base = json.load(f)
    old = base["rows"]
    ok_rows = waived(a.message)
    failures, notes = [], []
    print(f"baseline: {base['kio']}; this run: {version}")
    print(f"{'row':<13} {'fuel old -> new (%)':<44} {'peak old -> new (%)':<38} ms")
    for row in list(new) + [r for r in old if r not in new]:
        o, n = old.get(row), new.get(row)
        if n is None:
            print(f"{row:<13} missing from this run")
            failures.append(f"{row}: in the baseline, not run: run bench/gate.py --update")
            continue
        print(f"{row:<13} {cell(o and o['fuel'], n['fuel']):<44} "
              f"{cell(o and o['peak_memory'], n['peak_memory']):<38} {n['ms']:.0f}")
        if o is None:
            failures.append(f"{row}: not in the baseline: run bench/gate.py --update")
            continue
        if o["value"] != n["value"]:
            failures.append(f"{row}: printed {n['value']}, baseline {o['value']} (never waived)")
        for k in ("fuel", "peak_memory"):
            p = pct(o[k], n[k])
            if p > TOLERANCE and (row in ok_rows or "all" in ok_rows):
                notes.append(f"{row}: {k} +{p * 100:.1f}%: waived by perf-ok")
            elif p > TOLERANCE:
                failures.append(f"{row}: {k} +{p * 100:.1f}%: fix it, or waive with `perf-ok: {row}` in the commit message")
            elif p < -TOLERANCE:
                notes.append(f"{row}: {k} {p * 100:.1f}%: run bench/gate.py --update to lock it in")
    for n in notes:
        print("note: " + n)
    if failures:
        print("failed:")
        for f in failures:
            print("  " + f)
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
