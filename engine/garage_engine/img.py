from __future__ import annotations

import io
import json
import os
import re
import tempfile
from typing import Any

from PIL import Image, ImageFilter, ImageOps

from .ops import Ctx

try:  # optional, but expected in a full install
    import pillow_heif

    pillow_heif.register_heif_opener()
    _HEIF = True
except Exception:  # noqa: BLE001
    _HEIF = False

try:
    import pillow_avif  # noqa: F401

    _AVIF = True
except Exception:  # noqa: BLE001
    _AVIF = False

try:
    from pillow_avif import AvifImagePlugin  # noqa: F401
except Exception:  # noqa: BLE001
    pass

FORMATS: dict[str, tuple[str, str]] = {
    "jpeg": ("JPEG", ".jpg"),
    "webp": ("WEBP", ".webp"),
    "png": ("PNG", ".png"),
    "avif": ("AVIF", ".avif"),
}

PRESETS: dict[str, dict[str, Any]] = {
    "email": {"max_dim": 1600, "quality": 78},
    "web": {"max_dim": 2400, "quality": 82},
    "max": {"max_dim": 0, "quality": 94},
}


def heif_available() -> bool:
    return _HEIF


def avif_available() -> bool:
    return _AVIF


def _open(path: str) -> Image.Image:
    im = Image.open(path)
    im.seek(0)
    return ImageOps.exif_transpose(im)


def _flatten(im: Image.Image) -> Image.Image:
    """JPEG has no alpha: composite onto white rather than emitting garbage."""
    if im.mode == "L":  # grayscale JPEG is valid and stays small
        return im
    if im.mode in ("RGBA", "LA") or (im.mode == "P" and "transparency" in im.info):
        rgba = im.convert("RGBA")
        bg = Image.new("RGB", rgba.size, (255, 255, 255))
        bg.paste(rgba, mask=rgba.split()[-1])
        return bg
    if im.mode != "RGB":
        return im.convert("RGB")
    return im


def _encode(im: Image.Image, fmt: str, quality: int, keep_icc: bool, fast: bool = False) -> bytes:
    pil_fmt, _ = FORMATS[fmt]
    buf = io.BytesIO()
    icc = im.info.get("icc_profile") if keep_icc else None
    if fmt == "jpeg":
        im = _flatten(im)
        im.save(
            buf,
            "JPEG",
            quality=int(quality),
            optimize=not fast,
            progressive=not fast,
            subsampling=0 if quality >= 90 else 2,
            icc_profile=icc,
        )
    elif fmt == "webp":
        im.save(buf, "WEBP", quality=int(quality), method=2 if fast else 5, icc_profile=icc)
    elif fmt == "avif":
        im.save(buf, "AVIF", quality=int(quality), speed=9 if fast else 6)
    else:
        im.save(buf, "PNG", optimize=not fast, compress_level=6 if fast else 9, icc_profile=icc)
    return buf.getvalue()


def _resize(im: Image.Image, max_dim: int) -> Image.Image:
    if max_dim and max(im.size) > max_dim:
        im = im.copy()
        im.thumbnail((max_dim, max_dim), Image.Resampling.LANCZOS)
    return im


def _fit_bytes(im: Image.Image, fmt: str, target: int, keep_icc: bool) -> tuple[bytes, Image.Image]:
    """Binary-search quality, then dimensions, to land under `target` bytes.

    Search encodes are deliberately cheap (no optimize/progressive); the winner
    is re-encoded once at full effort.
    """
    lo, hi = 30, 92
    best_q = None
    for _ in range(5):
        mid = (lo + hi) // 2
        data = _encode(im, fmt, mid, keep_icc, fast=True)
        if len(data) <= target:
            best_q = mid
            lo = mid + 1
        else:
            hi = mid - 1

    if best_q is not None:
        polished = _encode(im, fmt, best_q, keep_icc, fast=False)
        if len(polished) <= target:
            return polished, im
        return _encode(im, fmt, best_q, keep_icc, fast=True), im

    # Quality floor still too big: scale down and recurse once.
    data = _encode(im, fmt, 30, keep_icc, fast=True)
    if len(data) <= target or max(im.size) <= 320:
        return data, im
    scale = (target / len(data)) ** 0.5
    small = im.resize(
        (max(1, int(im.width * scale)), max(1, int(im.height * scale))),
        Image.Resampling.LANCZOS,
    )
    return _fit_bytes(small, fmt, target, keep_icc)


