"""C24: --listen per request, kio against its Rust (hyper) twins, then the in-process probe, then
(Linux) a perf profile of the kio server split into phases. Called by run.sh, which builds the
binaries and passes them in the environment; writes OUT/report.md and the raw results beside it.

Environment: KIO, KIO_FP (frame-pointer build, for perf; optional), TWINS (dir with `hello` and
`bookings`), PROBE, OHA, OUT, ROOT, and the counts ROUNDS, N, WARM, CONC, PROBE_N, PROBE_ROUNDS,
PERF_SECS."""
import json, os, platform, re, socket, statistics, subprocess, sys, threading, time, urllib.request

E = os.environ
HERE = os.path.dirname(os.path.abspath(__file__))
APPS = os.path.join(HERE, "apps")
OUT, ROOT = E["OUT"], E["ROOT"]
KIO, TWINS, PROBE, OHA = E["KIO"], E["TWINS"], E["PROBE"], E["OHA"]
ROUNDS, N, WARM, CONC = (int(E.get(k, d)) for k, d in [("ROUNDS", 5), ("N", 50000), ("WARM", 5000), ("CONC", 12)])
PROBE_N, PROBE_ROUNDS, PERF_SECS = int(E.get("PROBE_N", 20000)), int(E.get("PROBE_ROUNDS", 3)), int(E.get("PERF_SECS", 10))
LINUX = sys.platform.startswith("linux")
NPROC = os.cpu_count() or 1
BODY = os.path.join(APPS, "bookings", "body.json")

# route: (kio cwd, kio args, path, POST body or None, expected status)
ROUTES = {
    "hello": (os.path.join(APPS, "hello"), ["app.kio", "--dir", "public=public"], "/hello/kio", None, 200),
    "bookings": (os.path.join(APPS, "bookings"), ["app.kio"], "/bookings", BODY, 201),
}


def log(*a):
    print(*a, file=sys.stderr, flush=True)
    with open(os.path.join(OUT, "log.txt"), "a") as f:
        print(*a, file=f)


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def argv(route, side, addr, kio=KIO):
    cwd, args, _, _, _ = ROUTES[route]
    if side == "kio":
        return cwd, [kio, "run"] + args + ["--listen", addr]
    return cwd, [os.path.join(TWINS, route), addr]


def request(url, body):
    data = open(body, "rb").read() if body else None
    req = urllib.request.Request(url, data=data, headers={"content-type": "application/json"} if body else {})
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def start(route, side, kio=KIO):
    addr = f"127.0.0.1:{free_port()}"
    cwd, cmd = argv(route, side, addr, kio)
    proc = subprocess.Popen(cmd, cwd=cwd, stdout=subprocess.DEVNULL, stderr=open(os.path.join(OUT, f"{side}-{route}.stderr"), "a"))
    for _ in range(600):
        if proc.poll() is not None:
            raise SystemExit(f"{side} {route} exited with {proc.returncode}; see {OUT}/{side}-{route}.stderr")
        try:
            socket.create_connection(("127.0.0.1", int(addr.split(":")[1])), timeout=0.2).close()
            break
        except OSError:
            time.sleep(0.05)
    else:
        raise SystemExit(f"{side} {route} did not start")
    return proc, addr


def cpu_secs(pid):
    """(user, system) CPU seconds of a process."""
    if LINUX:
        f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        t = os.sysconf("SC_CLK_TCK")
        return int(f[11]) / t, int(f[12]) / t
    o = subprocess.run(["ps", "-o", "utime=,stime=", "-p", str(pid)], capture_output=True, text=True).stdout.split()

    def secs(s):
        v = 0.0
        for x in s.split(":"):
            v = v * 60 + float(x)
        return v

    return secs(o[0]), secs(o[1])


def peak_rss_mb(pid):
    """Linux: VmHWM. macOS: RSS now (no peak without privileges)."""
    if LINUX:
        m = re.search(r"VmHWM:\s+(\d+) kB", open(f"/proc/{pid}/status").read())
        return int(m.group(1)) / 1024
    return int(subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout) / 1024


