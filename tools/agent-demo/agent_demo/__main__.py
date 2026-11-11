"""python -m agent_demo "<request>" --input DIR [--out DIR] [--kio PATH] [--rates-api]"""

from __future__ import annotations

import argparse
import sys
import tempfile
from contextlib import nullcontext
from pathlib import Path

from kio_run import Kio

from . import budget, standin
from .agent import Session, authority_report, build_agent, cost, kio_binary, read_key, run_request


def main() -> int:
    ap = argparse.ArgumentParser(prog="agent_demo", description="An agent that writes kio for file jobs.")
    ap.add_argument("request")
    ap.add_argument("--input", required=True, help="directory the program may read (granted read-only)")
    ap.add_argument("--out", help="directory the program may write (default: a new temp dir)")
    ap.add_argument("--kio", help="kio binary (default: $KIO, then PATH)")
    ap.add_argument("--rates-api", action="store_true", help="also grant `api: Http` to a local stand-in rates API")
    a = ap.parse_args()

    if not budget.allow():
        print(f"budget: ${budget.spent():.2f} of ${budget.BUDGET:.2f} spent; not starting", file=sys.stderr)
        return 1
    inp = Path(a.input).resolve()
    out = Path(a.out or tempfile.mkdtemp(prefix="kio-demo-out-")).resolve()
    out.mkdir(parents=True, exist_ok=True)
    agent = build_agent(read_key())
    api = standin.running(out.parent / (out.name + "-api.log")) if a.rates_api else nullcontext(None)
    with api as base:
        s = Session(Kio(kio_binary(a.kio)), inp, out, http_base=base)
        answer, usage, failure = run_request(agent, s, a.request)
    usd = cost(usage)
    budget.record("cli", usd, {"input_tokens": usage.input_tokens, "output_tokens": usage.output_tokens})

    print("== Answer\n" + (answer or f"(no answer: {failure})"))
    print("\n== Authority\n" + authority_report(s))
    print(f"\n== Outputs\n{out}")
    for f in sorted(out.iterdir()):
        print(f"  {f.name} ({f.stat().st_size} bytes)")
    print(f"\n{usage.requests} model calls, {usage.tool_calls} kio runs, "
          f"{usage.input_tokens} in / {usage.output_tokens} out tokens, ${usd:.3f}")
    return 0 if answer else 1


if __name__ == "__main__":
    sys.exit(main())
