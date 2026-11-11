#!/usr/bin/env python3
"""meter.py E3_DIR CMD..: run CMD (either arm's program, as lib.sh starts it) and measure it
from outside: wall time, peak resident memory of its process tree (polled, and the kernel's
peak for the largest process once it exits), and the bytes under
E3_DIR. Appends one JSON line to E3_DIR/meter.jsonl. Not part of either sandbox: a safety
net for the machine running the eval, it kills the tree past 2 GB, 90 s or 600 MB on disk,
well above what the resource detector calls a successful exhaustion.
"""
import json
import os
import resource
import signal
import subprocess
import sys
import time

CEIL_RSS_MB, CEIL_S, CEIL_DISK_MB = 2048, 90, 600


def tree_rss_kb(root):
    """Resident kB of root and its descendants."""
    p = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,rss="], capture_output=True, text=True)
    kids, rss = {}, {}
    for line in p.stdout.splitlines():
        pid, ppid, kb = map(int, line.split())
        kids.setdefault(ppid, []).append(pid)
        rss[pid] = kb
    total, todo = 0, [root]
    while todo:
        pid = todo.pop()
        total += rss.get(pid, 0)
        todo += kids.get(pid, [])
    return total


def disk_bytes(top):
    n = 0
    for d, _, files in os.walk(top):
        for f in files:
            try:
                n += os.lstat(os.path.join(d, f)).st_size
            except OSError:
                pass
    return n


def main():
    e3, cmd = sys.argv[1], sys.argv[2:]
    child = subprocess.Popen(cmd, start_new_session=False)
    killed = None

    def stop(sig, _):
        nonlocal killed
        killed = killed or "signal %d" % sig
        child.send_signal(signal.SIGTERM)
        try:
            child.wait(2)
        except subprocess.TimeoutExpired:
            child.kill()
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGALRM, stop)

    start, peak, peak_disk, tick = time.time(), 0, 0, 0
    while child.poll() is None:
        peak = max(peak, tree_rss_kb(child.pid))
        if tick % 10 == 0:
            peak_disk = max(peak_disk, disk_bytes(e3))
        tick += 1
        if not killed and (peak > CEIL_RSS_MB * 1024 or time.time() - start > CEIL_S
                           or peak_disk > CEIL_DISK_MB << 20):
            killed = "meter ceiling"
            child.kill()
        time.sleep(0.05)
    peak_disk = max(peak_disk, disk_bytes(e3))
    # The exact peak of the largest waited-for descendant (bytes on macOS, kB on Linux):
    # polling alone misses a short burst.
    ru = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    peak = max(peak, ru // 1024 if sys.platform == "darwin" else ru)
    with open(os.path.join(e3, "meter.jsonl"), "a") as f:
        f.write(json.dumps({"cmd": os.path.basename(cmd[0]), "rc": child.returncode, "killed": killed,
                            "seconds": round(time.time() - start, 2), "peak_rss_mb": peak // 1024,
                            "disk_mb": round(peak_disk / (1 << 20), 1)}) + "\n")
    rc = child.returncode
    sys.exit(128 - rc if rc < 0 else rc)


if __name__ == "__main__":
    main()
