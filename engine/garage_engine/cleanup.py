from __future__ import annotations

import os
from typing import Any

import numpy as np
from PIL import Image, ImageFilter, ImageOps

from .ops import Ctx


def _gray(im: Image.Image) -> Image.Image:
    if im.mode != "L":
        return im.convert("L")
    return im


def _estimate_skew(im: Image.Image, limit: float = 8.0, step: float = 0.25) -> float:
    """Projection-profile search: the angle where text rows become sharpest."""
    small = im.copy()
    small.thumbnail((900, 900), Image.Resampling.BILINEAR)
    gray = np.asarray(_gray(small), dtype=np.float32)
    ink = 255.0 - gray  # text becomes bright
    ink[ink < 40] = 0.0
    best_angle, best_score = 0.0, -1.0
    angle = -limit
    while angle <= limit:
        rotated = small.rotate(angle, resample=Image.Resampling.BILINEAR, fillcolor=255)
        rows = (255.0 - np.asarray(_gray(rotated), dtype=np.float32)).sum(axis=1)
        score = float(((rows[1:] - rows[:-1]) ** 2).sum())
        if score > best_score:
            best_angle, best_score = angle, score
        angle += step
    return best_angle


def _otsu(gray: np.ndarray) -> int:
    hist = np.bincount(gray.ravel(), minlength=256).astype(np.float64)
    total = hist.sum()
    if total == 0:
        return 128
    omega = np.cumsum(hist)
    mu = np.cumsum(hist * np.arange(256))
    mu_t = mu[-1]
    denom = omega * (total - omega)
    denom[denom == 0] = 1
    sigma = (mu_t * omega - mu) ** 2 / denom
    return int(np.argmax(sigma))


def _autocrop(im: Image.Image, margin: int = 12) -> Image.Image:
    gray = np.asarray(_gray(im))
    mask = gray < 235
    if not mask.any():
        return im
    rows = np.where(mask.any(axis=1))[0]
    cols = np.where(mask.any(axis=0))[0]
    top = max(0, rows[0] - margin)
    bottom = min(im.height, rows[-1] + margin)
    left = max(0, cols[0] - margin)
    right = min(im.width, cols[-1] + margin)
    if right - left < 16 or bottom - top < 16:
        return im
    return im.crop((left, top, right, bottom))


def enhance(ctx: Ctx, args: dict[str, Any]) -> dict[str, Any]:
    """Turn a phone photo of a document into something that looks scanned."""
    src = str(args["src"])
    dst = str(args.get("dst") or os.path.join(os.path.dirname(src), _stem(src) + ".clean.png"))
    deskew = bool(args.get("deskew", True))
    denoise = bool(args.get("denoise", True))
    threshold = bool(args.get("threshold", True))
    autocrop = bool(args.get("autocrop", True))
    upscale = float(args.get("upscale", 1.0) or 1.0)

    ctx.step(5, "loading")
    im = ImageOps.exif_transpose(Image.open(src))
    im = _gray(im)

    if upscale > 1.0:
        im = im.resize(
            (int(im.width * upscale), int(im.height * upscale)), Image.Resampling.LANCZOS
        )

    angle = 0.0
    if deskew:
        ctx.step(20, "estimating skew")
        angle = _estimate_skew(im)
        if abs(angle) > 0.15:
            im = im.rotate(
                angle, resample=Image.Resampling.BICUBIC, expand=True, fillcolor=255
            )

    if denoise:
        ctx.step(45, "denoise")
        im = im.filter(ImageFilter.MedianFilter(3))

    if threshold:
        ctx.step(65, "threshold")
        gray = np.asarray(im)
        cutoff = _otsu(gray)
        # Bias towards paper-white: anything close to the cutoff becomes 255.
        arr = np.where(gray > max(0, cutoff - 8), 255, gray).astype(np.uint8)
        im = Image.fromarray(arr, mode="L")

    if autocrop:
        ctx.step(80, "crop")
        im = _autocrop(im)

    ctx.step(90, "saving")
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    if dst.lower().endswith((".jpg", ".jpeg")):
        im.convert("RGB").save(dst, "JPEG", quality=92, optimize=True)
    else:
        im.save(dst, "PNG", optimize=True, compress_level=9)

    ctx.step(100, "done")
    return {
        "dst": dst,
        "bytes": os.path.getsize(dst),
        "width": im.width,
        "height": im.height,
        "skew": round(angle, 2),
    }


def _stem(path: str) -> str:
    return os.path.splitext(os.path.basename(path))[0]
