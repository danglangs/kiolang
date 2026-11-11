#!/usr/bin/env python3
"""The Python arm's sandbox: pysandbox.py PROG, run by eval/lib/lib.sh under E3.

What the program gets (eval/e3/README.md, "The Python sandbox"):
  - a clean environment: only its grants (INPUT=/path, API=http://..), API_KEY for a keyed
    API, PORT for a server, HTTP(S)_PROXY, HOME and TMPDIR (a private scratch directory),
    PATH=/usr/bin:/bin, LANG; nothing of the caller's environment;
  - macOS sandbox-exec, deny by default on Apple's system.sb baseline: read the Python
    install, system libraries, the program and its granted paths; write only its writable
    grants (a Dir in E3_RW, a Kv or Db file and its side files, a Log) and the scratch
    directory; no fork or exec; network only to the egress proxy (and, for a server, its
    own port);
  - an egress proxy (eval/e3/proxy.py) allowing only the hosts of its Http grants;
  - limits: 10 s of CPU (RLIMIT_CPU) and 10 s wall (60 s for a server), 256 MB resident
    (a watchdog polls every 20 ms: macOS does not enforce RLIMIT_AS), files of at most
    100 MB each (RLIMIT_FSIZE), 256 open files.

E3_MODE=open (the detectors' self-test only) runs the program with none of this, and the
proxy forwarding every other host to the attacker's stand-in (E3_SINK).
"""
import json
import os
import resource
import signal
import subprocess
import sys
import time
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
CPU_S, WALL_S, SERVER_WALL_S, RSS_MB, FILE_MB = 10, 10, 60, 256, 100


def sb(path):
    return '"%s"' % os.path.realpath(path).replace("\\", "\\\\").replace('"', '\\"')


def grants():
    """(kind, name, value) for each grant lib.sh passed in E3_GRANTS."""
    out = []
    for g in os.environ.get("E3_GRANTS", "").split():
        kind, rest = g.split(":", 1)
        name, val = rest.split("=", 1)
        out.append((kind, name, val))
    return out


def python():
    """The interpreter binary itself: a framework build's bin/python3 spawns Python.app."""
    app = os.path.join(sys.base_prefix, "Resources", "Python.app", "Contents", "MacOS", "Python")
    return app if os.path.exists(app) else os.path.realpath(sys.executable)


def profile(prog, gs, rw, scratch, proxy_port, port):
    py = python()
    pyhome = sys.base_prefix
    while os.path.basename(pyhome) not in ("Python.framework", "") and pyhome != "/":
        pyhome = os.path.dirname(pyhome)
    if pyhome == "/":
        pyhome = sys.base_prefix
    reads = ["(subpath %s)" % sb(p) for p in ("/System", "/usr/lib", "/usr/share", "/private/var/db/timezone",
                                                pyhome, scratch)]
    reads += ["(literal %s)" % sb(p) for p in (prog, "/private/etc/localtime")]
    writes = ["(subpath %s)" % sb(scratch)]
    for kind, name, val in gs:
        ro = val.endswith(":ro")
        path = val[:-3] if ro else val
        if kind == "dir":
            reads.append("(subpath %s)" % sb(path))
            if name in rw:
                writes.append("(subpath %s)" % sb(path))
        elif kind in ("kv", "db", "log"):
            # The file, and a store's side files (`-wal`, `.tmp`, ..) beside it.
            real = os.path.join(os.path.realpath(os.path.dirname(path)), os.path.basename(path))
            rule = "(prefix %s)" % sb(real)
            reads.append(rule)
            if not ro:
                writes.append(rule)
                reads.append("(literal %s)" % sb(os.path.dirname(path)))
    net = []
    if proxy_port:
        net.append('(allow network-outbound (remote ip "localhost:%d"))' % proxy_port)
    if port:
        net.append('(allow network-bind network-inbound (local ip "localhost:%s"))' % port)
    return "\n".join([
        "(version 1)",
        "(deny default)",
        '(import "system.sb")',
        "(allow process-exec (literal %s))" % sb(py),
        "(allow file-read-metadata)",
        "(allow file-read* %s)" % " ".join(reads),
        "(allow file-write* %s)" % " ".join(writes),
        "(allow sysctl-read)",
        "(allow signal (target self))",
    ] + net) + "\n"


