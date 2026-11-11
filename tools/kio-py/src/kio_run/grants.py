"""Grants: what a run may reach. The application builds them; never build them from model output."""

from __future__ import annotations

import os
from dataclasses import dataclass, field


@dataclass
class Limits:
    fuel: int | None = None
    timeout: int | None = None  # seconds
    memory: int | None = None  # MB
    stack: int | None = None  # KB
    calls: int | None = None
    io: int | None = None  # MB

    def flags(self) -> list[str]:
        out: list[str] = []
        for name in ("fuel", "timeout", "memory", "stack", "calls", "io"):
            v = getattr(self, name)
            if v is not None:
                out += [f"--{name}", str(v)]
        return out


@dataclass
class Grants:
    """A typed builder over kio's grant flags. The name is the `main` parameter's name.

    The application fixes grants in code. Paths, hosts and header values must not
    come from the model: a model may propose, the app chooses.
    """

    _flags: list[tuple[str, str]] = field(default_factory=list)
    _file: str | None = None
    _limits: Limits | None = None

    @classmethod
    def from_file(cls, path: str | os.PathLike) -> "Grants":
        """Use a grants file (one flag per line) the operator wrote."""
        return cls(_file=os.fspath(path))

    def dir(self, name: str, path: str | os.PathLike, ro: bool = False) -> "Grants":
        self._flags.append(("--dir", f"{name}={os.fspath(path)}{':ro' if ro else ''}"))
        return self

    def http(
        self,
        name: str,
        host: str,
        methods: list[str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> "Grants":
        """`headers` maps a header name to the environment variable holding its value;
        kio injects it, the program never sees it."""
        self._flags.append(("--http", f"{name}={host}"))
        if methods:
            self._flags.append(("--http-methods", f"{name}={','.join(methods)}"))
        for header, var in (headers or {}).items():
            self._flags.append(("--http-header", f"{name}={header}:env:{var}"))
        return self

    def kv(self, name: str, path: str | os.PathLike = ":memory:", ro: bool = False) -> "Grants":
        self._flags.append(("--kv", f"{name}={os.fspath(path)}{':ro' if ro else ''}"))
        return self

    def db(self, name: str, path: str | os.PathLike = ":memory:", ro: bool = False) -> "Grants":
        self._flags.append(("--db", f"{name}={os.fspath(path)}{':ro' if ro else ''}"))
        return self

    def rng(self, name: str, seed: int | None = None) -> "Grants":
        self._flags.append(("--rng", name if seed is None else f"{name}=seed:{seed}"))
        return self

    def clock(self, name: str, fixed: int | str | None = None) -> "Grants":
        self._flags.append(("--clock", name if fixed is None else f"{name}=fixed:{fixed}"))
        return self

    def log(self, name: str, path: str | os.PathLike) -> "Grants":
        """`path` "-" is stderr, which the wrapper discards."""
        self._flags.append(("--log", f"{name}={os.fspath(path)}"))
        return self

    def limits(self, limits: Limits) -> "Grants":
        self._limits = limits
        return self

    def args(self, limits: Limits | None = None) -> list[str]:
        """The command-line flags; `limits` given here override the builder's."""
        out = ["--grants", self._file] if self._file else []
        for flag, value in self._flags:
            out += [flag, value]
        merged = Limits(**vars(self._limits)) if self._limits else Limits()
        for k, v in vars(limits or Limits()).items():
            if v is not None:
                setattr(merged, k, v)
        return out + merged.flags()

    def file_text(self) -> str:
        """The builder's grants as a grants file, for `kio check --grants`."""
        return "".join(f"{flag} {value}\n" for flag, value in self._flags)
