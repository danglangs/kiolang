#!/usr/bin/env python3
"""Times every benchmark in kio compiled, kio interpreted, Rust and Python; prints a Markdown table.

Run through bench/run.sh, which builds everything first. Environment:
  KIO       the kio binary (default target/release/kio)
  RUNS      runs per measurement, best kept (default 5)
  ONLY      comma-separated benchmark names to run (default all)
  NO_INTERP set to skip the interpreter (slow)
"""
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

BENCH = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(BENCH)
BUILD = os.path.join(BENCH, "build")
DATA = os.path.join(BENCH, "data")
KIO = os.environ.get("KIO", os.path.join(ROOT, "target/release/kio"))
INTERP = os.path.join(BENCH, "target/release/kio-interp")
RUST = os.path.join(BENCH, "target/release")
RUNS = int(os.environ.get("RUNS", "5"))
# The interpreter's slow runs: one run when the first takes longer than this.
SLOW = 10.0
LIMITS = ["--fuel", str(10**15), "--timeout", "3600", "--memory", "4096"]

# name, label, kio needs the data dir, rust/python take a data file
BENCHES = [
    ("fib", "`fib(35)`, calls", None),
    ("loop", "30 M-iteration integer loop", None),
    ("strings", "100 k `s = s + i.to_str() + \",\"`", None),
    ("list", "1 M push, sort, index and `for` sums", None),
    ("map", "word count, 1 MB of text", "words.txt"),
    ("tree", "tree of 2^20 leaves built and folded", None),
    ("closures", "`map`/`filter` with closures, 1 M", None),
    ("mandel", "Mandelbrot 400x400, 100 iterations", None),
    ("json", "JSON parse + encode, 1 MB (json.kio)", "data.json"),
    ("json_builtin", "JSON parse + encode, 1 MB (built-in)", "data.json"),
]


def wall(cmd, env=None, check=True):
    t = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, env=env)
    dt = time.perf_counter() - t
    if check and r.returncode != 0:
        sys.exit(f"failed: {' '.join(cmd)}\n{r.stderr[-2000:]}")
    return dt, r.stdout.strip()


def best(cmd, env=None, runs=RUNS):
    """Best wall time of `runs` runs after one discarded warm-up run (macOS checks a new binary
    on its first launch; the first kio run fills the compile cache). A warm-up slower than SLOW
    is the measurement."""
    dt, out = wall(cmd, env)
    if dt > SLOW:
        return dt, out, 1
    times = []
    for _ in range(runs):
        dt, out = wall(cmd, env)
        times.append(dt)
    return min(times), out, len(times)


# Benchmarks timed against another's Rust and Python versions.
SHARED = {"json_builtin": "json"}


def commands(name, data):
    kio_file = os.path.join(BUILD, "kio", ("jsonb" if name == "json" else name) + ".kio")
    kio_args = ["--dir", "data=" + DATA] if data else []
    other = SHARED.get(name, name)
    rust = [os.path.join(RUST, other)] + ([os.path.join(DATA, data)] if data else [])
    py = [sys.executable, os.path.join(BENCH, "py", ("json_bench" if other == "json" else other) + ".py")]
    py += [os.path.join(DATA, data)] if data else []
    return (
        [KIO, "run", kio_file] + kio_args + LIMITS,
        [INTERP, kio_file] + kio_args,
        rust,
        py,
    )


def ms(s):
    if s is None:
        return "-"
    if s >= 10:
        return f"{s:.0f} s"
    if s >= 1:
        return f"{s:.2f} s"
    return f"{s * 1000:.0f} ms" if s >= 0.01 else f"{s * 1000:.1f} ms"


def ratio(kio, other):
    """kio's time against another's: "2.1x slower" or "13x faster"."""
    if kio is None or other is None or kio <= 0 or other <= 0:
        return "-"
    r, word = (kio / other, "slower") if kio >= other else (other / kio, "faster")
    return (f"{r:.1f}x" if r < 10 else f"{r:.0f}x") + " " + word


def machine():
    sc = lambda k: subprocess.run(["sysctl", "-n", k], capture_output=True, text=True).stdout.strip()
    cpu, ncpu, mem = sc("machdep.cpu.brand_string"), sc("hw.ncpu"), int(sc("hw.memsize") or 0) >> 30
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True).stdout.split()[1]
    py = sys.version.split()[0]
    return f"{cpu}, {ncpu} cores, {mem} GB; rustc {rustc}; Python {py}"


def load():
    return os.getloadavg()[0]


