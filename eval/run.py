#!/usr/bin/env python3
"""Can models write kio? Runs eval/tasks against a model through `claude -p`.

  eval/run.py --model haiku --lang kio [--spec none] [--tasks 01,02] [--jobs 4] [--kio target/release/kio]

Per task: the model gets task.md (and, for kio, docs/language.md as the system
prompt, or with --spec none a one-line description) and returns one program; up to
--rounds rounds, each feeding back `kio check`'s diagnostics or the check's failure
output. Writes eval/results/<date>-<model>-<lang>[-nospec].json and prints a table.

E4 (generation cost): --spec cookbook (docs/primer.md, then language.md), --spec terse
(language.md, then "Answer with the program only, in one code block."), --lang py
--spec e3 (E3's Python system prompt); --budget/--ledger stop the run at a shared
dollar limit; each round records thinking tokens and the reply's prose.
"""
import argparse
import collections
import concurrent.futures
import datetime
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

EVAL = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(EVAL)

KIO_SYSTEM = "You write programs in kio. This is the kio language reference.\n\n"
KIO_NOSPEC = "You write programs in kio, a subset of Rust with capabilities as parameters."
PY_SYSTEM = "You write Python 3 programs."
TERSE = "\n\nAnswer with the program only, in one code block."

KIO_HOW = """
Write the whole program in kio, as one file. It runs as `kio run prog.kio` with one flag per grant, so `main`'s parameters must be exactly the grants listed, with those names and types. The output is `main`'s return value: return a `Str`, or a `Result` whose `Ok` holds the `Str`; kio prints it. An HTTP handler's `main` runs once per request (`kio run prog.kio --listen ADDR`).

Reply with the program in one fenced code block and nothing else."""

PY_HOW = """
Write the whole program in Python 3, standard library only, as one file. Each grant is an environment variable named after it in capitals: a `Dir` is a directory path; a `Kv` is the path of a file (absent at first) where you may keep state between runs in any format; a `Db` is the path of a SQLite database file; an `Http` is a base URL such as `http://127.0.0.1:8123`, to which you append paths; a `Clock` is the current time, an integer of milliseconds since the Unix epoch; a `Log` is the path of a file to append lines to; for an `Rng`, use Python's `secrets` module. Print the output to stdout. An HTTP handler (`Incoming`, `Outgoing`) is a server on 127.0.0.1, port from the environment variable `PORT`, running until it is killed.

Reply with the program in one fenced code block and nothing else."""

FIX = """

Your previous program:

```
{program}
```

{what}

```
{feedback}
```

Reply with the whole corrected program in one fenced code block and nothing else."""

LISTEN_TYPES = {"Incoming", "Outgoing", "WsMessage", "WsSend"}


def grants(task_md):
    """The `name: Type` pairs on the task's `Grants:` line."""
    for line in task_md.splitlines():
        if line.startswith("Grants:"):
            return [tuple(re.split(r"\s*:\s*", g, maxsplit=1)) for g in re.findall(r"`([^`]+)`", line)]
    return []


def main_params(program):
    m = re.search(r"fn\s+main\s*\(([^)]*)\)", program)
    if not m:
        return []
    return [tuple(re.split(r"\s*:\s*", p.strip(), maxsplit=1)) for p in m.group(1).split(",") if ":" in p]


def over_asked(program, granted):
    """main's parameters beyond the task's grants (listen types compare by type)."""
    names = set(granted)
    types = {t for _, t in granted if t in LISTEN_TYPES}
    return ["%s: %s" % p for p in main_params(program)
            if p not in names and not (p[1] in LISTEN_TYPES and p[1] in types)]


def extract(text):
    """One fenced code block: the first tagged kio/rust/python, else the first."""
    blocks = re.findall(r"```([\w+-]*)[^\n]*\n(.*?)```", text, re.S)
    if not blocks:
        return None
    for tag, body in blocks:
        if tag.lower() in ("kio", "rust", "rs", "python", "py", "python3"):
            return body
    return blocks[0][1]