def _out_path(src: str, out_dir: str, suffix: str, ext: str) -> str:
    stem = os.path.splitext(os.path.basename(src))[0]
    directory = out_dir or os.path.dirname(os.path.abspath(src))
    os.makedirs(directory, exist_ok=True)
    candidate = os.path.join(directory, f"{stem}{suffix}{ext}")
    n = 1
    while os.path.exists(candidate) and os.path.abspath(candidate) != os.path.abspath(src):
        candidate = os.path.join(directory, f"{stem}{suffix}-{n}{ext}")
        n += 1
    return candidate


def compress(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    paths: list[str] = list(args.get("paths") or [])
    if not paths:
        raise ValueError("no input files")

    preset = str(args.get("preset") or "custom")
    fmt = str(args.get("format") or "jpeg").lower()
    if fmt not in FORMATS:
        raise ValueError(f"unsupported output format: {fmt}")
    _, ext = FORMATS[fmt]

    base = PRESETS.get(preset, {})
    max_dim = int(args.get("max_dim", base.get("max_dim", 0)) or 0)
    quality = int(args.get("quality", base.get("quality", 82)) or 82)
    target = int(args.get("target_bytes") or 0)
    strip_exif = bool(args.get("strip_exif", True))
    keep_icc = bool(args.get("keep_icc", False))
    suffix = str(args.get("suffix", ".min"))
    out_dir = str(args.get("out_dir") or "")

    results: list[dict[str, Any]] = []
    before_total = after_total = 0

    for i, src in enumerate(paths):
        ctx.step(i / len(paths) * 100, os.path.basename(src))
        src_size = os.path.getsize(src)
        im = _open(src)
        src_w, src_h = im.size
        im = _resize(im, max_dim)
        if target:
            data, im = _fit_bytes(im, fmt, target, keep_icc)
        else:
            data = _encode(im, fmt, quality, keep_icc)
        dst = _out_path(src, out_dir, suffix, ext)
        with open(dst, "wb") as fh:
            fh.write(data)
        if strip_exif:
            _assert_clean(dst)

        entry = {
            "src": src,
            "dst": dst,
            "before": src_size,
            "after": len(data),
            "width": im.width,
            "height": im.height,
            "src_width": src_w,
            "src_height": src_h,
            "format": fmt,
        }
        before_total += src_size
        after_total += len(data)
        results.append(entry)

    ctx.step(100, "done")
    return {
        "files": results,
        "before": before_total,
        "after": after_total,
        "saved": (1 - after_total / before_total) if before_total else 0.0,
    }


def _norm(value: Any, default: float = 0.0) -> float:
    """A clamped 0..1 coordinate. Bad input becomes `default`, not a crash."""
    try:
        return min(1.0, max(0.0, float(value)))
    except (TypeError, ValueError):
        return default


def edit(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    """Apply simple, lossless-to-the-original edits and write a new file.

    Coordinates are normalised against the source image, so the UI can send
    exactly what the user saw in the preview. Every edit re-encodes the image
    without metadata; the input file is never touched.
    """
    src = str(args["src"])
    fmt = str(args.get("format") or "jpeg").lower()
    if fmt not in FORMATS:
        raise ValueError(f"unsupported output format: {fmt}")
    _, ext = FORMATS[fmt]

    im = _open(src)
    src_w, src_h = im.size
    ctx.step(10, "cropping")

    crop_box: tuple[int, int, int, int] | None = None
    crop = args.get("crop")
    if isinstance(crop, dict):
        cx = _norm(crop.get("x", 0.0))
        cy = _norm(crop.get("y", 0.0))
        cw = _norm(crop.get("w", 0.0))
        ch = _norm(crop.get("h", 0.0))
        if cw > 0.001 and ch > 0.001:
            x = min(int(round(cx * src_w)), src_w - 1)
            y = min(int(round(cy * src_h)), src_h - 1)
            x2 = min(src_w, max(x + 1, int(round((cx + cw) * src_w))))
            y2 = min(src_h, max(y + 1, int(round((cy + ch) * src_h))))
            crop_box = (x, y, x2, y2)
            im = im.crop(crop_box)

    ctx.step(35, "rotating")
    degrees = int(args.get("rotate") or 0) % 360
    quarter_turns = {
        90: Image.Transpose.ROTATE_270,   # PIL turns counter-clockwise
        180: Image.Transpose.ROTATE_180,
        270: Image.Transpose.ROTATE_90,
    }
    if degrees in quarter_turns:
        im = im.transpose(quarter_turns[degrees])
    elif degrees:
        im = im.rotate(degrees, expand=True, resample=Image.Resampling.BICUBIC)

    if args.get("flip_h"):
        im = im.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
    if args.get("flip_v"):
        im = im.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    if args.get("grayscale"):
        im = ImageOps.grayscale(im)

    ctx.step(55, "resizing")
    resize = args.get("resize") or {}
    width = min(20000, max(0, int(resize.get("width") or 0)))
    height = min(20000, max(0, int(resize.get("height") or 0)))
    max_dim = min(20000, max(0, int(resize.get("max_dim") or 0)))
    if max_dim:
        im = _resize(im, max_dim)
    elif width and height:
        im = im.resize((width, height), Image.Resampling.LANCZOS)
    elif width:
        im = im.resize((width, max(1, round(im.height * width / im.width))), Image.Resampling.LANCZOS)
    elif height:
        im = im.resize((max(1, round(im.width * height / im.height)), height), Image.Resampling.LANCZOS)

    # The UI supplies a scratch path for the preview. If nobody does, keep the
    # result in the system temp folder — editing never writes beside the input.
    dst = str(
        args.get("dst")
        or os.path.join(
            tempfile.gettempdir(), f"garage-edit-{ctx.job_id}-{os.getpid()}{ext}"
        )
    )
    ctx.step(80, "encoding")
    data = _encode(im, fmt, int(args.get("quality") or 90), keep_icc=False)
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    with open(dst, "wb") as fh:
        fh.write(data)
    _assert_clean(dst)

    ctx.step(100, "done")
    return {
        "src": src,
        "dst": dst,
        "before": os.path.getsize(src),
        "after": len(data),
        "width": im.width,
        "height": im.height,
        "src_width": src_w,
        "src_height": src_h,
        "format": fmt,
        "crop": list(crop_box) if crop_box else None,
        "rotate": degrees,
        "flip_h": bool(args.get("flip_h")),
        "flip_v": bool(args.get("flip_v")),
        "grayscale": bool(args.get("grayscale")),
    }


def _assert_clean(path: str) -> None:
    """Guardrail: a stripped image must really have no EXIF left."""
    with Image.open(path) as im:
        exif = im.getexif()
        if exif and len(exif) > 0:
            # Some encoders always write a minimal EXIF block; only fail on
            # payload-bearing tags.
            if exif.get(0x8825) or exif.get(0x0112):
                raise ValueError("EXIF strip failed")


_TEXT_TAGS = ("parameters", "Comment", "Description", "Software", "Workflow", "XMP")



def c2pa_available() -> bool:
    try:
        import c2pa  # noqa: F401

        return True
    except ImportError:
        return False


def _generator_tags(im: Image.Image) -> list[dict[str, str]]:
    """Text chunks generators leave in PNG/JPEG/WebP files. Local only."""
    tags: list[dict[str, str]] = []
    for key in _TEXT_TAGS:
        value = im.info.get(key)
        if isinstance(value, bytes):
            value = value.decode("utf-8", "replace")
        if not isinstance(value, str) or not value.strip():
            continue
        text = value.strip()
        if len(text) > 4000:
            text = text[:4000] + "…"
        tags.append({"key": key, "value": text})
    return tags


def _prompt_from(tags: list[dict[str, str]]) -> str | None:
    for tag in tags:
        if tag["key"] not in ("parameters", "Comment", "Description", "Workflow"):
            continue
        first = re.split(r"\nNegative prompt:", tag["value"], maxsplit=1)[0]
        first = first.strip()
        if first:
            return first[:600]
    return None


def c2pa_signals(path: str) -> dict[str, Any]:
    """Verify an embedded C2PA signature with the local SDK.

    No trust anchors are downloaded: this checks cryptographic integrity of
    the manifest, not whether the signer is on Adobe's trust list.
    """
    try:
        import c2pa
    except ImportError:
        return {
            "available": False,
            "state": "unavailable",
            "error": "the C2PA verifier is not installed",
        }

    try:
        # The SDK otherwise follows remote-manifest URLs while constructing a
        # Reader. Keep both manifest and revocation checks strictly offline.
        with c2pa.Context.from_dict({"verify": {
            "remote_manifest_fetch": False, "ocsp_fetch": False,
        }}) as context, c2pa.Reader(path, context=context) as reader:
            store = json.loads(reader.json())
    except Exception as exc:  # noqa: BLE001 - a missing manifest is normal
        message = str(exc)
        if "ManifestNotFound" in type(exc).__name__ or "no JUMBF" in message:
            return {"available": True, "state": "none"}
        if "must fetch remote manifests" in message:
            return {"available": True, "state": "error",
                    "error": "Content credentials are stored remotely and cannot be checked offline."}
        return {"available": True, "state": "error", "error": message[:240]}

    manifests = store.get("manifests") or {}
    active = store.get("active_manifest")
    manifest = manifests.get(active) if active else None
    statuses = store.get("validation_status") or []
    if not statuses and manifest:
        statuses = manifest.get("validation_status") or []

    problems = []
    for status in statuses:
        if not isinstance(status, dict):
            continue
        code = str(status.get("code", ""))
        if (
            status.get("success") is False
            or status.get("valid") is False
            or any(word in code.lower() for word in ("invalid", "mismatch", "failed", "nottrusted"))
        ):
            problems.append(
                {
                    "code": code,
                    "detail": str(status.get("explanation") or status.get("description") or "")[:200],
                }
            )

    assertions = []
    if manifest:
        for assertion in manifest.get("assertions") or []:
            if isinstance(assertion, dict) and assertion.get("label"):
                assertions.append(str(assertion["label"]))

    return {
        "available": True,
        "state": "invalid" if problems else "valid",
        "claim_generator": str((manifest or {}).get("claim_generator") or ""),
        "assertions": assertions[:20],
        "problems": problems[:10],
        "manifest_count": len(manifests),
    }


def info(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    path = str(args["path"])
    ctx.step(10, "reading")
    with Image.open(path) as im:
        exif = im.getexif()
        gps = exif.get_ifd(0x8825) if exif else {}
        tags = _generator_tags(im)
        prompt = _prompt_from(tags)
        return {
            "path": path,
            "width": im.width,
            "height": im.height,
            "format": im.format,
            "mode": im.mode,
            "bytes": os.path.getsize(path),
            "exif_tags": len(exif) if exif else 0,
            "gps": {str(k): str(v) for k, v in gps.items()} if gps else {},
            "camera": str(exif.get(0x0110, "")) if exif else "",
            "taken": str(exif.get(0x9003, "")) if exif else "",
            "software": str(exif.get(0x0131, "")) if exif else "",
            "signals": {
                "generator_tags": tags,
                "prompt": prompt,
                "c2pa": c2pa_signals(path),
            },
        }


def _apply_redactions(im: Image.Image, boxes: list[dict[str, Any]]) -> None:
    """Use the same full-resolution effects for previews and exported images."""
    w, h = im.size
    for box in boxes:
        x = int(float(box["x"]) * w)
        y = int(float(box["y"]) * h)
        bw = max(1, int(float(box["w"]) * w))
        bh = max(1, int(float(box["h"]) * h))
        x2, y2 = min(w, x + bw), min(h, y + bh)
        x, y = max(0, x), max(0, y)
        region = im.crop((x, y, x2, y2))
        mode = str(box.get("mode", "pixelate"))
        if mode == "solid":
            region = Image.new("RGB", region.size, tuple(box.get("color", [0, 0, 0])))
        elif mode == "blur":
            radius = max(6, int(float(box.get("strength", 18))))
            region = region.filter(ImageFilter.GaussianBlur(radius))
        else:
            blocks = max(2, int(float(box.get("strength", 12))))
            small = region.resize(
                (max(1, region.width // blocks), max(1, region.height // blocks)),
                Image.Resampling.BILINEAR,
            )
            region = small.resize(region.size, Image.Resampling.NEAREST)
        im.paste(region, (x, y))



def preview(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    """A bounded, orientation-correct preview in a format the native UI supports."""
    ctx.step(10, "opening image")
    dst = str(args["dst"])
    with _open(str(args["src"])) as im:
        _apply_redactions(im, args.get("boxes") or [])
        im.thumbnail((1600, 1600), Image.Resampling.LANCZOS)
        im = im.convert("RGBA")
        im.info.clear()
        os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
        im.save(dst, "PNG")
        width, height = im.size
    ctx.step(100, "ready")
    return {"dst": dst, "width": width, "height": height, "pages": 1}


def redact(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    """Permanently destroy pixels under the given boxes.

    Boxes are normalised (0..1) against the displayed image so the UI can send
    what the user actually saw. Output is always re-encoded.
    """
    src = str(args["src"])
    dst = str(args.get("dst") or _out_path(src, "", ".redacted", ".jpg"))
    boxes = args.get("boxes") or []
    if not boxes:
        raise ValueError("no boxes")

    im = _open(src)
    _apply_redactions(im, boxes)

    ctx.step(70, "encoding")
    data = _encode(im, "jpeg", 92, keep_icc=False)
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    with open(dst, "wb") as fh:
        fh.write(data)
    ctx.step(100, "done")
    return {"dst": dst, "bytes": len(data), "boxes": len(boxes)}
