"""A Pydantic AI agent with one tool, `run_kio`: the model writes kio, the app runs it with
grants the app fixes. The model never chooses grants; it may say in text what it would need."""

from __future__ import annotations

import os
import re
import shutil
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

os.environ.setdefault("PYDANTIC_AI_NO_BANNER", "1")

from kio_run import Grants, Kio, Limits  # noqa: E402
from pydantic_ai import Agent, RunContext  # noqa: E402
from pydantic_ai.models.anthropic import AnthropicModel, AnthropicModelSettings  # noqa: E402
from pydantic_ai.providers.anthropic import AnthropicProvider  # noqa: E402
from pydantic_ai.usage import RunUsage, UsageLimits  # noqa: E402

MODEL = "claude-sonnet-5-5"
KEY_FILE = Path("~/.config/kio-demo/anthropic_key").expanduser()
DOCS = Path(__file__).resolve().parents[3] / "docs"

# USD per million tokens, claude-sonnet-5-5 (claude-api skill's model table, cached 2026-10-06):
# input 2.00, output 10.00, cache read 0.20; cache write (5 min) taken as 1.25x input.
PRICE = {"input": 2.00, "output": 10.00, "cache_read": 0.20, "cache_write": 2.50}

HTTP_KEY_VAR = "KIO_DEMO_RATES_KEY"  # a fake key for the local stand-in API, never a real secret

INTRO = """You complete data jobs by writing kio programs and running them with the `run_kio` tool.
kio is described below: first idioms by example, then the language reference.

Rules:
- The application fixes the grants; you cannot change them. Declare `main`'s parameters exactly
  as the user message lists them (same names, same types, nothing else).
- `input` is read-only: read the data from it. Write output files to `out` (top level, the file
  names the request asks for). Return the value the request asks for from `main`, else a short
  status string.
- `run_kio` checks your program first; on errors, fix them and call it again. When it runs, read
  the value and files it reports and verify them against the request before you answer.
- If the job needs authority the grants do not give (another directory, a host), do not try to
  work around it: say in your answer what you would need.
- Final answer: two or three sentences saying what was done and the key numbers.
"""


def instructions() -> str:
    return INTRO + "\n\n" + (DOCS / "primer.md").read_text() + "\n\n" + (DOCS / "language.md").read_text()


@dataclass
class Session:
    kio: Kio
    input_dir: Path
    out_dir: Path
    http_base: str | None = None  # the stand-in API's origin, when the job has one
    attempts: list[dict] = field(default_factory=list)

    def grants(self) -> Grants:
        g = Grants().dir("input", self.input_dir, ro=True).dir("out", self.out_dir)
        if self.http_base:
            g.http("api", self.http_base, methods=["GET"], headers={"Authorization": HTTP_KEY_VAR})
        return g

    def grant_lines(self) -> list[str]:
        lines = [f"input: Dir  (read-only) = {self.input_dir}", f"out: Dir  (writable) = {self.out_dir}"]
        if self.http_base:
            lines.append(f"api: Http  (GET only) = {self.http_base}, Authorization header injected by the host")
        return lines

    def signature(self) -> str:
        return "fn main(input: Dir, out: Dir" + (", api: Http" if self.http_base else "") + ")"

    def last_run(self) -> dict | None:
        """The last attempt that ran: `out` holds its files."""
        return next((a for a in reversed(self.attempts) if a.get("ran")), None)


def audit_summary(records: list[dict]) -> str:
    calls = Counter((r.get("cap"), r.get("method"), r.get("result")) for r in records if r.get("event") == "call")
    parts = [f"{cap}.{m} x{n} {res}" for (cap, m, res), n in sorted(calls.items(), key=lambda kv: str(kv[0]))]
    end = next((r for r in records if r.get("event") == "end"), {})
    return (", ".join(parts) or "no host calls") + (f"; outcome {end.get('outcome')}" if end else "")


def _files(out: Path) -> list[tuple[str, int, str | None]]:
    res = []
    for p in sorted(out.rglob("*")):
        if p.is_file():
            data = p.read_bytes()
            small = data.decode("utf-8", "replace") if len(data) <= 2000 else None
            res.append((str(p.relative_to(out)), len(data), small))
    return res