def oha(url, n, body, extra=()):
    cmd = [OHA, "-n", str(n), "-c", str(CONC), "--no-tui", "--output-format", "json", *extra]
    if body:
        cmd += ["-m", "POST", "-D", body, "-T", "application/json"]
    r = subprocess.run(cmd + [url], capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"oha failed: {r.stderr}")
    return json.loads(r.stdout)


def bad(d, status):
    codes = d.get("statusCodeDistribution", {})
    return sum(v for k, v in codes.items() if int(k) != status) + sum(d.get("errorDistribution", {}).values())


class LoadSampler(threading.Thread):
    def __init__(self):
        super().__init__(daemon=True)
        self.peak, self.stop = 0.0, threading.Event()

    def run(self):
        while not self.stop.is_set():
            self.peak = max(self.peak, os.getloadavg()[0])
            self.stop.wait(0.5)


def one(route, side):
    _, _, path, body, status = ROUTES[route]
    proc, addr = start(route, side)
    try:
        url = f"http://{addr}{path}"
        got, resp = request(url, body)
        if got != status:
            raise SystemExit(f"{side} {route}: status {got}, expected {status}: {resp[:200]!r}")
        oha(url, WARM, body)
        load0 = os.getloadavg()[0]
        sampler = LoadSampler()
        sampler.start()
        (u0, s0), t0 = cpu_secs(proc.pid), time.time()
        d = oha(url, N, body)
        (u1, s1), wall = cpu_secs(proc.pid), time.time() - t0
        sampler.stop.set()
        sampler.join()
        lat = d["latencyPercentiles"]
        return {
            "route": route, "side": side, "n": N,
            "cpu_us": ((u1 - u0) + (s1 - s0)) / N * 1e6, "sys_us": (s1 - s0) / N * 1e6,
            "rps": d["summary"]["requestsPerSec"], "p50_us": lat["p50"] * 1e6, "p99_us": lat["p99"] * 1e6,
            "rss_mb": peak_rss_mb(proc.pid), "load_before": load0, "load_during": max(sampler.peak, os.getloadavg()[0]),
            "bad": bad(d, status), "wall_s": wall, "body": resp.decode(errors="replace"),
        }
    finally:
        proc.terminate()
        proc.wait()


def probe(route, threads):
    cwd, _, _, body, _ = ROUTES[route]
    files = ["app.kio", "web.kio"] if route == "hello" else ["app.kio"]
    env = dict(E, PUBLIC="public") if route == "hello" else dict(E, BODY=body)
    load0 = os.getloadavg()[0]
    o = subprocess.run([PROBE, route, str(threads), str(PROBE_N)] + files, cwd=cwd, env=env, capture_output=True, text=True)
    if o.returncode != 0:
        raise SystemExit(f"probe {route} {threads}: {o.stderr}")
    r = {k: float(v) for k, v in re.findall(r"(\w+)=([\d.]+)", o.stdout)}
    r.update(route=route, load_before=load0, load_after=os.getloadavg()[0])
    return r


# Perf: phases from the stacks of `perf script` (leaf first). Compiled code has no symbols:
# Wasmtime's code memory is anonymous, so its frames have no symbol or file. A frame is
# "sym (dso)"; the request path is told apart by kio's and Wasmtime's frames (handle_sending
# may be inlined into listen's closure).
JIT = re.compile(r"\((\[unknown\]|//anon|\[anon[^)]*\]|memfd:[^)]*|/tmp/perf-\d+\.map)\)$")
KERNEL = "[kernel.kallsyms]"


