"""Validated PDF workspace operations. All exports are staged and never replace files."""
from __future__ import annotations

import os
import errno
import shutil
import tempfile
from pathlib import Path

from PIL import Image, ImageOps
from . import pdf

fitz = pdf.pymupdf


def selection(value, count):
    if value is None or value == "" or value == []:
        return list(range(1, count + 1))
    if isinstance(value, str):
        result = []
        for part in value.split(','):
            ends = part.strip().split('-')
            if len(ends) > 2 or not all(x.strip().isdigit() for x in ends):
                raise ValueError('Pages must look like 1-5, 8, 11-13')
            a, b = int(ends[0]), int(ends[-1])
            if not 1 <= a <= b <= count:
                raise ValueError(f'Page range must be between 1 and {count}')
            result.extend(range(a, b + 1))
    else:
        result = [int(x) for x in value]
    if not result or any(x < 1 or x > count for x in result):
        raise ValueError(f'Pages must be between 1 and {count}')
    return list(dict.fromkeys(result))


def roman(n):
    if not 1 <= n <= 3999:
        raise ValueError('Roman numbers must be between 1 and 3999')
    out = ''
    for value, label in [(1000,'m'),(900,'cm'),(500,'d'),(400,'cd'),(100,'c'),(90,'xc'),(50,'l'),(40,'xl'),(10,'x'),(9,'ix'),(5,'v'),(4,'iv'),(1,'i')]:
        while n >= value:
            out += label
            n -= value
    return out


def _publish(staged, desired):
    """Hard-link atomically; EEXIST retries cannot overwrite concurrent exports."""
    target = Path(desired)
    target.parent.mkdir(parents=True, exist_ok=True)
    index = 0
    while True:
        candidate = target if index == 0 else target.with_name(f'{target.stem}-{index}{target.suffix}')
        try:
            os.link(staged, candidate)
            return str(candidate)
        except FileExistsError:
            index += 1
        except OSError as error:
            if error.errno not in (errno.EPERM, errno.EOPNOTSUPP, errno.ENOSYS, errno.EXDEV):
                raise
            try:
                with open(candidate, 'xb') as output:
                    try:
                        with open(staged, 'rb') as source:
                            shutil.copyfileobj(source, output)
                    except BaseException:
                        output.close()
                        candidate.unlink()
                        raise
                return str(candidate)
            except FileExistsError:
                index += 1


def inspect(ctx, args):
    with pdf._open(str(args['src']), args.get('password')) as doc:
        return {'pages': doc.page_count}


def _stamp_page(page, args, number, total):
    mode = args.get('mode', 'watermark')
    size = float(args.get('fontsize', 54 if mode == 'watermark' else 12))
    opacity = float(args.get('opacity', .16 if mode == 'watermark' else 1))
    color = tuple(float(c) for c in args.get('color', [.35, .35, .35]))
    if not 1 <= size <= 300 or not 0 <= opacity <= 1 or len(color) != 3 or any(not 0 <= c <= 1 for c in color):
        raise ValueError('Invalid font size, opacity, or color')
    if args.get('number_style', 'arabic') not in ('arabic', 'roman'):
        raise ValueError('Number style must be arabic or roman')
    numeral = roman(number) if args.get('number_style') == 'roman' else str(number)
    text = str(args.get('text', 'DRAFT' if mode == 'watermark' else '{page}'))
    text = text.replace('{page}', numeral).replace('{total}', str(total))
    if args.get('show_total') and mode == 'numbers':
        text += f' / {total}'
    if not text.strip() or any(ord(c) > 255 or ord(c) < 32 for c in text):
        raise ValueError('Enter text supported by the PDF Latin font (single line)')
    position = args.get('position', 'center' if mode == 'watermark' else 'bottom-center')
    if position not in pdf._POSITIONS:
        raise ValueError('Invalid placement')
    fx, fy = pdf._POSITIONS[position]
    # Draw in displayed-page coordinates, then map to unrotated PDF coordinates.
    rotation = page.rotation
    page.set_rotation(0)
    try:
        visible = page.rect if rotation % 180 == 0 else fitz.Rect(0, 0, page.rect.height, page.rect.width)
        width = fitz.get_text_length(text, fontsize=size)
        x = visible.width * fx - (0 if fx < .1 else width if fx > .9 else width / 2)
        point = fitz.Point(x, visible.height * fy)
        angle = 45 if args.get('diagonal', mode == 'watermark') else 0
        # Morph supports arbitrary angles, unlike insert_text's rotate parameter.
        matrix = fitz.Matrix(angle - rotation)
        if rotation:
            transform = fitz.Matrix(rotation).pretranslate(0, 0)
            bounds = page.rect * transform
            point = fitz.Point(point.x + bounds.x0, point.y + bounds.y0) * ~transform
        page.insert_text(point, text, fontsize=size, color=color,
                         fill_opacity=opacity, morph=(point, matrix), overlay=True)
    finally:
        page.set_rotation(rotation)


