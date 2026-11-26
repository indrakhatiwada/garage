from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
from typing import Any, Iterator

from .ops import Ctx

try:
    import pymupdf
except ImportError:
    import fitz as pymupdf  # type: ignore[no-redef]

from PIL import Image

DEVANAGARI_DIGITS = str.maketrans("०१२३४५६७८९", "0123456789")

# 2082-08-06, 06/08/2082, 2025.11.22, with optional spaces around separators.
DATE_RE = re.compile(
    r"(?<![\d०-९])([0-9०-९]{1,4})\s*[-/.]\s*([0-9०-९]{1,2})\s*[-/.]\s*([0-9०-९]{1,4})(?![\d०-९])"
)

IMAGE_EXT = {".png", ".jpg", ".jpeg", ".webp", ".tif", ".tiff", ".bmp", ".heic", ".avif"}

# Tesseract slows down badly above this; a 300dpi A4 scan is ~3500px and
# upscaling past the source resolution only costs time.
MAX_OCR_PX = 2600


def available() -> bool:
    return shutil.which("tesseract") is not None


def languages() -> list[str]:
    if not available():
        return []
    try:
        out = subprocess.run(
            ["tesseract", "--list-langs"], capture_output=True, text=True, timeout=20
        )
        return [line.strip() for line in out.stdout.splitlines()[1:] if line.strip()]
    except Exception:  # noqa: BLE001
        return []


def _is_image(path: str) -> bool:
    return os.path.splitext(path)[1].lower() in IMAGE_EXT


def _tesseract_tsv(image: str, lang: str, psm: int = 6) -> list[dict[str, Any]]:
    out = subprocess.run(
        ["tesseract", image, "stdout", "-l", lang, "--psm", str(psm), "tsv"],
        capture_output=True,
        text=True,
        timeout=300,
    )
    if out.returncode != 0:
        raise RuntimeError(out.stderr.strip() or "tesseract failed")
    rows: list[dict[str, Any]] = []
    for line in out.stdout.splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) < 12:
            continue
        text = cols[11].strip()
        if not text:
            continue
        try:
            conf = float(cols[10])
        except ValueError:
            continue
        if conf < 0:
            continue
        rows.append(
            {
                "text": text,
                "conf": conf,
                "left": int(cols[6]),
                "top": int(cols[7]),
                "width": int(cols[8]),
                "height": int(cols[9]),
                "line": (int(cols[2]), int(cols[3]), int(cols[4])),
            }
        )
    return rows


def _page_raster(page, dpi: int, path: str):
    """Render a page, clamped so we never invent resolution we don't have."""
    longest_pt = max(page.rect.width, page.rect.height, 1.0)
    effective = min(float(dpi), MAX_OCR_PX / (longest_pt / 72.0))
    effective = max(96, int(effective))
    pix = page.get_pixmap(dpi=effective, alpha=False)
    pix.save(path)
    return pix


def _open_pdf(src: str, password: str | None = None):
    doc = pymupdf.open(src)
    if doc.needs_pass and not (password and doc.authenticate(password)):
        raise ValueError("password required")
    if doc.page_count == 0:
        raise ValueError("no pages")
    return doc


def _sources(src: str, password: str | None, dpi: int, tmp: str) -> Iterator[dict[str, Any]]:
    """Yield (image path, pixel size, normalised page size) for each page."""
    if _is_image(src):
        with Image.open(src) as im:
            width, height = im.size
        yield {"image": src, "px": (width, height), "page": 1, "index": 0}
        return
    doc = _open_pdf(src, password)
    try:
        for pno in range(doc.page_count):
            image = os.path.join(tmp, f"p{pno}.png")
            pix = _page_raster(doc[pno], dpi, image)
            yield {"image": image, "px": (pix.width, pix.height), "page": pno + 1, "index": pno}
    finally:
        doc.close()