def phase(frames):
    j = " | ".join(frames)
    jit = [i for i, f in enumerate(frames) if JIT.search(f)]
    if not jit and not any(m in j for m in ("kio_host::compiled", "wasmtime::", "kio_host::new_store", "kio_host::push_grants", "kio_host::web_grants")):
        return "net: hyper, tokio, sockets, hand-off"
    if "compiled::instantiate" in j or "InstancePre" in j:
        if "MemoryImage" in j or "cow::" in j:
            return "instantiate: memory image"
        return "instantiate: other"
    if "drop_in_place<wasmtime" in j or "clear_and_remain_ready" in j or "deallocate" in j or "decommit" in j.lower():
        return "store drop: memory reset, free"
    if jit:
        user = [f for f in frames[: jit[0]] if KERNEL not in f]
        if user:
            return "host bindings (called from guest)"
        if jit[0] > 0:
            return "handler: kernel (page faults) under compiled code"
        return "handler: compiled code"
    if "compiled::call" in j or "call_run" in j:
        return "call: entry, lift, lower"
    return "host: store, grants, other"


def parse_script(lines):
    """Samples of `perf script`: a header line, then indented frames "ip sym (dso)", leaf first.
    Phase counts and folded stacks (root first, for flame graphs)."""
    counts, folded, frames = {}, {}, []

    def flush():
        if frames:
            k = phase(frames)
            counts[k] = counts.get(k, 0) + 1
            f = ";".join(reversed([re.sub(r"\+0x[0-9a-f]+", "", x) for x in frames]))
            folded[f] = folded.get(f, 0) + 1
        frames.clear()

    for line in lines:
        s = line.strip()
        if s and line[0].isspace():
            frames.append(s.split(None, 1)[1] if " " in s else s)
        else:
            flush()
    flush()
    return counts, folded


def perf_split(route):
    if not (LINUX and E.get("KIO_FP")):
        return None
    _, _, path, body, _ = ROUTES[route]
    proc, addr = start(route, "kio", kio=E["KIO_FP"])
    data = os.path.join(OUT, f"perf-{route}.data")
    try:
        url = f"http://{addr}{path}"
        oha(url, WARM, body)
        load = subprocess.Popen([OHA, "-z", f"{PERF_SECS + 4}s", "-c", str(CONC), "--no-tui", "--output-format", "quiet"]
                                + (["-m", "POST", "-D", body, "-T", "application/json"] if body else []) + [url])
        time.sleep(1)
        rec = subprocess.run(["perf", "record", "-F", "999", "--call-graph", "fp", "-p", str(proc.pid), "-o", data,
                              "--", "sleep", str(PERF_SECS)], capture_output=True, text=True)
        load.wait()
        if rec.returncode != 0:
            return {"error": rec.stderr[-2000:]}
    finally:
        proc.terminate()
        proc.wait()
    rep = subprocess.run(["perf", "report", "-i", data, "--no-children", "--sort", "dso,sym", "--stdio"],
                         capture_output=True, text=True).stdout
    open(os.path.join(OUT, f"perf-{route}-report.txt"), "w").write("\n".join(rep.splitlines()[:400]))
    script = subprocess.Popen(["perf", "script", "-i", data, "-F", "comm,tid,ip,sym,dso"], stdout=subprocess.PIPE,
                              text=True, stderr=subprocess.DEVNULL)
    counts, folded = parse_script(script.stdout)
    script.wait()
    with open(os.path.join(OUT, f"perf-{route}.folded"), "w") as f:
        for k, v in sorted(folded.items(), key=lambda kv: -kv[1]):
            f.write(f"{k} {v}\n")
    os.remove(data)
    return counts


def med(xs):
    return statistics.median(xs)


def machine():
    m = {"os": platform.platform(), "nproc": NPROC}
    if LINUX:
        lscpu = subprocess.run(["lscpu"], capture_output=True, text=True).stdout
        mm = re.search(r"Model name:\s*(.+)", lscpu)
        m["cpu"] = mm.group(1).strip() if mm else "?"
        mem = re.search(r"MemTotal:\s+(\d+) kB", open("/proc/meminfo").read())
        m["mem_gb"] = int(mem.group(1)) / 1024 / 1024
        open(os.path.join(OUT, "lscpu.txt"), "w").write(lscpu)
    else:
        sc = lambda k: subprocess.run(["sysctl", "-n", k], capture_output=True, text=True).stdout.strip()
        m["cpu"] = sc("machdep.cpu.brand_string")
        m["mem_gb"] = int(sc("hw.memsize")) / 1024 ** 3
    return m