def run_kio_impl(s: Session, source: str) -> str:
    """The tool body, separate from Pydantic AI so it can be tested without a model."""
    grants = s.grants()
    attempt: dict = {"source": source, "ran": False}
    s.attempts.append(attempt)
    checked = s.kio.check(source, grants=grants)
    if not checked.ok:
        attempt.update(ok=False, errors=[str(e) for e in checked.errors], report=checked.report)
        text = "CHECK FAILED (nothing ran)\n" + checked.error_text()
        if any(e.kind == "grant" for e in checked.errors):
            text += f"\nThe app's grants are fixed: declare exactly `{s.signature()}`."
        return text
    for p in s.out_dir.iterdir():  # each run starts from an empty output dir
        shutil.rmtree(p) if p.is_dir() else p.unlink()
    r = s.kio.run(source, grants, limits=Limits(fuel=2_000_000_000, timeout=20))
    files = _files(s.out_dir)
    attempt.update(
        ran=True, ok=r.ok, value=r.value, errors=[str(e) for e in r.errors], report=r.report,
        audit=audit_summary(r.audit_records), files=[(n, size) for n, size, _ in files],
    )
    out = [("RUN OK" if r.ok else "RUN FAILED"), "authority: " + "; ".join(r.report)]
    if r.errors:
        out.append("errors:\n" + r.error_text())
    if r.value is not None:
        out.append("value:\n" + (r.value if len(r.value) <= 4000 else r.value[:4000] + "\n.. (truncated)"))
    out.append(f"files in out ({len(files)}):")
    for name, size, small in files:
        out.append(f"- {name} ({size} bytes)" + (f"\n{small}" if small is not None else " (too large to show)"))
    out.append("audit: " + attempt["audit"])
    return "\n".join(out)


def build_agent(api_key: str) -> Agent[Session, str]:
    model = AnthropicModel(MODEL, provider=AnthropicProvider(api_key=api_key))
    settings = AnthropicModelSettings(
        max_tokens=16000, anthropic_effort="medium", anthropic_cache_instructions=True, anthropic_cache=True,
    )
    agent = Agent(model, deps_type=Session, instructions=instructions(), model_settings=settings)

    @agent.tool
    def run_kio(ctx: RunContext[Session], source: str) -> str:
        """Check and run a kio program (the whole source of one file) with the app's fixed grants.
        Returns check errors to fix, or the run's value, the files written to `out` and an audit summary."""
        return run_kio_impl(ctx.deps, source)

    return agent


def read_key() -> str:
    """The Anthropic key from KEY_FILE. Never printed, logged or put in an error."""
    try:
        key = KEY_FILE.read_text().strip()
    except OSError:
        raise SystemExit(f"no API key: put it in {KEY_FILE} (one line, chmod 600)") from None
    if not key:
        raise SystemExit(f"{KEY_FILE} is empty")
    return key


def cost(u: RunUsage) -> float:
    """USD for a run's usage. Pydantic AI's input_tokens include cache reads and writes."""
    plain = max(u.input_tokens - u.cache_read_tokens - u.cache_write_tokens, 0)
    return (
        plain * PRICE["input"] + u.cache_write_tokens * PRICE["cache_write"]
        + u.cache_read_tokens * PRICE["cache_read"] + u.output_tokens * PRICE["output"]
    ) / 1e6


def prompt(s: Session, request: str) -> str:
    listing = []
    for p in sorted(s.input_dir.iterdir()):
        if p.is_file():
            head = p.read_text(errors="replace").split("\n")[:3]
            listing.append(f"- {p.name} ({p.stat().st_size} bytes), first lines:\n    " + "\n    ".join(head))
    return (
        "Grants (fixed by the app):\n" + "\n".join("- " + g for g in s.grant_lines())
        + f"\nSo `main` is `{s.signature()} -> ..`.\n\nFiles in input:\n" + "\n".join(listing)
        + "\n\nRequest: " + request
    )


LIMITS = UsageLimits(request_limit=10, output_tokens_limit=60_000)


def run_request(agent: Agent[Session, str], s: Session, request: str) -> tuple[str | None, RunUsage, str | None]:
    """(answer, usage, failure). Usage is counted even when the run stops at a limit."""
    usage = RunUsage()
    try:
        res = agent.run_sync(prompt(s, request), deps=s, usage=usage, usage_limits=LIMITS)
        return res.output, usage, None
    except Exception as e:  # noqa: BLE001 - a run that stops is a failed request, not a crash
        return None, usage, type(e).__name__ + ": " + _redact(str(e))[:300]


def _redact(text: str) -> str:
    """API error texts do not carry the key; strip anything key-shaped anyway."""
    return re.sub(r"sk-ant-[A-Za-z0-9_-]+", "[key]", text)


def authority_report(s: Session) -> str:
    lines = ["Grants (fixed by the app):"] + ["  " + g for g in s.grant_lines()]
    a = s.last_run() or (s.attempts[-1] if s.attempts else None)
    if a:
        lines.append("Program's authority (kio's report): " + "; ".join(a.get("report") or []))
        if a.get("ran"):
            lines.append("Audit: " + a["audit"])
    lines.append(f"Runs: {sum(1 for x in s.attempts if x.get('ran'))} of {len(s.attempts)} attempts passed the check")
    return "\n".join(lines)


def kio_binary(explicit: str | None) -> str:
    return explicit or os.environ.get("KIO") or shutil.which("kio") or "kio"
