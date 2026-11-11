"""The 10 pilot requests over data/ and a checker for each: pass/fail without judgement.

Each checker recomputes the answer in plain Python from the input files and compares it
with what the run wrote to `out` (or with the run's value). It returns "" on a pass, else
the first difference.
"""

from __future__ import annotations

import csv
import io
import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

VALID = (
    "orders.csv has a header line. Blank lines are ignored (not rows). After trimming spaces around "
    "each field, a row is valid when it has exactly 5 comma-separated fields; order_id and "
    "customer_id are non-empty; order_date is YYYY-MM-DD with month 01-12 and day 01-31; amount is "
    "digits with an optional '.' followed by 1 or 2 digits (no sign); status is paid, pending or "
    "refunded. When an order_id appears in several valid rows, only the first one counts."
)

LOG = (
    "In app.log an entry is a line that starts with a timestamp like 2026-03-01T17:57:12Z, then a "
    "space and a level word (INFO, WARN, ERROR, in any case); every other line (stack traces, "
    "blanks, junk) is ignored. An entry's day is the timestamp's first 10 characters."
)


@dataclass
class Job:
    id: str
    request: str
    check: Callable[[Path, Path, str | None], str]  # (input dir, out dir, value) -> "" or why
    http: bool = False


# ---- reference computations -------------------------------------------------------------

_DATE = re.compile(r"^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$")
_AMOUNT = re.compile(r"^\d+(\.\d{1,2})?$")


def _reason(fields: list[str]) -> str:
    if len(fields) != 5:
        return "fields"
    oid, cid, date, amount, status = fields
    if not oid or not cid:
        return "empty id"
    if not _DATE.match(date):
        return "date"
    if not _AMOUNT.match(amount):
        return "amount"
    if status not in ("paid", "pending", "refunded"):
        return "status"
    return ""


def rows(inp: Path) -> tuple[list[dict], list[int]]:
    """Unique valid orders in file order, and the 1-based line numbers of invalid rows."""
    lines = (inp / "orders.csv").read_text().split("\n")
    seen, out, bad = set(), [], []
    for i, line in enumerate(lines[1:], start=2):
        if line.strip() == "":
            continue
        f = [x.strip() for x in line.split(",")]
        if _reason(f):
            bad.append(i)
            continue
        if f[0] in seen:
            continue
        seen.add(f[0])
        out.append(dict(order_id=f[0], customer_id=f[1], date=f[2], amount=float(f[3]), status=f[4]))
    return out, bad


def customers(inp: Path) -> dict[str, dict]:
    return {c["id"]: c for c in json.loads((inp / "customers.json").read_text())}


def paid_totals(inp: Path) -> dict[str, float]:
    t: dict[str, float] = {}
    for r in rows(inp)[0]:
        if r["status"] == "paid":
            t[r["customer_id"]] = t.get(r["customer_id"], 0.0) + r["amount"]
    return t


def log_entries(inp: Path) -> list[tuple[str, str]]:
    out = []
    for line in (inp / "app.log").read_text().split("\n"):
        m = re.match(r"^(\d{4}-\d{2}-\d{2})T\d{2}:\d{2}:\d{2}Z ([A-Za-z]+)( |$)", line)
        if m:
            out.append((m.group(1), m.group(2).upper()))
    return out


# ---- comparison helpers ----------------------------------------------------------------


def _close(a: float, b: float) -> bool:
    return abs(a - b) < 0.006


def _read(out: Path, name: str) -> str | None:
    p = out / name
    return p.read_text() if p.is_file() else None


def _csv(text: str) -> list[list[str]]:
    return [[c.strip() for c in r] for r in csv.reader(io.StringIO(text.strip()))]


def _money_rows(got: list[list[str]], want: list[tuple[str, float]], header: list[str]) -> str:
    if not got or got[0] != header:
        return f"header {got[:1]} != {header}"
    body = got[1:]
    if [r[0] for r in body] != [k for k, _ in want]:
        return f"keys {[r[0] for r in body]} != {[k for k, _ in want]}"
    for r, (k, v) in zip(body, want):
        if len(r) != 2 or not re.match(r"^-?\d+\.\d\d$", r[1]) or not _close(float(r[1]), v):
            return f"{k}: {r[1:]} != {v:.2f} (2 decimals)"
    return ""


# ---- the jobs ---------------------------------------------------------------------------


def c1(inp, out, value):
    got = _read(out, "totals.csv")
    if got is None:
        return "no totals.csv"
    want = sorted(paid_totals(inp).items())
    return _money_rows(_csv(got), want, ["customer_id", "total"])