def main():
    only = set(filter(None, os.environ.get("ONLY", "").split(",")))
    print(f"Machine: {machine()}. Best of {RUNS}. Load average at start {load():.1f}.", flush=True)
    cache = tempfile.mkdtemp(prefix="kio-bench-cache-")
    env = dict(os.environ, XDG_CACHE_HOME=cache)
    empty = os.path.join(BUILD, "kio", "empty.kio")
    # Startup of each, subtracted from every time below so times are the work alone.
    base_kio = best([KIO, "run", empty] + LIMITS, env)[0]
    base_interp = best([INTERP, empty])[0]
    base_py = best([sys.executable, "-c", "pass"])[0]
    base_rust = best(["/usr/bin/true"])[0]
    compile_empty = compile_time([KIO, "run", empty] + LIMITS, env)
    print(f"Startup subtracted: kio (cached) {ms(base_kio)}, interpreter {ms(base_interp)}, python {ms(base_py)}, process {ms(base_rust)}; compiling an empty program {ms(compile_empty)}.\n")
    print("| Benchmark | Rust | Python | kio compiled | compile | kio interpreted | vs Rust | vs Python | vs interpreted |")
    print("|---|---|---|---|---|---|---|---|---|")
    for name, label, data in BENCHES:
        if only and name not in only:
            continue
        kio, interp, rust, py = commands(name, data)
        t_kio, o_kio, _ = best(kio, env)
        t_comp = compile_time(kio, env)
        t_rust, o_rust, _ = best(rust)
        t_py, o_py, _ = best(py)
        t_int, o_int, n_int = (None, o_kio, 0) if os.environ.get("NO_INTERP") else best(interp)
        outs = {"kio": o_kio, "interp": o_int, "rust": o_rust, "python": o_py}
        if len(set(outs.values())) != 1:
            print(f"**{name}: outputs differ: {outs}**")
        c, r, p = t_kio - base_kio, t_rust - base_rust, t_py - base_py
        i = None if t_int is None else t_int - base_interp
        note = "" if n_int in (0, RUNS) else f" ({n_int} run)"
        print(f"| {label} | {ms(r)} | {ms(p)} | {ms(c)} | {ms(t_comp)} | {ms(i)}{note} | {ratio(c, r)} | {ratio(c, p)} | {ratio(c, i)} |", flush=True)
    http_rows()
    print(f"\nLoad average at end {load():.1f}.")
    shutil.rmtree(cache, ignore_errors=True)


def compile_time(cmd, env):
    """Compiling alone: a run with `--fuel 1` stops at once, so an empty compile cache against
    the filled one (`env`) differs by the compile."""
    cmd = [("1" if a == str(10**15) else a) for a in cmd]
    cached = min(wall(cmd, env, check=False)[0] for _ in range(RUNS))
    fresh = []
    for _ in range(RUNS):
        cache = tempfile.mkdtemp(prefix="kio-bench-fresh-")
        fresh.append(wall(cmd, dict(os.environ, XDG_CACHE_HOME=cache), check=False)[0])
        shutil.rmtree(cache, ignore_errors=True)
    return min(fresh) - cached


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_port(port, proc):
    for _ in range(300):
        if proc.poll() is not None:
            sys.exit(f"server exited: {proc.args}")
        try:
            socket.create_connection(("127.0.0.1", port), 0.1).close()
            return
        except OSError:
            time.sleep(0.05)
    sys.exit(f"server did not start: {proc.args}")


def ab(port):
    """Best of RUNS `ab -k -c 12` runs: (requests per second, p99 ms)."""
    url = f"http://127.0.0.1:{port}/hello/kio"
    subprocess.run(["ab", "-k", "-c", "12", "-n", "2000", url], capture_output=True)  # warm up
    results = []
    for _ in range(RUNS):
        r = subprocess.run(["ab", "-k", "-c", "12", "-n", "50000", url], capture_output=True, text=True)
        out = r.stdout
        if r.returncode != 0:
            sys.exit(f"ab failed on port {port}: {r.stderr[-500:]}")
        failed = int(re.search(r"Failed requests:\s+(\d+)", out).group(1))
        non2xx = re.search(r"Non-2xx responses:\s+(\d+)", out)
        if failed or non2xx:
            sys.exit(f"ab: {failed} failed, {non2xx and non2xx.group(1)} non-2xx\n{out}")
        rps = float(re.search(r"Requests per second:\s+([\d.]+)", out).group(1))
        p99 = int(re.search(r"\s99%\s+(\d+)", out).group(1))
        results.append((rps, p99))
    return max(results)


def http_rows():
    only = set(filter(None, os.environ.get("ONLY", "").split(",")))
    if only and "http" not in only:
        return
    servers = {}
    for who in ["kio", "rust", "python"]:
        port = free_port()
        if who == "kio":
            app = os.path.join(BUILD, "hello")
            cmd = [KIO, "run", os.path.join(app, "app.kio"), "--dir", "public=" + os.path.join(app, "public")]
            cmd += ["--listen", f"127.0.0.1:{port}"]
        elif who == "rust":
            cmd = [os.path.join(RUST, "http"), f"127.0.0.1:{port}"]
        else:
            cmd = [sys.executable, os.path.join(BENCH, "py", "http_server.py"), str(port)]
        proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            wait_port(port, proc)
            servers[who] = ab(port)
        finally:
            proc.terminate()
            proc.wait()
    f = lambda w: f"{servers[w][0]:,.0f} req/s, p99 {servers[w][1] or "<1"} ms"
    print(f"\nHTTP, `GET /hello/kio` (route and JSON) under `--listen`, `ab -k -c 12 -n 50000`, best of {RUNS}:\n")
    print("| Server | Throughput |")
    print("|---|---|")
    print(f"| Rust (hyper, tokio) | {f('rust')} |")
    print(f"| Python (http.server, a thread per connection) | {f('python')} |")
    print(f"| kio-web hello example, compiled | {f('kio')} |")


if __name__ == "__main__":
    main()