def text(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    src = str(args["src"])
    lang = best_lang(args.get("lang"))
    if not available():
        raise ValueError("tesseract not installed")

    if not _is_image(src):
        doc = _open_pdf(src, args.get("password"))
        native = "\n".join(doc[p].get_text("text") for p in range(doc.page_count)).strip()
        count = doc.page_count
        doc.close()
        if native and not args.get("force"):
            return {"text": native, "source": "pdf-text", "pages": count}

    tmp = tempfile.mkdtemp(prefix="garage-ocr-")
    parts: list[str] = []
    try:
        pages = list(_sources(src, args.get("password"), 300, tmp))
        total = max(1, len(pages))
        for i, page in enumerate(pages):
            ctx.step(10 + i / total * 85, f"page {page['page']}")
            rows = _tesseract_tsv(page["image"], lang)
            parts.append(" ".join(r["text"] for r in rows))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return {"text": "\n".join(parts), "source": "ocr", "pages": len(parts)}


def searchable_pdf(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    """Keep the original scan and add an invisible, selectable text layer."""
    if not available():
        raise ValueError("tesseract not installed")
    src = str(args["src"])
    dst = str(args.get("dst") or os.path.join(
        str(args.get("out_dir") or os.path.dirname(src)), _stem(src) + ".ocr.pdf"
    ))
    lang = best_lang(args.get("lang"))
    dpi = int(args.get("dpi", 300))
    doc = _open_pdf(src, args.get("password"))
    tmp = tempfile.mkdtemp(prefix="garage-ocr-")
    added = 0

    try:
        for pno in range(doc.page_count):
            ctx.step(pno / doc.page_count * 95, f"page {pno + 1}/{doc.page_count}")
            page = doc[pno]
            if len(page.get_text("text").strip()) > 40 and not args.get("force"):
                continue  # already has a text layer
            image = os.path.join(tmp, f"p{pno}.png")
            pix = _page_raster(page, dpi, image)
            rows = _tesseract_tsv(image, lang)
            scale = page.rect.width / max(1, pix.width)
            for row in rows:
                word = row["text"]
                if any(ord(ch) > 127 for ch in word):
                    continue  # Latin-only layer; needs a shaping font otherwise
                size = max(4.0, row["height"] * scale * 0.82)
                x = row["left"] * scale
                baseline = (row["top"] + row["height"]) * scale
                try:
                    page.insert_text(
                        pymupdf.Point(x, baseline),
                        word,
                        fontsize=size,
                        fontname="helv",
                        render_mode=3,  # invisible but selectable
                        overlay=True,
                    )
                    added += 1
                except Exception:  # noqa: BLE001
                    continue
        doc.save(dst, garbage=4, deflate=True)
    finally:
        doc.close()
        shutil.rmtree(tmp, ignore_errors=True)

    ctx.step(100, "done")
    return {"dst": dst, "words": added, "bytes": os.path.getsize(dst)}


def _stem(path: str) -> str:
    return os.path.splitext(os.path.basename(path))[0]


def _classify(a: str, b: str, c: str) -> tuple[str, tuple[int, int, int]] | None:
    """Return (system, (y, m, d)) for a numeric date triple, or None.

    1975-2100 overlaps both calendars, so those years come back as
    "ambiguous" and the UI offers both readings rather than guessing.
    """
    try:
        x, y, z = int(a), int(b), int(c)
    except ValueError:
        return None

    def valid(day: int, month: int) -> bool:
        return 1 <= month <= 12 and 1 <= day <= 32

    def system_for(year: int) -> str:
        return "ambiguous" if 1975 <= year <= 2100 else "AD"

    if len(a) == 4 and 1918 <= x <= 2100 and valid(z, y):
        return (system_for(x), (x, y, z))
    if len(c) == 4 and 1918 <= z <= 2100 and valid(x, y):
        return (system_for(z), (z, y, x))
    return None


def best_lang(requested: str | None) -> str:
    """Fall back to what is actually installed instead of failing."""
    installed = set(languages())
    if requested:
        wanted = [p for p in requested.split("+") if p in installed]
        if wanted:
            return "+".join(wanted)
    if "nep" in installed and "eng" in installed:
        return "eng+nep"
    if "eng" in installed:
        return "eng"
    return next(iter(installed), "eng")


def scan_dates(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    if not available():
        raise ValueError("tesseract not installed")
    src = str(args["src"])
    lang = best_lang(args.get("lang"))
    dpi = int(args.get("dpi", 300))
    tmp = tempfile.mkdtemp(prefix="garage-dates-")
    found: list[dict[str, Any]] = []
    seen: set[tuple] = set()
    total = 1

    try:
        pages = list(_sources(src, args.get("password"), dpi, tmp))
        total = max(1, len(pages))
        for i, source in enumerate(pages):
            ctx.step(i / total * 95, f"page {source['page']}/{total}")
            rows = _tesseract_tsv(source["image"], lang)
            width, height = source["px"]

            # Join words per line so "2082 - 08 - 06" survives OCR spacing.
            lines: dict[tuple[int, int, int], list[dict[str, Any]]] = {}
            for row in rows:
                lines.setdefault(row["line"], []).append(row)

            for words in lines.values():
                words.sort(key=lambda r: r["left"])
                joined = " ".join(w["text"] for w in words).translate(DEVANAGARI_DIGITS)
                for match in DATE_RE.finditer(joined):
                    triple = _classify(*match.groups())
                    if not triple:
                        continue
                    system, (y, m, d) = triple
                    first = words[0]
                    right = max(w["left"] + w["width"] for w in words)
                    bottom = max(w["top"] + w["height"] for w in words)
                    key = (source["page"], y, m, d, match.group(0))
                    if key in seen:
                        continue
                    seen.add(key)
                    found.append(
                        {
                            "page": source["page"],
                            "text": match.group(0),
                            "system": system,
                            "year": y,
                            "month": m,
                            "day": d,
                            "x": first["left"] / width,
                            "y": first["top"] / height,
                            "w": (right - first["left"]) / width,
                            "h": (bottom - first["top"]) / height,
                            "conf": min(w["conf"] for w in words),
                        }
                    )
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    ctx.step(100, "done")
    return {"dates": found, "count": len(found), "languages": languages()}
