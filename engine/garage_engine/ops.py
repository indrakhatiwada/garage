from __future__ import annotations

import threading
from dataclasses import dataclass
from typing import Any, Callable

Handler = Callable[["Ctx", dict[str, Any]], Any]


class Cancelled(Exception):
    """Raised when the UI asks a job to stop."""


@dataclass
class Ctx:
    job_id: str
    progress: Callable[[float, str], None]
    log: Callable[[str], None]
    cancel: threading.Event

    def check(self) -> None:
        if self.cancel.is_set():
            raise Cancelled("cancelled")

    def step(self, pct: float, note: str = "") -> None:
        self.check()
        self.progress(pct, note)


class Ops:
    def __init__(self) -> None:
        self._handlers: dict[str, Handler] = {}

    def register(self, name: str, handler: Handler) -> None:
        self._handlers[name] = handler

    def lookup(self, name: str) -> Handler:
        try:
            return self._handlers[name]
        except KeyError:
            raise ValueError(f"unknown op: {name}") from None
