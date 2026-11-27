from __future__ import annotations

import io
import os
import shutil
import subprocess
import tempfile
from typing import Any

from .ops import Ctx

try:
    import pymupdf
except ImportError:  # older installs expose the legacy name
    import fitz as pymupdf  # type: ignore[no-redef]

from PIL import Image


def available() -> bool:
    return pymupdf is not None


def ghostscript_available() -> bool:
    return shutil.which("gs") is not None


def _open(path: str, password: str | None = None):
    doc = pymupdf.open(path)
    if doc.needs_pass:
        if not password or not doc.authenticate(password):
            doc.close()
            raise ValueError("password required")
    return doc


def _tmp(dst: str) -> str:
    directory = os.path.dirname(os.path.abspath(dst))
    os.makedirs(directory, exist_ok=True)
    fd, path = tempfile.mkstemp(suffix=".pdf", dir=directory)
    os.close(fd)
    return path


def _size(path: str) -> int:
    try:
        return os.path.getsize(path)
    except OSError:
        return 0


def _human(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024.0
    return f"{n:.1f} GB"


# --------------------------------------------------------------------------- info


def info(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    path = str(args["path"])
    ctx.step(5, "opening")
    doc = pymupdf.open(path)
    encrypted = bool(doc.needs_pass)
    result: dict[str, Any] = {
        "path": path,
        "bytes": _size(path),
        "encrypted": encrypted,
        "pages": 0 if encrypted else doc.page_count,
        "metadata": {},
        "images": [],
        "fonts": [],
        "has_text": False,
        "text_chars": 0,
    }
    if encrypted:
        return result

    meta = doc.metadata or {}
    result["metadata"] = {k: v for k, v in meta.items() if v}
    text_chars = 0
    images: list[dict[str, Any]] = []
    seen: set[int] = set()
    fonts: set[str] = set()

    for pno in range(doc.page_count):
        ctx.step(5 + pno / max(1, doc.page_count) * 90, f"page {pno + 1}/{doc.page_count}")
        page = doc[pno]
        if text_chars < 20000:
            text_chars += len(page.get_text("text").strip())
        for item in page.get_images(full=True):
            xref = item[0]
            if xref in seen or len(images) >= 400:
                continue
            seen.add(xref)
            raw = doc.extract_image(xref)
            images.append(
                {
                    "xref": xref,
                    "page": pno + 1,
                    "width": raw.get("width", 0),
                    "height": raw.get("height", 0),
                    "bytes": len(raw.get("image", b"")),
                    "ext": raw.get("ext", "?"),
                }
            )
        for font in page.get_fonts(full=True):
            fonts.add(str(font[3]))

    result["images"] = images
    result["fonts"] = sorted(fonts)
    result["text_chars"] = text_chars
    result["has_text"] = text_chars > 40
    result["image_bytes"] = sum(i["bytes"] for i in images)
    doc.close()
    ctx.step(100, "done")
    return result


# ----------------------------------------------------------------------- compress


def _image_usage(doc) -> dict[int, dict[str, Any]]:
    """Map each image xref to every page placement that uses it."""
    usage: dict[int, dict[str, Any]] = {}
    for pno in range(doc.page_count):
        page = doc[pno]
        for item in page.get_images(full=True):
            xref = item[0]
            entry = usage.setdefault(xref, {"pages": [], "rect": None})
            try:
                rects = page.get_image_rects(xref)
            except Exception:  # noqa: BLE001
                rects = []
            for rect in rects:
                entry["pages"].append(pno)
                if entry["rect"] is None or abs(rect.width * rect.height) > abs(
                    entry["rect"].width * entry["rect"].height
                ):
                    entry["rect"] = rect
    return usage


def _encode_jpeg(im: Image.Image, quality: int, grayscale: bool) -> bytes:
    if im.mode in ("RGBA", "LA") or (im.mode == "P" and "transparency" in im.info):
        im = im.convert("RGBA")
        bg = Image.new("RGB", im.size, (255, 255, 255))
        bg.paste(im, mask=im.split()[-1])
        im = bg
    elif im.mode == "CMYK":
        im = im.convert("RGB")
    elif im.mode not in ("RGB", "L"):
        im = im.convert("RGB")
    if grayscale and im.mode != "L":
        im = im.convert("L")
    buf = io.BytesIO()
    im.save(buf, "JPEG", quality=quality, optimize=True, progressive=True, subsampling=2)
    return buf.getvalue()


def _downsample(doc, ctx: Ctx, dpi: int, quality: int, grayscale: bool, log) -> list[dict[str, Any]]:
    usage = _image_usage(doc)
    report: list[dict[str, Any]] = []
    total = max(1, len(usage))
    for index, (xref, entry) in enumerate(sorted(usage.items())):
        ctx.step(index / total * 70, f"image {index + 1}/{total}")
        try:
            if doc.xref_get_key(xref, "ImageMask")[1] == "true":
                continue
            if doc.xref_get_key(xref, "SMask")[1] not in ("null", ""):
                continue  # keep alpha images untouched rather than flatten them
            rect = entry["rect"]
            raw = doc.extract_image(xref)
            width, height = int(raw.get("width", 0)), int(raw.get("height", 0))
            if width < 64 or height < 64 or not rect or not rect.width:
                continue
            effective = width / (rect.width / 72.0)
            if effective <= dpi * 1.05:
                continue
            target_w = max(1, round(rect.width / 72.0 * dpi))
            target_h = max(1, round(target_w * height / width))
            im = Image.open(io.BytesIO(raw["image"]))
            im = im.resize((target_w, target_h), Image.Resampling.LANCZOS)
            encoded = _encode_jpeg(im, quality, grayscale)
            if len(encoded) >= len(raw["image"]):
                continue
            placed = False
            for pno in sorted(set(entry["pages"])):
                try:
                    doc[pno].replace_image(xref, stream=encoded)
                    placed = True
                except Exception as exc:  # noqa: BLE001
                    log(f"image {xref} on page {pno + 1} skipped: {exc}")
            if not placed:
                continue
            report.append(
                {
                    "page": entry["pages"][0] + 1,
                    "xref": xref,
                    "before": len(raw["image"]),
                    "after": len(encoded),
                    "from": f"{width}x{height}",
                    "to": f"{target_w}x{target_h}",
                    "dpi": round(effective),
                }
            )
        except Exception as exc:  # noqa: BLE001 - one bad image must not kill the job
            log(f"image {xref} skipped: {exc}")
    return report


def _ghostscript(src: str, dst: str, level: str = "ebook") -> str | None:
    if not ghostscript_available():
        return None
    cmd = [
        "gs",
        "-sDEVICE=pdfwrite",
        "-dCompatibilityLevel=1.7",
        f"-dPDFSETTINGS=/{level}",
        "-dNOPAUSE",
        "-dQUIET",
        "-dBATCH",
        "-dDetectDuplicateImages=true",
        "-dCompressFonts=true",
        "-dSubsetFonts=true",
        f"-sOutputFile={dst}",
        src,
    ]
    try:
        subprocess.run(cmd, check=True, capture_output=True, timeout=600)
        return dst
    except Exception:  # noqa: BLE001
        return None


def compress(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    src = str(args["src"])
    dst = str(args.get("dst") or os.path.join(_out_dir(args, src), _stem(src) + ".min.pdf"))
    dpi = int(args.get("dpi", 150) or 150)
    quality = int(args.get("quality", 78) or 78)
    grayscale = bool(args.get("grayscale", False))
    subset_fonts = bool(args.get("subset_fonts", True))
    strip_meta = bool(args.get("strip_meta", True))
    use_gs = str(args.get("ghostscript", "auto"))
    before = _size(src)

    ctx.step(2, "opening")
    doc = _open(src, args.get("password"))
    notes: list[str] = []

    images = _downsample(doc, ctx, dpi, quality, grayscale, ctx.log)
    ctx.step(72, "fonts")

    if subset_fonts:
        try:
            doc.subset_fonts()
        except Exception as exc:  # noqa: BLE001
            notes.append(f"font subsetting skipped: {exc}")

    if strip_meta:
        try:
            doc.set_metadata({})
            doc.del_xml_metadata()
        except Exception:  # noqa: BLE001
            pass

    ctx.step(80, "writing")
    tmp = _tmp(dst)
    doc.save(
        tmp,
        garbage=4,
        clean=True,
        deflate=True,
        deflate_images=True,
        deflate_fonts=True,
        linear=False,
    )
    doc.close()
    best = tmp

    if use_gs in ("auto", "on") and ghostscript_available():
        ctx.step(88, "ghostscript")
        gs_out = tmp + ".gs.pdf"
        candidate = _ghostscript(src, gs_out, "ebook" if dpi <= 150 else "printer")
        if candidate and _size(candidate) < _size(best):
            if best != candidate:
                os.unlink(best)
            best = candidate
            notes.append("ghostscript pass produced the smaller file")
        elif candidate:
            os.unlink(candidate)

    os.replace(best, dst)
    after = _size(dst)
    ctx.step(100, "done")

    return {
        "dst": dst,
        "before": before,
        "after": after,
        "saved": (1 - after / before) if before else 0.0,
        "images": images,
        "notes": notes,
        "before_human": _human(before),
        "after_human": _human(after),
    }


def _stem(path: str) -> str:
    return os.path.splitext(os.path.basename(path))[0]


def _out_dir(args: dict[str, Any], src: str) -> str:
    """Where derived files go: the caller's chosen folder when supplied."""
    return str(args.get("out_dir") or os.path.dirname(src))


# -------------------------------------------------------------------- page edits


def pages(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    from .pdf_tools import execute
    mapped = dict(args, action="organize")
    return execute(ctx, mapped)



def merge(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    srcs: list[str] = [str(p) for p in (args.get("srcs") or [])]
    if len(srcs) < 2:
        raise ValueError("merge needs at least two files")
    dst = str(args["dst"])
    out = pymupdf.open()
    for i, path in enumerate(srcs):
        ctx.step(i / len(srcs) * 90, os.path.basename(path))
        with pymupdf.open(path) as part:
            if part.needs_pass:
                raise ValueError(f"encrypted: {path}")
            out.insert_pdf(part)
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    out.save(dst, garbage=4, deflate=True)
    count = out.page_count
    out.close()
    ctx.step(100, "done")
    return {"dst": dst, "pages": count, "bytes": _size(dst)}


def split(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    src = str(args["src"])
    out_dir = str(args.get("out_dir") or os.path.dirname(src))
    os.makedirs(out_dir, exist_ok=True)
    doc = _open(src, args.get("password"))
    ranges = args.get("ranges")
    if not ranges:
        every = int(args.get("every") or 1)
        ranges = [
            [i + 1, min(i + every, doc.page_count)]
            for i in range(0, doc.page_count, every)
        ]
    outputs: list[dict[str, Any]] = []
    base = _stem(src)
    for i, pair in enumerate(ranges):
        start, end = int(pair[0]), int(pair[1])
        part = pymupdf.open()
        part.insert_pdf(doc, from_page=start - 1, to_page=end - 1)
        name = f"{base}-{start:03d}-{end:03d}.pdf"
        path = os.path.join(out_dir, name)
        part.save(path, garbage=4, deflate=True)
        part.close()
        outputs.append({"dst": path, "pages": end - start + 1, "bytes": _size(path)})
        ctx.step((i + 1) / len(ranges) * 100, name)
    doc.close()
    return {"files": outputs}


def extract(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    src = str(args["src"])
    dst = str(args.get("dst") or os.path.join(_out_dir(args, src), _stem(src) + ".pages.pdf"))
    pages_list = [int(p) for p in (args.get("pages") or [])]
    if not pages_list:
        raise ValueError("no pages selected")
    doc = _open(src, args.get("password"))
    doc.select([p - 1 for p in pages_list if 0 < p <= doc.page_count])
    doc.save(dst, garbage=4, deflate=True)
    count = doc.page_count
    doc.close()
    ctx.step(100, "done")
    return {"dst": dst, "pages": count, "bytes": _size(dst)}


def images_to_pdf(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    from .pdf_tools import execute
    mapped = dict(args, action="images")
    return execute(ctx, mapped)



def to_images(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    from .pdf_tools import execute
    mapped = dict(args, action="export")
    return execute(ctx, mapped)



def _apply_redactions(page, items: list[dict[str, Any]]) -> None:
    """Apply boxes in displayed page coordinates for both preview and export."""
    width, height = page.rect.width, page.rect.height
    for box in items:
        rect = pymupdf.Rect(
            float(box["x"]) * width,
            float(box["y"]) * height,
            (float(box["x"]) + float(box["w"])) * width,
            (float(box["y"]) + float(box["h"])) * height,
        )
        # The preview includes page rotation; annotations use unrotated coordinates.
        rect = rect * page.derotation_matrix
        page.add_redact_annot(rect, fill=tuple(box.get("color", (0, 0, 0))))
    try:
        page.apply_redactions(
            images=pymupdf.PDF_REDACT_IMAGE_PIXELS,
            graphics=pymupdf.PDF_REDACT_LINE_ART_REMOVE_IF_TOUCHED,
            text=pymupdf.PDF_REDACT_TEXT_REMOVE,
        )
    except TypeError:
        page.apply_redactions()


def render(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    src = str(args["src"])
    dst = str(args["dst"])
    page_no = int(args.get("page", 1))
    dpi = int(args.get("dpi", 96))
    doc = _open(src, args.get("password"))
    if not (1 <= page_no <= doc.page_count):
        raise ValueError(f"page {page_no} out of range")
    page = doc[page_no - 1]
    if args.get("rotation"):
        page.set_rotation((page.rotation + int(args["rotation"])) % 360)
    boxes = [box for box in (args.get("boxes") or []) if int(box.get("page", 1)) == page_no]
    if boxes:
        _apply_redactions(page, boxes)
    pix = page.get_pixmap(dpi=dpi, alpha=False)
    pages = doc.page_count
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    pix.save(dst)
    doc.close()
    ctx.step(100, "done")
    return {"dst": dst, "width": pix.width, "height": pix.height, "pages": pages}


# --------------------------------------------------------------------- redaction


def redact(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    """Remove content, not just cover it.

    Boxes are normalised against each page. `flatten` rasterises the page so
    nothing recoverable survives underneath.
    """
    src = str(args["src"])
    dst = str(args.get("dst") or os.path.join(_out_dir(args, src), _stem(src) + ".redacted.pdf"))
    boxes = args.get("boxes") or []
    if not boxes:
        raise ValueError("no redaction boxes")
    flatten = bool(args.get("flatten", False))
    doc = _open(src, args.get("password"))

    by_page: dict[int, list[dict[str, Any]]] = {}
    for box in boxes:
        by_page.setdefault(int(box.get("page", 1)), []).append(box)

    for pno, items in by_page.items():
        if not (1 <= pno <= doc.page_count):
            continue
        page = doc[pno - 1]
        _apply_redactions(page, items)
        ctx.step(50, f"page {pno}")

    if flatten:
        # Rebuild the whole document from rasters. Nothing of the original
        # object graph, text layer, or metadata survives.
        raster = pymupdf.open()
        for pno in range(doc.page_count):
            ctx.step(50 + pno / max(1, doc.page_count) * 40, f"flatten {pno + 1}")
            page = doc[pno]
            pix = page.get_pixmap(dpi=200, alpha=False)
            fresh = raster.new_page(width=page.rect.width, height=page.rect.height)
            fresh.insert_image(fresh.rect, pixmap=pix)
        doc.close()
        raster.save(dst, garbage=4, deflate=True)
        raster.close()
    else:
        doc.set_metadata({})
        doc.del_xml_metadata()
        doc.save(dst, garbage=4, deflate=True)
        doc.close()

    ctx.step(100, "done")
    return {"dst": dst, "boxes": len(boxes), "bytes": _size(dst), "flattened": flatten}


# ----------------------------------------------------------------------- stamping


_POSITIONS = {
    "top-left": (0.08, 0.06),
    "top-center": (0.5, 0.06),
    "top-right": (0.92, 0.06),
    "middle-left": (0.08, 0.5),
    "center": (0.5, 0.5),
    "middle-right": (0.92, 0.5),
    "bottom-left": (0.08, 0.94),
    "bottom-center": (0.5, 0.94),
    "bottom-right": (0.92, 0.94),
}


def stamp(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    from .pdf_tools import execute
    mapped = dict(args, action="watermark")
    mapped["action"] = "numbers" if args.get("mode") == "numbers" else "watermark"
    mapped.setdefault("diagonal", args.get("mode") == "watermark")
    if not args.get("pages") and args.get("start_page"):
        with _open(str(args["src"]), args.get("password")) as doc:
            mapped["pages"] = f'{int(args["start_page"])}-{doc.page_count}'
    mapped.setdefault("text", "")
    mapped.setdefault("fontsize", 54 if args.get("mode") == "watermark" else 12)
    mapped.setdefault("opacity", .16 if args.get("mode") == "watermark" else 1)
    return execute(ctx, mapped)


# ---------------------------------------------------------------------- passwords


def protect(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    from .pdf_tools import execute
    mapped = dict(args, action="protect")
    mapped["source_password"] = args.get("source_password", args.get("password"))
    mapped["new_password"] = args.get("password", "")
    return execute(ctx, mapped)



def unlock(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    from .pdf_tools import execute
    mapped = dict(args, action="unlock")
    return execute(ctx, mapped)