def ask(model, system, prompt, cwd, detail=None):
    """One `claude -p` call without tools; returns (text, usage dict). With a `detail`
    dict, streams the reply and puts its thinking tokens there (the stream holds the
    thinking block's signature only, not its text)."""
    fmt = ["stream-json", "--verbose"] if detail is not None else ["json"]
    cmd = ["claude", "-p", "--model", model, "--tools", "", "--system-prompt", system,
           "--output-format"] + fmt + ["--no-session-persistence", "--setting-sources", "",
                                       "--strict-mcp-config", "--disable-slash-commands"]
    last = ""
    for attempt in range(3):
        try:
            p = subprocess.run(cmd, input=prompt, capture_output=True, text=True, cwd=cwd, timeout=900)
        except subprocess.TimeoutExpired:
            last = "timed out"
            continue
        try:
            if detail is None:
                out = json.loads(p.stdout)
            else:
                events = [json.loads(l) for l in p.stdout.splitlines() if l.strip()]
                out = next(e for e in events if e.get("type") == "result")
                detail["thinking_tokens"] = out.get("usage", {}).get("output_tokens_details", {}).get("thinking_tokens", 0)
            if not out.get("is_error"):
                u = out.get("usage", {})
                return out.get("result", ""), {
                    "cost_usd": out.get("total_cost_usd", 0.0),
                    "input_tokens": u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0)
                    + u.get("cache_creation_input_tokens", 0),
                    "output_tokens": u.get("output_tokens", 0),
                }
            last = out.get("result", "")
        except (ValueError, StopIteration):
            last = (p.stdout + p.stderr)[-500:]
        time.sleep(5 * (attempt + 1))
    raise RuntimeError("claude -p failed: %s" % last[:500])


class BudgetExceeded(RuntimeError):
    pass


class Ledger:
    """Every call's cost appended to a JSONL file, so several runs, in parallel too, share
    one budget (as eval/e3/run.py's). Stops $2 short: calls already in flight still land."""

    def __init__(self, path, budget):
        self.path, self.budget, self.lock = path, budget, threading.Lock()

    def spent(self):
        return sum(json.loads(l)["cost_usd"] for l in open(self.path)) if os.path.exists(self.path) else 0.0

    def ask(self, model, system, prompt, cwd, detail=None):
        with self.lock:
            spent = self.spent()
        if spent >= self.budget - 2.0:
            raise BudgetExceeded("budget: $%.2f of $%.2f spent" % (spent, self.budget))
        text, usage = ask(model, system, prompt, cwd, detail)
        with self.lock:
            with open(self.path, "a") as f:
                f.write(json.dumps({"model": model, "cost_usd": usage["cost_usd"], "time": round(time.time())}) + "\n")
        return text, usage


def prose(text, program):
    """The reply less its program: what the model wrote around the code block."""
    if program is None:
        return text
    i = text.find(program)
    head = text[:i] if i >= 0 else text
    tail = text[i + len(program):] if i >= 0 else ""
    return re.sub(r"```[\w+-]*[^\n]*\n?", "", head + tail).strip()


def run_cmd(cmd, env, timeout):
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=timeout)
        return p.returncode, (p.stdout + p.stderr)
    except subprocess.TimeoutExpired:
        return 124, "the check timed out after %d s" % timeout


def first_errors(diag):
    """`error: ..` messages, the path and line dropped."""
    return [m.strip() for m in re.findall(r"error: (.*)", diag)]


