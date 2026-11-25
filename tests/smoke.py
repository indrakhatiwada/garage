"""End-to-end checks for the engine. Run: PYTHONPATH=engine .venv/bin/python tests/smoke.py"""

from __future__ import annotations

import os
import shutil
import tempfile
import threading
import time
import traceback

from garage_engine import cleanup, img, ocr, pdf
from garage_engine.ops import Ctx

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIX = os.path.join(ROOT, "tests", "fixtures")
OUT = tempfile.mkdtemp(prefix="garage-smoke-")

PASS, FAIL = [], []


def ctx() -> Ctx:
    return Ctx(
        job_id="test",
        progress=lambda pct, note="": None,
        log=lambda message: None,
        cancel=threading.Event(),
    )


def check(name: str, fn):
    started = time.time()
    try:
        result = fn()
        PASS.append(name)
        print(f"  ok   {name}  ({time.time() - started:.1f}s)")
        return result
    except Exception as exc:  # noqa: BLE001
        FAIL.append((name, exc))
        print(f"  FAIL {name}  ({time.time() - started:.1f}s): {exc}")
        traceback.print_exc()
        return None



def generator_signals():
    """PNG text chunks and EXIF Software are surfaced as AI signals."""
    from PIL import Image, PngImagePlugin

    path = os.path.join(OUT, "generated.png")
    image = Image.new("RGB", (320, 200), (24, 48, 32))
    pnginfo = PngImagePlugin.PngInfo()
    pnginfo.add_text(
        "parameters", "a red car, cinematic lighting\nNegative prompt: blurry"
    )
    exif = image.getexif()
    exif[0x0131] = "Test Generator 1.0"
    image.save(path, pnginfo=pnginfo, exif=exif.tobytes())

    result = img.info(ctx(), {"path": path})
    assert result["software"] == "Test Generator 1.0", result["software"]
    tags = {tag["key"]: tag["value"] for tag in result["signals"]["generator_tags"]}
    assert "parameters" in tags, tags
    assert result["signals"]["prompt"].startswith("a red car"), result["signals"]["prompt"]
    assert result["signals"]["c2pa"]["state"] == "none"
    return result