def execute(ctx, args):
    action = args['action']
    src = str(args.get('src', ''))
    sources = args.get('srcs') or ([src] if src else [])
    if not sources:
        raise ValueError('Choose at least one input file')
    base = Path(sources[0])
    out_dir = Path(Path(args['dst']).parent if args.get('dst') else (args.get('out_dir') or base.parent))
    out_dir.mkdir(parents=True, exist_ok=True)
    outputs = []
    with tempfile.TemporaryDirectory(prefix='.garage-', dir=out_dir) as work:
        dst = str(Path(work) / 'result.pdf')
        if action == 'images':
            doc = fitz.open()
            try:
                mode = args.get('page_size', 'fit')
                if mode not in ('fit', 'a4', 'letter', 'legal'):
                    raise ValueError('Invalid paper size')
                margin = float(args.get('margin', 0))
                if margin < 0:
                    raise ValueError('Margin cannot be negative')
                for i, path in enumerate(sources):
                    ctx.step(i / len(sources) * 90, 'Adding image')
                    with Image.open(path) as original:
                        image = ImageOps.exif_transpose(original).convert('RGB')
                        import io
                        stream = io.BytesIO()
                        image.save(stream, format='PNG')
                        rect = fitz.Rect(0, 0, image.width, image.height) if mode == 'fit' else fitz.paper_rect(mode)
                        if mode != 'fit' and margin * 2 >= min(rect.width, rect.height):
                            raise ValueError('Margins leave no space for the image')
                        page = doc.new_page(width=rect.width, height=rect.height)
                        inset = 0 if mode == 'fit' else margin
                        page.insert_image(fitz.Rect(inset, inset, rect.width-inset, rect.height-inset), stream=stream.getvalue())
                result_pages = doc.page_count
                doc.save(dst, garbage=4, deflate=True)
            finally:
                doc.close()
        else:
            with pdf._open(src, args.get('source_password', args.get('password'))) as doc:
                count = doc.page_count
                chosen = selection(args.get('pages'), count)
                if action == 'export':
                    dpi = int(args.get('dpi', 150))
                    fmt = args.get('format', 'png')
                    if dpi not in (72,150,300) or fmt not in ('png','jpeg'):
                        raise ValueError('Invalid resolution or image format')
                    for i, p in enumerate(chosen):
                        ctx.step(i / len(chosen)*95, f'Page {p}')
                        file = str(Path(work) / f'{base.stem}-{p:03d}.{fmt}')
                        doc[p-1].get_pixmap(dpi=dpi, alpha=False).save(file)
                        outputs.append(file)
                elif action == 'organize':
                    rotations = args.get('rotate') or {}
                    shared_rotation = int(args.get('shared_rotation', 0))
                    if shared_rotation % 90:
                        raise ValueError('Rotation must be a multiple of 90')
                    if shared_rotation:
                        rotations = {str(p): shared_rotation for p in range(1, count+1)}
                    for key, degrees in rotations.items():
                        p = int(key)
                        if p < 1 or p > count or int(degrees) % 90:
                            raise ValueError('Invalid page rotation')
                        doc[p-1].set_rotation((doc[p-1].rotation + int(degrees)) % 360)
                    order = args.get('order')
                    if order is not None:
                        if not order:
                            raise ValueError('Cannot delete every page')
                        chosen = selection(order, count)
                    else:
                        drop = set(selection(args['delete'], count)) if args.get('delete') else set()
                        chosen = [p for p in range(1, count+1) if p not in drop]
                        if not chosen:
                            raise ValueError('Cannot delete every page')
                    doc.select([p-1 for p in chosen])
                elif action in ('watermark', 'numbers'):
                    start = int(args.get('number_start', 1))
                    if start < 1:
                        raise ValueError('Starting number must be positive')
                    for i, p in enumerate(chosen):
                        ctx.step(i / len(chosen)*90, f'Page {p}')
                        if not args.get('preview') or p == int(args.get('preview_page', 1)):
                            _stamp_page(doc[p-1], dict(args, mode=action), start+i, len(chosen))
                elif action not in ('protect', 'unlock'):
                    raise ValueError('Unknown PDF action')
                result_pages = doc.page_count
                if action != 'export':
                    save = dict(garbage=4, deflate=True, encryption=fitz.PDF_ENCRYPT_NONE)
                    if action == 'protect':
                        password = str(args.get('new_password', ''))
                        if len(password) < 4:
                            raise ValueError('New password must have at least 4 characters')
                        save.update(encryption=fitz.PDF_ENCRYPT_AES_256, user_pw=password,
                                    owner_pw=args.get('owner_password') or __import__('secrets').token_hex(16),
                                    permissions=int(args.get('permissions', -1)))
                    if args.get('preview'):
                        if action not in ('watermark', 'numbers'):
                            raise ValueError('Preview is only available for stamping')
                        p = int(args.get('preview_page', 1))
                        if not 1 <= p <= doc.page_count:
                            raise ValueError('Preview page out of range')
                        preview = str(Path(work) / 'preview.png')
                        doc[p-1].get_pixmap(dpi=96, alpha=False).save(preview)
                        outputs.append(preview)
                    else:
                        doc.save(dst, **save)
        ctx.check()
        if outputs:
            # Reserve a unique batch folder; publish only completed renders.
            root = out_dir
            if not args.get('preview'):
                index = 0
                while True:
                    root = out_dir / (base.stem + '-images' + (f'-{index}' if index else ''))
                    try:
                        root.mkdir()
                        break
                    except FileExistsError:
                        index += 1
            published = [_publish(f, root / Path(f).name) for f in outputs]
            return {'files': published, 'dst': published[0], 'dir': str(root)}
        target = args.get('dst') or str(out_dir / f'{base.stem}.{action}.pdf')
        published = _publish(dst, target)
        return {'dst': published, 'pages': result_pages, 'bytes': os.path.getsize(published)}