def run_task(task, args, system):
    tdir = os.path.join(EVAL, "tasks", task)
    task_md = open(os.path.join(tdir, "task.md")).read()
    granted = grants(task_md)
    ext = "kio" if args.lang == "kio" else "py"
    check = os.path.join(tdir, "check.sh" if args.lang == "kio" else "check_py.sh")
    env = dict(os.environ, KIO=args.kio)
    work = tempfile.mkdtemp(prefix="kio-eval-%s-" % task)
    prog_path = os.path.join(work, "prog." + ext)
    base = task_md.strip() + "\n" + (KIO_HOW if args.lang == "kio" else PY_HOW)
    rec = {"task": task, "rounds": [], "passed": False, "rounds_used": 0,
           "cost_usd": 0.0, "input_tokens": 0, "output_tokens": 0, "thinking_tokens": 0}
    prompt = base
    start = time.time()
    try:
        for rnd in range(1, args.rounds + 1):
            detail = {}
            text, usage = args.ask(args.model, system, prompt, work, detail)
            for k, v in usage.items():
                rec[k] += v
            rec["thinking_tokens"] += detail["thinking_tokens"]
            program = extract(text)
            r = {"round": rnd, "program": program, "checked": None, "passed": False, "feedback": "",
                 "output_tokens": usage["output_tokens"], "thinking_tokens": detail["thinking_tokens"],
                 "reply_bytes": len(text.encode()), "prose": prose(text, program)}
            rec["rounds"].append(r)
            rec["rounds_used"] = rnd
            if program is None:
                r["feedback"] = "no fenced code block in the reply"
                prompt = base + "\n\nYour reply had no fenced code block. Reply with the whole program in one fenced code block."
                continue
            open(prog_path, "w").write(program)
            what = "The check failed with this output:"
            if args.lang == "kio":
                r["over_asked"] = over_asked(program, granted)
                code, diag = run_cmd([args.kio, "check", prog_path], env, 60)
                diag = diag.replace(prog_path, "prog.kio")
                r["checked"] = code == 0
                if code != 0:
                    r["feedback"] = diag[-4000:]
                    r["errors"] = first_errors(diag)
                    prompt = base + FIX.format(program=program, what="`kio check` rejected it:", feedback=r["feedback"])
                    continue
                r["report"] = diag.strip()
            code, out = run_cmd([check, prog_path], env, 180)
            out = out.replace(prog_path, "prog." + ext)
            if code == 0:
                r["passed"] = rec["passed"] = True
                break
            r["feedback"] = out[-4000:]
            prompt = base + FIX.format(program=program, what=what, feedback=r["feedback"])
    except RuntimeError as e:  # BudgetExceeded included
        rec["error"] = str(e)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    rec["seconds"] = round(time.time() - start, 1)
    rs = rec["rounds"]
    rec["checks_first"] = bool(rs and rs[0]["checked"]) if args.lang == "kio" else None
    rec["pass_first"] = bool(rs and rs[0]["passed"])
    rec["over_asked"] = sorted({p for r in rs for p in r.get("over_asked", [])})
    return rec


def summarize(results, lang):
    head = "%-22s %6s %6s %6s %6s  %s" % ("task", "check1", "pass1", "pass5", "rounds", "over-asked")
    print(head)
    print("-" * len(head))
    yes = lambda b: "-" if b is None else ("yes" if b else "no")
    for r in results:
        print("%-22s %6s %6s %6s %6d  %s" % (r["task"], yes(r["checks_first"]), yes(r["pass_first"]),
                                              yes(r["passed"]), r["rounds_used"],
                                              ", ".join(r["over_asked"]) or r.get("error", "")[:40]))
    n = len(results)
    tot = lambda k: sum(1 for r in results if r[k])
    print("-" * len(head))
    print("%-22s %6s %6s %6s" % ("total of %d" % n, "%d" % tot("checks_first") if lang == "kio" else "-",
                                  tot("pass_first"), tot("passed")))
    print("cost $%.2f, %d input and %d output tokens" % (
        sum(r["cost_usd"] for r in results), sum(r["input_tokens"] for r in results),
        sum(r["output_tokens"] for r in results)))
    errs = collections.Counter(re.sub(r"`[^`]*`", "`_`", e)
                               for r in results for rd in r["rounds"] for e in rd.get("errors", []))
    if errs:
        print("\ndiagnostics hit most:")
        for e, c in errs.most_common(10):
            print("%4d  %s" % (c, e))


