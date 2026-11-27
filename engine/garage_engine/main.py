"""Garage engine: a headless worker that owns every byte that touches disk.

Protocol: newline-delimited JSON on stdin/stdout.

    -> {"id":"j1","op":"image.compress","args":{...}}
    <- {"id":"j1","event":"progress","pct":40,"note":"3/8"}
    <- {"id":"j1","event":"done","result":{...}}
    <- {"id":"j1","event":"error","code":"...","message":"..."}

No sockets. No telemetry. No network imports anywhere in this package.
"""

from __future__ import annotations

import json
import os
import sys
import threading
import traceback
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Callable

from . import cleanup, img, ocr, pdf
from .ops import Ctx, Ops

VERSION = "0.1.1"

_write_lock = threading.Lock()


def emit(payload: dict[str, Any]) -> None:
    line = json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
    with _write_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def features() -> dict[str, bool]:
    return {
        "pdf": pdf.available(),
        "ocr": ocr.available(),
        "ghostscript": pdf.ghostscript_available(),
        "heif": img.heif_available(),
        "avif": img.avif_available(),
        "c2pa": img.c2pa_available(),
    }


def build_ops() -> Ops:
    from . import pdf_tools
    ops = Ops()
    ops.register("pdf.workspace", pdf_tools.execute)
    ops.register("pdf.inspect", pdf_tools.inspect)
    ops.register("ping", lambda ctx, a: {"version": VERSION, "features": features(), "pid": os.getpid()})

    # --- images -------------------------------------------------------
    ops.register("image.compress", img.compress)
    ops.register("image.edit", img.edit)
    ops.register("image.info", img.info)
    ops.register("image.preview", img.preview)
    ops.register("image.redact", img.redact)

    # --- pdf ----------------------------------------------------------
    ops.register("pdf.info", pdf.info)
    ops.register("pdf.compress", pdf.compress)
    ops.register("pdf.pages", pdf.pages)
    ops.register("pdf.merge", pdf.merge)
    ops.register("pdf.split", pdf.split)
    ops.register("pdf.extract", pdf.extract)
    ops.register("pdf.images_to_pdf", pdf.images_to_pdf)
    ops.register("pdf.to_images", pdf.to_images)
    ops.register("pdf.render", pdf.render)
    ops.register("pdf.redact", pdf.redact)
    ops.register("pdf.stamp", pdf.stamp)
    ops.register("pdf.protect", pdf.protect)
    ops.register("pdf.unlock", pdf.unlock)

    # --- ocr ----------------------------------------------------------
    ops.register("ocr.text", ocr.text)
    ops.register("ocr.searchable_pdf", ocr.searchable_pdf)
    ops.register("ocr.scan_dates", ocr.scan_dates)

    # --- cleanup ------------------------------------------------------
    ops.register("cleanup.enhance", cleanup.enhance)

    return ops


class Cancels:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._flags: dict[str, threading.Event] = {}

    def get(self, job_id: str) -> threading.Event:
        with self._lock:
            return self._flags.setdefault(job_id, threading.Event())

    def cancel(self, job_id: str) -> None:
        self.get(job_id).set()

    def drop(self, job_id: str) -> None:
        with self._lock:
            self._flags.pop(job_id, None)


def main() -> None:
    ops = build_ops()
    cancels = Cancels()
    pool = ThreadPoolExecutor(max_workers=2, thread_name_prefix="job")

    def run(req: dict[str, Any]) -> None:
        job_id = str(req.get("id", "?"))
        op = str(req.get("op", ""))
        args = req.get("args") or {}
        cancel = cancels.get(job_id)

        def progress(pct: float, note: str = "") -> None:
            emit({"id": job_id, "event": "progress", "pct": round(float(pct), 1), "note": note})

        def log(message: str) -> None:
            emit({"id": job_id, "event": "log", "message": message})

        ctx = Ctx(job_id=job_id, progress=progress, log=log, cancel=cancel)
        try:
            handler = ops.lookup(op)
            result = handler(ctx, args)
            emit({"id": job_id, "event": "done", "result": result})
        except Exception as exc:  # noqa: BLE001 - the boundary must not crash
            code, message = classify(exc)
            emit(
                {
                    "id": job_id,
                    "event": "error",
                    "code": code,
                    "message": message,
                    "detail": traceback.format_exc(limit=4) if os.environ.get("GARAGE_DEBUG") else None,
                }
            )
        finally:
            cancels.drop(job_id)

    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as exc:
            emit({"id": None, "event": "error", "code": "bad_json", "message": str(exc)})
            continue

        op = req.get("op")
        if op == "cancel":
            cancels.cancel(str(req.get("id")))
            continue
        if op == "shutdown":
            break
        if op == "ping":
            # Cheap liveness check, answered inline so startup feels instant.
            emit({"id": req.get("id"), "event": "done", "result": {"version": VERSION, "features": features()}})
            continue
        pool.submit(run, req)

    pool.shutdown(wait=False, cancel_futures=True)


def classify(exc: Exception) -> tuple[str, str]:
    """Map an exception to a stable code the UI can act on."""
    name = type(exc).__name__
    message = str(exc).strip() or name
    lowered = message.lower()
    if "password" in lowered or "encrypted" in lowered or "authenticate" in lowered:
        return "encrypted", message
    if isinstance(exc, FileNotFoundError):
        return "not_found", message
    if isinstance(exc, PermissionError):
        return "permission", message
    if isinstance(exc, ValueError):
        return "invalid", message
    if "unsupported" in lowered or "cannot" in lowered:
        return "unsupported", message
    return "internal", f"{name}: {message}"


if __name__ == "__main__":
    main()
