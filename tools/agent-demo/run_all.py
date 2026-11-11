"""Run the pilot requests and write eval/results/d5-<date>.json: pass/fail, rounds, tokens, cost.

    KIO=target/release/kio .venv/bin/python run_all.py [job ids..] [--name suffix]
"""

from __future__ import annotations

import argparse
import datetime
import json
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "kio-py" / "src"))

from kio_run import Kio  # noqa: E402

from agent_demo import budget, standin  # noqa: E402
from agent_demo.agent import MODEL, Session, build_agent, cost, kio_binary, read_key, run_request  # noqa: E402
from agent_demo.jobs import JOBS  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("jobs", nargs="*", help="job id prefixes (default: all)")
    ap.add_argument("--name", default="", help="suffix for the result file")
    ap.add_argument("--kio")
    a = ap.parse_args()
    jobs = [j for j in JOBS if not a.jobs or any(j.id.startswith(p) for p in a.jobs)]
    data = HERE / "data"
    runs = HERE / "runs" / datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    kio = Kio(kio_binary(a.kio))
    agent = build_agent(read_key())
    results = []
    for job in jobs:
        if not budget.allow():
            print(f"budget: ${budget.spent():.2f} spent of ${budget.BUDGET:.2f}; stopping before {job.id}")
            break
        out = runs / job.id
        out.mkdir(parents=True)
        api_log = runs / (job.id + "-api.log")
        if job.http:
            with standin.running(api_log) as base:
                s = Session(kio, data, out, http_base=base)
                answer, usage, failure = run_request(agent, s, job.request)
        else:
            s = Session(kio, data, out)
            answer, usage, failure = run_request(agent, s, job.request)
        usd = cost(usage)
        budget.record(job.id, usd, {"input_tokens": usage.input_tokens, "output_tokens": usage.output_tokens})
        last = s.last_run()  # `out` holds its files
        if last is None:
            why = failure or "no run"
        elif not last["ok"]:
            why = failure or "last run failed: " + "; ".join(last["errors"])
        else:
            why = failure or job.check(data, out, last.get("value"))
        if job.http and not why:
            log = [json.loads(x) for x in api_log.read_text().splitlines()] if api_log.exists() else []
            if not log or not all(r["auth_ok"] for r in log):
                why = f"stand-in API saw {len(log)} requests, auth_ok {[r['auth_ok'] for r in log]}"
        r = {
            "id": job.id, "pass": not why, "why": why, "rounds": usage.tool_calls, "requests": usage.requests,
            "kio_runs": sum(1 for x in s.attempts if x.get("ran")), "check_failures": sum(1 for x in s.attempts if not x.get("ran")),
            "input_tokens": usage.input_tokens, "output_tokens": usage.output_tokens,
            "cache_read_tokens": usage.cache_read_tokens, "cache_write_tokens": usage.cache_write_tokens,
            "cost_usd": round(usd, 4), "answer": answer, "report": last.get("report") if last else None,
            "audit": last.get("audit") if last else None, "program": last.get("source") if last else None,
        }
        results.append(r)
        print(f"{job.id:20} {'PASS' if r['pass'] else 'FAIL'} rounds={r['rounds']} ${usd:.3f} {why}")
        if failure and usage.requests == 0:
            print("the model was not reached; stopping")
            break
    total = sum(r["cost_usd"] for r in results)
    summary = {
        "model": MODEL, "date": datetime.date.today().isoformat(), "passed": sum(r["pass"] for r in results),
        "total": len(results), "cost_usd": round(total, 4), "ledger_total_usd": round(budget.spent(), 4),
        "prices_per_mtok": {"input": 2.0, "output": 10.0, "cache_read": 0.2, "cache_write": 2.5},
        "results": results,
    }
    name = f"d5-{summary['date']}{'-' + a.name if a.name else ''}.json"
    dest = HERE.parent.parent / "eval" / "results" / name
    dest.write_text(json.dumps(summary, indent=1) + "\n")
    print(f"{summary['passed']}/{summary['total']} passed, ${total:.2f}; ledger ${summary['ledger_total_usd']:.2f}; {dest}")
    if not any(r.get("program") for r in results):
        shutil.rmtree(runs, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
