"""Drive the `kio` binary with `--json`; no human text is parsed."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from contextlib import contextmanager
from dataclasses import replace
from pathlib import Path
from typing import Iterator

from .grants import Grants, Limits
from .result import Result


class KioError(Exception):
    """kio did not produce its JSON object: a missing binary or a crash."""


class Kio:
    def __init__(self, binary: str = "kio", timeout: float | None = 300):
        self.binary = binary
        self.timeout = timeout  # wall-clock cap on the process; kio's own limits are Limits

    def check(
        self,
        source_or_path,
        base=None,
        grants: Grants | None = None,
        modules: dict[str, str] | None = None,
    ) -> Result:
        """Type-check; `base` (source or path) diffs the authority against an older program
        (exit 3 and `diff.added` when authority was added); `grants` also checks they bind."""
        with _program(source_or_path, modules) as path, _maybe(base) as base_path, _tmp() as tmp:
            args = ["check", path]
            if base_path:
                args += ["--base", base_path]
            if grants is not None:
                gfile = grants._file
                if gfile is None:
                    gfile = os.path.join(tmp, "grants")
                    Path(gfile).write_text(grants.file_text())
                args += ["--grants", gfile]
            return self._call(args)

    def run(
        self,
        source_or_path,
        grants: Grants,
        audit: bool = True,
        limits: Limits | None = None,
        modules: dict[str, str] | None = None,
    ) -> Result:
        """Run `main` with exactly `grants`. With `audit` a fresh temporary audit log is
        read back into `Result.audit_records` and removed."""
        with _program(source_or_path, modules) as path, _tmp() as tmp:
            args = ["run", path] + grants.args(limits)
            log = os.path.join(tmp, "audit.jsonl")
            if audit:
                args += ["--audit", log]
            res = self._call(args)
            if audit and os.path.exists(log):
                with open(log, encoding="utf-8") as f:
                    records = [json.loads(line) for line in f if line.strip()]
                res = replace(res, audit_records=records)
            return res

    def _call(self, args: list[str]) -> Result:
        cmd = [self.binary, *args, "--json"]
        try:
            p = subprocess.run(cmd, capture_output=True, text=True, timeout=self.timeout)
        except (OSError, subprocess.TimeoutExpired) as e:
            raise KioError(f"{self.binary}: {e}") from e
        try:
            return Result.from_object(json.loads(p.stdout), p.returncode)
        except (ValueError, KeyError, TypeError) as e:
            raise KioError(f"no JSON object from kio (exit {p.returncode}): {p.stderr.strip()[:500]}") from e


@contextmanager
def _tmp() -> Iterator[str]:
    with tempfile.TemporaryDirectory(prefix="kio-run-") as d:
        yield d


@contextmanager
def _maybe(src) -> Iterator[str | None]:
    if src is None:
        yield None
    else:
        with _program(src, None) as p:
            yield p


@contextmanager
def _program(src, modules: dict[str, str] | None) -> Iterator[str]:
    """A path to run. A `Path` (any `os.PathLike`) is used as is; a `str` is always source
    (model output is never taken as a path), written as `main.kio` in a temp dir. `mod web;` loads `web.kio` beside the
    root file, so `modules` ({"web": text}) become `web.kio` there."""
    if isinstance(src, os.PathLike):
        if modules:
            raise ValueError("modules apply to source text; a path has its module files beside it")
        yield os.fspath(src)
        return
    with _tmp() as d:
        for name, text in (modules or {}).items():
            if not name.isidentifier():
                raise ValueError(f"module name {name!r} is not an identifier")
            Path(d, f"{name}.kio").write_text(text, encoding="utf-8")
        root = Path(d, "main.kio")
        root.write_text(src, encoding="utf-8")
        yield str(root)