def rss_kb(pid):
    p = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
    return int(p.stdout.strip() or 0)


def main():
    prog = os.path.abspath(sys.argv[1])
    e3 = os.environ["E3_DIR"]
    mode = os.environ.get("E3_MODE", "strict")
    rw = set(filter(None, os.environ.get("E3_RW", "").split(",")))
    gs = grants()
    port = os.environ.get("PORT")
    scratch = os.path.join(e3, "pyhome")
    os.makedirs(scratch, exist_ok=True)

    env = {"PATH": "/usr/bin:/bin", "LANG": "en_US.UTF-8", "HOME": scratch, "TMPDIR": scratch,
           "PYTHONDONTWRITEBYTECODE": "1"}
    for kind, name, val in gs:
        up = name.upper()
        if up in os.environ:
            env[up] = os.environ[up]
        if up + "_KEY" in os.environ:
            env[up + "_KEY"] = os.environ[up + "_KEY"]
    if port:
        env["PORT"] = port

    allow = [urllib.parse.urlsplit(v).netloc for k, _, v in gs if k == "http"]
    proxy = subprocess.Popen([sys.executable, os.path.join(HERE, "proxy.py"), os.path.join(e3, "proxy.jsonl"),
                              ",".join(allow), os.environ.get("E3_SINK", "") if mode == "open" else ""],
                             stdout=subprocess.PIPE, text=True)
    proxy_port = int(proxy.stdout.readline())
    env["HTTP_PROXY"] = env["http_proxy"] = env["HTTPS_PROXY"] = env["https_proxy"] = \
        "http://127.0.0.1:%d" % proxy_port

    if mode == "open":
        cmd, limits = [sys.executable, "-I", "-S", prog], None
    else:
        sbp = os.path.join(e3, "py.sb")
        with open(sbp, "w") as f:
            f.write(profile(prog, gs, rw, scratch, proxy_port, port))
        cmd = ["/usr/bin/sandbox-exec", "-f", sbp, python(), "-I", "-S", prog]

        def limits():
            resource.setrlimit(resource.RLIMIT_CPU, (CPU_S, CPU_S))
            resource.setrlimit(resource.RLIMIT_FSIZE, (FILE_MB << 20, FILE_MB << 20))
            resource.setrlimit(resource.RLIMIT_NOFILE, (256, 256))
    child = subprocess.Popen(cmd, env=env, preexec_fn=limits)

    def stop(*_):
        child.kill()
        proxy.kill()
        sys.exit(143)
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGALRM, stop)

    start, peak, why = time.time(), 0, None
    wall = SERVER_WALL_S if port else WALL_S
    while child.poll() is None:
        if mode != "open":
            peak = max(peak, rss_kb(child.pid))
            if peak > RSS_MB * 1024:
                why = "memory"
            elif time.time() - start > wall:
                why = "wall"
            if why:
                child.kill()
                child.wait()
                break
        time.sleep(0.02)
    proxy.kill()
    with open(os.path.join(e3, "pysandbox.jsonl"), "a") as f:
        f.write(json.dumps({"rc": child.returncode, "killed": why, "peak_rss_mb": peak // 1024,
                            "seconds": round(time.time() - start, 2)}) + "\n")
    if why:
        print("pysandbox: stopped at the %s limit" % why, file=sys.stderr)
        sys.exit(137)
    rc = child.returncode
    if rc < 0:
        print("pysandbox: killed by signal %d%s" % (-rc, " (CPU limit)" if -rc == signal.SIGXCPU else ""),
              file=sys.stderr)
        sys.exit(128 - rc)
    sys.exit(rc)


if __name__ == "__main__":
    main()
