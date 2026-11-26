"""Keep the frozen engine small.

PyMuPDF 2.x ships ~50 MB of AI document-layout ONNX models plus its C headers.
Garage only renders pages, extracts text, and redacts; it never calls the
layout API. The upstream hook collects all of those data files unconditionally,
so filter them out here.
"""

from pathlib import PurePath

from PyInstaller.utils.hooks import collect_data_files

_datas = collect_data_files("pymupdf")
datas = [
    (src, dest)
    for src, dest in _datas
    if not any(part in PurePath(src).parts for part in ("layout", "mupdf-devel"))
]