def web_limits():
    src = open(os.path.join(ROOT, "host/src/lib.rs")).read()
    body = re.search(r"pub fn web\(\) -> Self \{(.*?)\n    \}", src, re.S).group(1)
    return ", ".join(f"{k} {v.strip()}" for k, v in re.findall(r"(\w+): ([^,\n]+),", body) if k != "..Limits")


def main():
    os.makedirs(OUT, exist_ok=True)
    runs = []
    order = [(r, s) for r in ROUTES for s in ("kio", "rust")]
    for i in range(ROUNDS):
        log(f"# round {i + 1}/{ROUNDS} load {os.getloadavg()[0]:.2f}")
        for route, side in order if i % 2 == 0 else list(reversed(order)):
            x = one(route, side)
            x["round"] = i + 1
            runs.append(x)
            log(f"  {side:4s} {route:8s} cpu {x['cpu_us']:7.1f} us (sys {x['sys_us']:6.1f})  {x['rps']:8.0f} req/s"
                f"  p50 {x['p50_us']:7.0f} us  p99 {x['p99_us']:7.0f} us  rss {x['rss_mb']:5.1f} MB  load {x['load_before']:.1f}->{x['load_during']:.1f}  bad {x['bad']}")
    tcounts = sorted({1, min(12, NPROC)})
    probes = []
    for i in range(PROBE_ROUNDS):
        for route in ROUTES:
            for t in tcounts:
                p = probe(route, t)
                p["round"] = i + 1
                probes.append(p)
                log(f"  probe {route:8s} threads {t:2d}: cpu {p['cpu_us']:6.1f} us (sys {p['sys_us']:5.1f}) wall {p['wall_us']:6.2f} us {p['rps']:7.0f} req/s")
    splits = {}
    for route in ROUTES:
        log(f"# perf {route}")
        try:
            splits[route] = perf_split(route)
        except Exception as e:  # the numbers above stand without the split
            splits[route] = {"error": repr(e)}
    json.dump({"runs": runs, "probes": probes, "splits": splits}, open(os.path.join(OUT, "results.json"), "w"), indent=1)
    report(runs, probes, splits, tcounts)