def c2(inp, out, value):
    got = _read(out, "duplicates.txt")
    if got is None:
        return "no duplicates.txt"
    counts: dict[str, int] = {}
    for line in (inp / "orders.csv").read_text().split("\n")[1:]:
        oid = line.split(",")[0].strip()
        if oid:
            counts[oid] = counts.get(oid, 0) + 1
    want = sorted(k for k, n in counts.items() if n > 1)
    lines = [x.strip() for x in got.strip().split("\n") if x.strip()]
    return "" if lines == want else f"{lines} != {want}"


def c3(inp, out, value):
    got = _read(out, "by_country.json")
    if got is None:
        return "no by_country.json"
    cs = customers(inp)
    want: dict[str, float] = {}
    for r in rows(inp)[0]:
        if r["status"] == "paid":
            k = (cs.get(r["customer_id"]) or {}).get("country") or "unknown"
            want[k] = want.get(k, 0.0) + r["amount"]
    try:
        g = json.loads(got)
    except ValueError as e:
        return f"not JSON: {e}"
    if not isinstance(g, dict) or sorted(g) != sorted(want):
        return f"keys {sorted(g) if isinstance(g, dict) else g} != {sorted(want)}"
    bad = [k for k in want if not isinstance(g[k], (int, float)) or not _close(g[k], want[k])]
    return f"values differ for {bad}" if bad else ""


def c4(inp, out, value):
    cs = customers(inp)
    t = paid_totals(inp)
    top = sorted(t.items(), key=lambda kv: -kv[1])[:3]
    want = [(cs[k]["name"] if k in cs else k, v) for k, v in top]
    try:
        g = json.loads(value or "")
    except ValueError:
        return f"value is not JSON: {value!r:.200}"
    if not isinstance(g, list) or len(g) != 3:
        return f"not a 3-element array: {g!r:.200}"
    for e, (name, v) in zip(g, want):
        if not isinstance(e, dict) or e.get("name") != name or not _close(float(e.get("total", -1)), v):
            return f"{e!r} != {{name: {name}, total: {v:.2f}}}"
    return ""


def c5(inp, out, value):
    got = _read(out, "rejects.txt")
    if got is None:
        return "no rejects.txt"
    want = rows(inp)[1]
    nums = []
    for line in got.strip().split("\n"):
        m = re.match(r"^\s*(\d+):\s*\S", line)
        if not m:
            return f"line not '<n>: <reason>': {line!r}"
        nums.append(int(m.group(1)))
    return "" if nums == want else f"{nums} != {want}"


def c6(inp, out, value):
    got = _read(out, "march_1_15.csv")
    if got is None:
        return "no march_1_15.csv"
    want = [r["order_id"] for r in rows(inp)[0] if "2026-03-01" <= r["date"] <= "2026-03-15"]
    g = _csv(got)
    if not g or g[0] != ["order_id", "customer_id", "order_date", "amount", "status"]:
        return f"header {g[:1]}"
    ids = [r[0] for r in g[1:]]
    return "" if ids == want else f"{ids} != {want}"


def c7(inp, out, value):
    got = _read(out, "errors_per_day.csv")
    if got is None:
        return "no errors_per_day.csv"
    n: dict[str, int] = {}
    for day, lvl in log_entries(inp):
        if lvl == "ERROR":
            n[day] = n.get(day, 0) + 1
    want = [["day", "errors"]] + [[d, str(n[d])] for d in sorted(n)]
    g = _csv(got)
    return "" if g == want else f"{g} != {want}"


def c8(inp, out, value):
    got = _read(out, "clean.csv")
    if got is None:
        return "no clean.csv"
    rs = sorted(rows(inp)[0], key=lambda r: r["order_id"])
    want = [["order_id", "customer_id", "order_date", "amount", "status"]] + [
        [r["order_id"], r["customer_id"], r["date"], f"{r['amount']:.2f}", r["status"]] for r in rs
    ]
    lines = got.strip().split("\n")
    g = [line.split(",") for line in lines]
    if g != want:
        diff = next((f"line {i + 1}: {a} != {b}" for i, (a, b) in enumerate(zip(g, want)) if a != b), "")
        return diff or f"{len(g)} lines != {len(want)}"
    return ""


