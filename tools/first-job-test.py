#!/usr/bin/env python3
"""Follow docs/first-job.md with a release archive, as a new user would:

    tools/first-job-test.py dist/kio-TARGET.tar.gz docs/first-job.md

Extracts the archive into a fresh directory and runs every ```console block of
the guide in order in one bash shell, from the archive's directory (the guide
puts `kio` on PATH). In a block, `$ cmd` is a command (a trailing `\\` continues
it) and the lines after it are its expected output (stdout and stderr), compared
exactly but for audit timings (`"ms":1.2`). Other blocks (the download) are not
run. CI runs this in a container without Rust (ci.yml, job first-job).
"""
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time


def steps(guide):
    """[(command, [expected lines])] from the guide's ```console blocks."""
    out, block, cont = [], False, False
    for line in open(guide).read().splitlines():
        if line.startswith("```"):
            block = line == "```console"
            continue
        if not block:
            continue
        if cont:
            out[-1][0] += "\n" + line
        elif line.startswith("$ "):
            out.append([line[2:], []])
        else:
            out[-1][1].append(line)
        cont = line.endswith("\\")
    return out


def norm(lines):
    return [re.sub(r'"ms":[0-9.]+', '"ms":_', l.rstrip()) for l in lines]


def main():
    archive, guide = map(os.path.abspath, sys.argv[1:3])
    for tool in ("cargo", "rustc"):
        if shutil.which(tool):
            print(f"note: {tool} is on PATH; the test is not on a clean machine")
    work = tempfile.mkdtemp()
    with tarfile.open(archive) as t:
        t.extractall(work, filter="data")
    (root,) = os.listdir(work)
    root = os.path.join(work, root)

    todo = steps(guide)
    mark = "@@step-end@@"
    script = "".join(f"{cmd}\necho {mark}\n" for cmd, _ in todo)
    start = time.time()
    run = subprocess.run(["bash", "-c", script + "kill %1 2>/dev/null; true"], cwd=root,
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=120)
    got = run.stdout.split(mark + "\n")
    failed = 0
    for i, (cmd, want) in enumerate(todo):
        have = got[i].splitlines() if i < len(got) else ["<no output: the shell stopped>"]
        if norm(have) != norm(want):
            failed += 1
            print(f"--- step {i + 1}: $ {cmd}\nwant:\n" + "\n".join(want) + "\nhave:\n" + "\n".join(have))
    print(f"{len(todo) - failed}/{len(todo)} commands as in the guide, {time.time() - start:.1f} s")
    shutil.rmtree(work)
    sys.exit(1 if failed else 0)


main()