def report(runs, probes, splits, tcounts):
    m = machine()
    sh = lambda *c: subprocess.run(list(c), capture_output=True, text=True, cwd=ROOT).stdout.strip()
    L = ["# C24: `--listen` per request", ""]
    L += [f"| | |", "|---|---|",
          f"| commit | {sh('git', 'rev-parse', 'HEAD')}{' (dirty)' if sh('git', 'status', '--porcelain', '--untracked-files=no') else ''} |",
          f"| rustc | {E.get('RUSTC_VERSION', '?')} |",
          f"| os | {m['os']} |", f"| cpu | {m['cpu']} |", f"| nproc | {m['nproc']} |", f"| memory | {m['mem_gb']:.1f} GB |",
          f"| kio limits | no limit flags: the `--listen` defaults (`Limits::web`): {web_limits()}; workers 2 x nproc = {2 * NPROC} |",
          f"| load generator | {sh(OHA, '--version')}, `-c {CONC}`, keep-alive, HTTP/1.1, on the same machine |",
          f"| counts | {ROUNDS} rounds, kio and Rust interleaved (order reversed every other round); each run a fresh server, {WARM} warm-up requests, then {N} measured |",
          f"| CPU per request | server process utime+stime delta over the measured requests / {N} ({'/proc/<pid>/stat' if LINUX else 'ps'}) |",
          f"| peak RSS | {'VmHWM, peak over the server process life, for kio including compiling the program at startup' if LINUX else 'RSS at the end of the run (macOS has no VmHWM)'} |",
          "| load | 1-minute load average before the measured requests, and its maximum sampled every 0.5 s during them |", ""]
    L += ["## Server", "", "Median over rounds (min-max). Latency is oha's per request, client side.", "",
          "| route | side | CPU/req µs | sys µs | req/s | p50 µs | p99 µs | peak RSS MB | load before | load during | non-2xx/errors |",
          "|---|---|---|---|---|---|---|---|---|---|---|"]
    rng = lambda xs, f: f"{f(med(xs))} ({f(min(xs))}-{f(max(xs))})"
    f1, f0 = (lambda v: f"{v:.1f}"), (lambda v: f"{v:.0f}")
    for route in ROUTES:
        for side in ("kio", "rust"):
            xs = [x for x in runs if x["route"] == route and x["side"] == side]
            g = lambda k: [x[k] for x in xs]
            L.append(f"| {route} | {side} | {rng(g('cpu_us'), f1)} | {f1(med(g('sys_us')))} | {rng(g('rps'), f0)} | {rng(g('p50_us'), f0)} | "
                     f"{rng(g('p99_us'), f0)} | {f1(max(g('rss_mb')))} | {f1(med(g('load_before')))} | {f1(med(g('load_during')))} | {sum(g('bad'))} |")
    L += ["", "Responses (one per side, checked before each run):", ""]
    for route in ROUTES:
        for side in ("kio", "rust"):
            b = next(x["body"] for x in runs if x["route"] == route and x["side"] == side)
            L.append(f"- {route} {side}: `{b}`")
    clamp = "" if NPROC >= 12 else f" (12 clamped to nproc = {NPROC})"
    L += ["", "## In-process probe", "",
          f"`Server::handle` in a loop, no sockets, hyper or tokio, `Limits::web`, a fresh instance each; {PROBE_N} requests after 2,000 warm-up, "
          f"{PROBE_ROUNDS} rounds, threads {', '.join(map(str, tcounts))}{clamp}. Median over rounds (min-max); CPU is the process's getrusage.", "",
          "| route | threads | CPU/req µs | sys µs | wall/req µs | req/s | load before |", "|---|---|---|---|---|---|---|"]
    for route in ROUTES:
        for t in tcounts:
            xs = [p for p in probes if p["route"] == route and p["threads"] == t]
            g = lambda k: [p[k] for p in xs]
            L.append(f"| {route} | {t} | {rng(g('cpu_us'), f1)} | {f1(med(g('sys_us')))} | {f1(med(g('wall_us')))} | {f0(med(g('rps')))} | {f1(med(g('load_before')))} |")
    L += ["", "## Split (perf)", ""]
    if not any(splits.values()):
        L.append("Not measured: " + ("perf needs Linux." if not LINUX else "no frame-pointer build or no perf (see run.sh)."))
    for route, c in splits.items():
        if not c:
            continue
        if "error" in c:
            L += [f"{route}: perf record failed:", "", "```", c["error"], "```", ""]
            continue
        total = sum(c.values())
        cpu = med([x["cpu_us"] for x in runs if x["route"] == route and x["side"] == "kio"])
        L += [f"### {route}", "",
              f"`perf record -F 999 --call-graph fp` on the kio server (a frame-pointer build of the same source) for {PERF_SECS} s under `oha -c {CONC}`; "
              f"{total} samples. µs = share x the server's median CPU per request above ({cpu:.1f} µs), an estimate.", "",
              "| phase | samples | share | µs/req |", "|---|---|---|---|"]
        for k, v in sorted(c.items(), key=lambda kv: -kv[1]):
            L.append(f"| {k} | {v} | {100 * v / total:.1f}% | {cpu * v / total:.1f} |")
        L += ["", f"Top symbols: perf-{route}-report.txt; stacks: perf-{route}.folded.", ""]
    open(os.path.join(OUT, "report.md"), "w").write("\n".join(L) + "\n")
    log(f"wrote {OUT}/report.md")


if __name__ == "__main__":
    main()
