"""Visual redaction alignment checks: PYTHONPATH=engine .venv/bin/python -m unittest discover -s tests -p test_redact.py"""
import hashlib
import tempfile
import threading
import unittest
from pathlib import Path

import pymupdf
from PIL import Image, ImageChops, ImageDraw, ImageStat

from garage_engine import img, pdf
from garage_engine.main import build_ops
from garage_engine.ops import Ctx


def ctx():
    return Ctx("redact-test", lambda *_: None, lambda *_: None, threading.Event())


class VisualRedactionTests(unittest.TestCase):
    def test_redacted_image_preview_matches_export_for_every_style(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            src = root / "striped.png"
            image = Image.new("RGB", (400, 200), "white")
            draw = ImageDraw.Draw(image)
            for x in range(0, 400, 8):
                draw.rectangle((x, 0, x + 3, 199), fill="black")
            image.save(src)
            original = src.read_bytes()
            for mode in ("solid", "blur", "pixelate"):
                with self.subTest(mode=mode):
                    boxes = [{"x": .25, "y": .25, "w": .5, "h": .5, "mode": mode, "strength": 12}]
                    preview, exported = root / f"{mode}.png", root / f"{mode}.jpg"
                    img.preview(ctx(), {"src": str(src), "dst": str(preview), "boxes": boxes})
                    img.redact(ctx(), {"src": str(src), "dst": str(exported), "boxes": boxes})
                    with Image.open(preview) as rendered, Image.open(exported) as result:
                        rendered = rendered.convert("RGB")
                        self.assertIsNotNone(ImageChops.difference(image, rendered).getbbox())
                        # The actual effect must match, allowing JPEG compression differences.
                        difference = ImageChops.difference(rendered.crop((110, 60, 290, 140)), result.crop((110, 60, 290, 140)))
                        self.assertLess(max(ImageStat.Stat(difference).mean), 10)
                        self.assertEqual(rendered.getpixel((20, 20)), image.getpixel((20, 20)))
                        if mode == "solid":
                            self.assertEqual(rendered.getpixel((200, 100)), (0, 0, 0))
            self.assertEqual(src.read_bytes(), original)

    def test_image_preview_and_export_use_the_same_exif_orientation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            src, preview, dst = root / "photo.jpg", root / "preview.png", root / "redacted.jpg"
            image = Image.new("RGB", (240, 120), "white")
            ImageDraw.Draw(image).rectangle((0, 0, 119, 119), fill="red")
            exif = image.getexif()
            exif[274] = 6  # Rotated 90 degrees by a viewer.
            image.save(src, exif=exif, quality=100)
            original = src.read_bytes()
            result = build_ops().lookup("image.preview")(ctx(), {"src": str(src), "dst": str(preview)})
            self.assertEqual((result["width"], result["height"]), (120, 240))
            with Image.open(preview) as rendered:
                self.assertEqual(rendered.size, (120, 240))
                self.assertFalse(rendered.getexif())
                self.assertGreater(rendered.getpixel((48, 48))[0], 200)
            img.redact(ctx(), {"src": str(src), "dst": str(dst), "boxes": [
                {"x": .1, "y": .1, "w": .6, "h": .2, "mode": "solid"},
            ]})
            with Image.open(dst) as redacted:
                self.assertEqual(redacted.size, (120, 240))
                self.assertLess(max(redacted.getpixel((48, 48))), 20)
                self.assertGreater(redacted.getpixel((108, 96))[0], 200)
            self.assertEqual(src.read_bytes(), original)

    def test_large_preview_is_bounded_without_changing_the_source(self):
        with tempfile.TemporaryDirectory() as directory:
            src, dst = Path(directory) / "large.png", Path(directory) / "preview.png"
            Image.new("RGB", (3200, 800), "white").save(src)
            result = img.preview(ctx(), {"src": str(src), "dst": str(dst)})
            self.assertEqual((result["width"], result["height"]), (1600, 400))
            with Image.open(src) as original:
                self.assertEqual(original.size, (3200, 800))

    def test_pdf_preview_coordinates_match_export_on_every_rotation_and_crop(self):
        for rotation in (0, 90, 180, 270):
            for cropped in (False, True):
                with self.subTest(rotation=rotation, cropped=cropped), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    src, preview, dst = root / "source.pdf", root / "preview.png", root / "redacted.pdf"
                    with pymupdf.open() as doc:
                        for _ in range(2):
                            page = doc.new_page(width=400, height=300)
                            page.draw_rect(page.rect, fill=(.8, .9, .7), color=None)
                            page.insert_text((40, 60), "Private document")
                            if cropped:
                                page.set_cropbox(pymupdf.Rect(20, 20, 380, 280))
                            page.set_rotation(rotation)
                        doc.save(src)
                    original = hashlib.sha256(src.read_bytes()).digest()
                    result = pdf.render(ctx(), {"src": str(src), "dst": str(preview), "page": 2})
                    self.assertEqual(result["pages"], 2)
                    boxes = [
                        {"page": 2, "x": .2, "y": .3, "w": .3, "h": .2, "color": [0, 0, 0]},
                    ]
                    # Preview the same boxes on both pages: only page 2 is affected.
                    for page_no in (1, 2):
                        result_preview = root / f"result-page-{page_no}.png"
                        pdf.render(ctx(), {"src": str(src), "dst": str(result_preview), "page": page_no, "boxes": boxes})
                        with Image.open(result_preview) as rendered:
                            marked = rendered.getpixel((int(rendered.width * .35), int(rendered.height * .4)))
                            if page_no == 2:
                                self.assertLess(max(marked), 10)
                            else:
                                self.assertGreater(min(marked), 100)
                    pdf.redact(ctx(), {"src": str(src), "dst": str(dst), "flatten": True, "boxes": boxes})
                    with pymupdf.open(dst) as doc:
                        self.assertEqual(doc.page_count, 2)
                        self.assertFalse(doc[1].get_text().strip())
                        for page_no in (0, 1):
                            pixmap = doc[page_no].get_pixmap(alpha=False)
                            marked = pixmap.pixel(int(pixmap.width * .35), int(pixmap.height * .4))
                            outside = pixmap.pixel(int(pixmap.width * .85), int(pixmap.height * .55))
                            self.assertGreater(min(outside), 100)
                            if page_no == 1:
                                self.assertLess(max(marked), 10)
                            else:
                                self.assertGreater(min(marked), 100)
                    self.assertEqual(hashlib.sha256(src.read_bytes()).digest(), original)


if __name__ == "__main__":
    unittest.main()