def c2pa_signals_check():
    """The remote-only fixture stays offline instead of fetching its manifest."""
    signed = os.path.join(FIX, "signed.jpg")
    if not os.path.exists(signed):
        return None
    good = img.info(ctx(), {"path": signed})["signals"]["c2pa"]
    assert good["state"] == "error", good
    assert "stored remotely" in good["error"], good

    tampered = os.path.join(OUT, "tampered.jpg")
    data = bytearray(open(signed, "rb").read())
    data[len(data) // 2] ^= 0x01
    with open(tampered, "wb") as handle:
        handle.write(data)
    bad = img.info(ctx(), {"path": tampered})["signals"]["c2pa"]
    assert bad["state"] in ("invalid", "error"), bad
    return {"remote_manifest": good["state"], "tampered": bad["state"]}


def main() -> int:
    print(f"fixtures: {FIX}\nworkdir:  {OUT}\n")

    photo = os.path.join(FIX, "photo.jpg")
    scan = os.path.join(FIX, "scan.png")
    scans = os.path.join(FIX, "scans.pdf")

    # --- images ---------------------------------------------------------
    meta = check("image.info reads GPS", lambda: img.info(ctx(), {"path": photo}))
    if meta:
        assert meta["gps"], "expected GPS tags on the fixture"
        assert meta["width"] == 3000
        assert meta["signals"]["c2pa"]["state"] in {
            "valid", "invalid", "none", "error", "unavailable"
        }
        assert isinstance(meta["signals"]["generator_tags"], list)

    check("image.info reads generator tags", generator_signals)
    check("image.info keeps remote C2PA credentials offline", c2pa_signals_check)

    def compress_photo():
        result = img.compress(
            ctx(),
            {
                "paths": [photo],
                "preset": "email",
                "format": "jpeg",
                "strip_exif": True,
                "out_dir": OUT,
            },
        )
        entry = result["files"][0]
        assert entry["after"] < entry["before"], "photo should shrink"
        assert max(entry["width"], entry["height"]) <= 1600, entry
        with img.Image.open(entry["dst"]) as im:
            exif = im.getexif()
            assert not exif.get(0x8825), "GPS must be gone"
        return entry

    compressed = check("image.compress strips GPS + resizes", compress_photo)

    def target_size():
        result = img.compress(
            ctx(),
            {
                "paths": [photo],
                "format": "webp",
                "target_bytes": 80_000,
                "out_dir": OUT,
                "suffix": ".small",
            },
        )
        entry = result["files"][0]
        assert entry["after"] <= 80_000, entry["after"]
        return entry

    check("image.compress hits a byte target", target_size)

    def redact_image():
        result = img.redact(
            ctx(),
            {
                "src": photo,
                "dst": os.path.join(OUT, "photo.redacted.jpg"),
                "boxes": [{"x": 0.05, "y": 0.1, "w": 0.3, "h": 0.2, "mode": "pixelate"}],
            },
        )
        assert os.path.getsize(result["dst"]) > 0
        return result

    check("image.redact writes a flattened copy", redact_image)

    def edit_image():
        from PIL import Image

        result = img.edit(
            ctx(),
            {
                "src": photo,
                "dst": os.path.join(OUT, "photo.edited.jpg"),
                "crop": {"x": 0.1, "y": 0.2, "w": 0.5, "h": 0.4},
                "rotate": 90,
                "flip_h": True,
                "grayscale": True,
                "resize": {"max_dim": 600},
            },
        )
        assert result["crop"], result
        assert result["rotate"] == 90, result
        assert max(result["width"], result["height"]) == 600, result
        with Image.open(result["dst"]) as im:
            assert im.mode == "L", im.mode
            assert not im.getexif().get(0x8825), "GPS must be gone"
        return result

    check("image.edit crops, rotates, flips, resizes", edit_image)

    def edit_without_destination_stays_in_temp():
        result = img.edit(ctx(), {"src": photo, "rotate": 90})
        assert os.path.dirname(os.path.abspath(result["dst"])) == tempfile.gettempdir(), result
        assert not os.path.exists(os.path.join(FIX, "photo.edited.jpg")), "no silent output next to the input"
        return result

    check("image.edit with no destination writes only a temp preview", edit_without_destination_stays_in_temp)

    def edit_resize_width():
        from PIL import Image

        result = img.edit(
            ctx(),
            {
                "src": photo,
                "dst": os.path.join(OUT, "photo.wide.webp"),
                "format": "webp",
                "resize": {"width": 240},
            },
        )
        assert result["width"] == 240, result
        with Image.open(result["dst"]) as im:
            assert im.height > 0
        return result

    check("image.edit keeps aspect with one resize dimension", edit_resize_width)

    # --- pdf ------------------------------------------------------------
    info = check("pdf.info", lambda: pdf.info(ctx(), {"path": scans}))
    if info:
        assert info["pages"] == 6, info["pages"]
        assert not info["has_text"], "fixture is image-only"
        assert len(info["images"]) == 6, info["images"]

    def compress_pdf():
        result = pdf.compress(
            ctx(),
            {
                "src": scans,
                "dst": os.path.join(OUT, "scans.min.pdf"),
                "dpi": 150,
                "quality": 72,
                "ghostscript": "off",
            },
        )
        assert result["after"] < result["before"] * 0.9, result
        assert len(result["images"]) == 6, result["images"]
        assert pdf.pymupdf.open(result["dst"]).page_count == 6
        return result

    compressed_pdf = check("pdf.compress shrinks a scan", compress_pdf)

    def render_page():
        result = pdf.render(
            ctx(),
            {"src": scans, "dst": os.path.join(OUT, "page1.png"), "page": 1, "dpi": 96},
        )
        assert os.path.getsize(result["dst"]) > 1000
        return result

    check("pdf.render", render_page)

    def page_ops():
        result = pdf.pages(
            ctx(),
            {
                "src": scans,
                "dst": os.path.join(OUT, "reordered.pdf"),
                "order": [6, 1, 2, 3, 4, 5],
                "rotate": {"1": 90},
                "delete": [],
            },
        )
        doc = pdf.pymupdf.open(result["dst"])
        assert doc.page_count == 6
        assert doc[1].rotation == 90, doc[1].rotation
        return result

    check("pdf.pages reorder + rotate", page_ops)

    def merge_split():
        merged = pdf.merge(
            ctx(),
            {
                "srcs": [scans, os.path.join(FIX, "resume.pdf")],
                "dst": os.path.join(OUT, "merged.pdf"),
            },
        )
        assert merged["pages"] > 6
        parts = pdf.split(
            ctx(),
            {"src": merged["dst"], "out_dir": OUT, "ranges": [[1, 2], [3, 4]]},
        )
        assert len(parts["files"]) == 2
        assert pdf.pymupdf.open(parts["files"][0]["dst"]).page_count == 2
        return merged

    check("pdf.merge + pdf.split", merge_split)

    def stamp():
        result = pdf.stamp(
            ctx(),
            {
                "src": scans,
                "dst": os.path.join(OUT, "numbered.pdf"),
                "mode": "numbers",
                "text": "Page {page} of {total}",
                "position": "bottom-center",
            },
        )
        text = pdf.pymupdf.open(result["dst"])[0].get_text()
        assert "Page 1 of 6" in text, text[:200]
        return result

    check("pdf.stamp page numbers", stamp)

    def redact_pdf():
        result = pdf.redact(
            ctx(),
            {
                "src": os.path.join(OUT, "numbered.pdf"),
                "dst": os.path.join(OUT, "redacted.pdf"),
                "boxes": [{"page": 1, "x": 0.05, "y": 0.8, "w": 0.4, "h": 0.1}],
                "flatten": True,
            },
        )
        text = pdf.pymupdf.open(result["dst"])[0].get_text().strip()
        assert text == "", f"flattened redaction must leave no text, got {text!r}"
        return result

    check("pdf.redact flatten removes text", redact_pdf)

    def protect_unlock():
        locked = pdf.protect(
            ctx(),
            {"src": scans, "dst": os.path.join(OUT, "locked.pdf"), "password": "hunter22"},
        )
        doc = pdf.pymupdf.open(locked["dst"])
        assert doc.needs_pass, "should require a password"
        doc.close()
        unlocked = pdf.unlock(
            ctx(),
            {
                "src": locked["dst"],
                "dst": os.path.join(OUT, "unlocked.pdf"),
                "password": "hunter22",
            },
        )
        doc = pdf.pymupdf.open(unlocked["dst"])
        assert not doc.needs_pass
        doc.close()
        return unlocked

    check("pdf.protect + pdf.unlock", protect_unlock)

    # --- ocr ------------------------------------------------------------
    if ocr.available():
        dates = check("ocr.scan_dates finds both calendars", lambda: ocr.scan_dates(
            ctx(), {"src": scan, "lang": "eng"}
        ))
        if dates:
            found = {(d["system"], d["year"], d["month"], d["day"]) for d in dates["dates"]}
            print(f"       dates: {sorted(found)}")
            assert ("BS", 2082, 8, 6) in found or ("ambiguous", 2082, 8, 6) in found, found
            assert ("AD", 2025, 11, 22) in found or ("ambiguous", 2025, 11, 22) in found, found

        def searchable():
            two = pdf.extract(
                ctx(),
                {"src": scans, "dst": os.path.join(OUT, "two.pdf"), "pages": [1, 2]},
            )
            result = ocr.searchable_pdf(
                ctx(),
                {"src": two["dst"], "dst": os.path.join(OUT, "searchable.pdf"), "lang": "eng"},
            )
            assert result["words"] > 10, result
            text = pdf.pymupdf.open(result["dst"])[0].get_text()
            assert "statement" in text.lower(), text[:200]
            return result

        check("ocr.searchable_pdf adds a text layer", searchable)
    else:
        print("  skip OCR (tesseract not installed)")

    # --- cleanup --------------------------------------------------------
    def enhance():
        result = cleanup.enhance(
            ctx(),
            {
                "src": scan,
                "dst": os.path.join(OUT, "clean.png"),
                "deskew": True,
                "denoise": True,
                "threshold": True,
                "autocrop": True,
            },
        )
        assert -5.0 < result["skew"] < -1.0, f"expected ~-3.4deg, got {result['skew']}"
        return result

    check("cleanup.enhance deskews a scan", enhance)

    print(f"\n{len(PASS)} passed, {len(FAIL)} failed")
    if not FAIL:
        print(f"artifacts kept in {OUT}")
    else:
        for name, exc in FAIL:
            print(f"  - {name}: {exc}")
    return 1 if FAIL else 0


if __name__ == "__main__":
    raise SystemExit(main())
