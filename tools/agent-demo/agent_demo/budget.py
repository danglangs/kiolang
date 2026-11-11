"""A shared spend ledger: every model run appends its cost; a new run starts only while the
total plus one request's worst case stays under the budget."""

from __future__ import annotations

import json
import time
from pathlib import Path

LEDGER = Path(__file__).resolve().parents[3] / "eval" / "results" / "d5-ledger.jsonl"
BUDGET = 10.00  # USD, D5 in total, development runs included
WORST = 0.60  # USD, one request at the usage limits, with margin


def spent() -> float:
    if not LEDGER.exists():
        return 0.0
    return sum(json.loads(line)["cost_usd"] for line in LEDGER.read_text().splitlines() if line.strip())


def allow() -> bool:
    return spent() + WORST <= BUDGET


def record(job: str, cost: float, tokens: dict) -> None:
    LEDGER.parent.mkdir(parents=True, exist_ok=True)
    with LEDGER.open("a") as f:
        f.write(json.dumps({"job": job, "cost_usd": round(cost, 5), **tokens, "time": round(time.time())}) + "\n")
