"""D2's JSON object (docs/product-todo.md, docs/guide.md "JSON output") as dataclasses."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class Error:
    kind: str
    message: str
    hint: str | None = None
    file: str | None = None
    line: int | None = None
    col: int | None = None

    def __str__(self) -> str:
        where = f"{self.file}:{self.line}:{self.col}: " if self.line is not None else ""
        hint = f"\n  hint: {self.hint}" if self.hint else ""
        return f"{where}{self.kind}: {self.message}{hint}"


@dataclass(frozen=True)
class Used:
    fuel: int
    calls: int
    io: int
    peak_memory: int
    ms: float


@dataclass(frozen=True)
class Diff:
    grants: list[str]
    funcs: list[str]
    added: bool


@dataclass(frozen=True)
class Result:
    ok: bool
    value: str | None
    errors: list[Error]
    report: list[str]
    diff: Diff | None
    audit: str | None
    used: Used | None
    # Not in D2's object: the process exit code (3 = `check --base` added authority),
    # and the parsed audit records of a `run(audit=True)` (the file is temporary).
    exit_code: int = 0
    audit_records: list[dict[str, Any]] = field(default_factory=list)

    def value_json(self) -> Any:
        """`value` parsed as JSON; None when there is no value or it is not JSON."""
        if self.value is None:
            return None
        try:
            return json.loads(self.value)
        except ValueError:
            return None

    def error_text(self) -> str:
        """Every error as one text a model can read: place, message, hint."""
        return "\n".join(str(e) for e in self.errors)

    @classmethod
    def from_object(cls, o: dict[str, Any], exit_code: int = 0) -> "Result":
        d, u = o["diff"], o["used"]
        return cls(
            ok=o["ok"],
            value=o["value"],
            errors=[Error(**e) for e in o["errors"]],
            report=list(o["report"]),
            diff=None if d is None else Diff(**d),
            audit=o["audit"],
            used=None if u is None else Used(**u),
            exit_code=exit_code,
        )