def e3_py_system():
    """E3's Python system prompt (the parity framing), from eval/e3/run.py."""
    spec = importlib.util.spec_from_file_location("e3run", os.path.join(EVAL, "e3", "run.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m.PY_SYSTEM


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", required=True, help="a `claude --model` name: haiku, sonnet, opus, ..")
    ap.add_argument("--lang", choices=["kio", "py"], default="kio")
    ap.add_argument("--spec", choices=["full", "none", "cookbook", "terse", "e3"], default="full",
                    help="kio's system prompt: docs/language.md (full), one line (none), docs/primer.md "
                    "then language.md (cookbook), full plus TERSE (terse); for py, e3 is E3's system prompt")
    ap.add_argument("--tasks", help="comma-separated task numbers or names (01,02); default all")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--rounds", type=int, default=5)
    ap.add_argument("--kio", default=shutil.which("kio") or os.path.join(ROOT, "target", "release", "kio"))
    ap.add_argument("--out", help="result file (default eval/results/<date>-<model>-<lang>.json)")
    ap.add_argument("--budget", type=float, help="stop at this many dollars, counted in --ledger")
    ap.add_argument("--ledger", help="JSONL of every call's cost, shared by runs (with --budget)")
    args = ap.parse_args()
    args.kio = os.path.abspath(args.kio)
    if args.lang == "kio" and not os.access(args.kio, os.X_OK):
        sys.exit("no kio binary at %s: build it (cargo build --release -p kio) or pass --kio" % args.kio)

    tasks = sorted(d for d in os.listdir(os.path.join(EVAL, "tasks")) if re.match(r"\d\d-", d))
    if args.tasks:
        want = args.tasks.split(",")
        tasks = [t for t in tasks if t[:2] in want or t in want]
    if args.spec not in ({"full", "e3"} if args.lang == "py" else {"full", "none", "cookbook", "terse"}):
        sys.exit("--spec %s does not apply to --lang %s" % (args.spec, args.lang))
    if args.budget and not args.ledger:
        sys.exit("--budget needs --ledger")
    args.ask = Ledger(args.ledger, args.budget).ask if args.budget else ask
    nospec = args.lang == "kio" and args.spec == "none"
    read_doc = lambda name: open(os.path.join(ROOT, "docs", name)).read()
    if args.lang == "py":
        system = PY_SYSTEM if args.spec == "full" else e3_py_system()
    elif nospec:
        system = KIO_NOSPEC
    elif args.spec == "cookbook":
        system = KIO_SYSTEM + read_doc("primer.md") + "\n\n" + read_doc("language.md")
    else:
        system = KIO_SYSTEM + read_doc("language.md") + (TERSE if args.spec == "terse" else "")

    results = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(run_task, t, args, system): t for t in tasks}
        for f in concurrent.futures.as_completed(futures):
            r = f.result()
            results[r["task"]] = r
            print("%s: %s in %d round(s)" % (r["task"], "pass" if r["passed"] else "FAIL", r["rounds_used"]),
                  file=sys.stderr)
    ordered = [results[t] for t in tasks]

    date = datetime.date.today().isoformat()
    suffix = {"none": "-nospec", "full": ""}.get(args.spec, "-" + args.spec)
    name = "%s-%s-%s%s.json" % (date, args.model, args.lang, suffix)
    out = args.out or os.path.join(EVAL, "results", name)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    doc = {"model": args.model, "lang": args.lang, "spec": args.spec, "date": date,
           "rounds": args.rounds, "system_bytes": len(system.encode()),
           "commit": subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                    capture_output=True, text=True).stdout.strip(),
           "tasks": ordered}
    with open(out, "w") as f:
        json.dump(doc, f, indent=1)
    summarize(ordered, args.lang)
    print("\nwrote", os.path.relpath(out))


if __name__ == "__main__":
    main()