def c9(inp, out, value):
    got = _read(out, "summary.json")
    if got is None:
        return "no summary.json"
    rs = rows(inp)[0]
    want = {
        "orders": len(rs),
        "revenue": sum(r["amount"] for r in rs if r["status"] == "paid"),
        "refunded": sum(r["amount"] for r in rs if r["status"] == "refunded"),
        "customers": len({r["customer_id"] for r in rs}),
        "first_date": min(r["date"] for r in rs),
        "last_date": max(r["date"] for r in rs),
    }
    try:
        g = json.loads(got)
    except ValueError as e:
        return f"not JSON: {e}"
    if not isinstance(g, dict) or sorted(g) != sorted(want):
        return f"keys {sorted(g) if isinstance(g, dict) else g} != {sorted(want)}"
    for k, v in want.items():
        ok = _close(g[k], v) if isinstance(v, float) and isinstance(g[k], (int, float)) else g[k] == v
        if not ok:
            return f"{k}: {g[k]!r} != {v!r}"
    return ""


def c10(inp, out, value):
    want = ",".join(sorted(r["order_id"] for r in rows(inp)[0] if r["amount"] > 500))
    got = (value or "").strip()
    return "" if got == want else f"{got!r} != {want!r}"


RATES = {"EUR": 0.92, "GBP": 0.79, "JPY": 151.3}


def c11(inp, out, value):
    got = _read(out, "totals_eur.csv")
    if got is None:
        return "no totals_eur.csv"
    want = sorted((k, v * RATES["EUR"]) for k, v in paid_totals(inp).items())
    return _money_rows(_csv(got), want, ["customer_id", "total_eur"])


JOBS = [
    Job("01-totals", "Total the amount of paid orders per customer. " + VALID +
        " Write totals.csv with header customer_id,total, one row per customer with at least one "
        "paid order, total with exactly 2 decimals, sorted by customer_id.", c1),
    Job("02-duplicates", "Find order ids that appear on more than one line of orders.csv (any line "
        "after the header whose first field, trimmed, is non-empty, valid or not). Write "
        "duplicates.txt with one id per line, sorted, and return how many there are.", c2),
    Job("03-join-country", "Join the orders with customers.json and total the paid amount per "
        "customer country. " + VALID + " Orders whose customer is missing from customers.json, or "
        "whose country is null, count under \"unknown\". Write by_country.json: one JSON object "
        "mapping country code to the total as a number.", c3),
    Job("04-top3", "Which 3 customers have the largest paid totals? " + VALID + " Return (as the "
        "program's value) a JSON array of 3 objects {\"name\": customer name from customers.json, "
        "\"total\": number}, largest first. Write no files.", c4),
    Job("05-rejects", "Validate orders.csv. " + VALID + " Write rejects.txt with one line per "
        "invalid row, \"<line number>: <reason>\", in file order, line numbers 1-based counting "
        "the header as line 1. Duplicates of a valid row are not rejects.", c5),
    Job("06-date-range", "List the orders dated 2026-03-01 through 2026-03-15 inclusive. " + VALID +
        " Write march_1_15.csv: the header line order_id,customer_id,order_date,amount,status then "
        "each such order's trimmed fields, in file order.", c6),
    Job("07-errors-per-day", "Make a per-day histogram of errors in app.log. " + LOG + " Count "
        "entries whose level is ERROR. Write errors_per_day.csv with header day,errors, one row "
        "per day with at least one error, sorted by day.", c7),
    Job("08-clean-csv", "Write a clean copy of orders.csv as clean.csv. " + VALID + " Keep only "
        "valid rows, first occurrence of each order_id, fields trimmed, amount with exactly 2 "
        "decimals, sorted by order_id, same header, no spaces, no quoting.", c8),
    Job("09-summary-json", "Write summary.json for a dashboard. " + VALID + " Over the valid "
        "orders: {\"orders\": count, \"revenue\": sum of paid amounts, \"refunded\": sum of "
        "refunded amounts, \"customers\": count of distinct customer_id, \"first_date\": earliest "
        "order_date, \"last_date\": latest order_date}. Numbers as JSON numbers, dates as strings.",
        c9),
    Job("10-over-threshold", "Find the orders with an amount over 500.00 (any status). " + VALID +
        " Return (as the program's value) their order ids, sorted, joined with commas and no "
        "spaces, nothing else. Write no files.", c10),
    Job("11-http-eur", "Convert each customer's paid total to euros using today's rate from the "
        "rates API (the `api: Http` grant; GET /v1/rates?base=USD returns JSON like "
        "{\"base\":\"USD\",\"rates\":{\"EUR\":0.9}}; authentication is added by the host). " + VALID +
        " Write totals_eur.csv with header customer_id,total_eur, one row per customer with a paid "
        "order, total_eur = total * EUR rate with exactly 2 decimals, sorted by customer_id.",
        c11, http=True),
]
