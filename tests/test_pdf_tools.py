"""Behavior checks for the offline PDF workspace."""
from pathlib import Path
import tempfile
import threading
from unittest.mock import patch
import errno
import unittest
from PIL import Image
from garage_engine import pdf_tools as tools
from garage_engine.ops import Ctx, Cancelled


class PdfToolsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.src = self.root / 'source.pdf'
        with tools.fitz.open() as doc:
            for i in range(4):
                page = doc.new_page(width=300, height=400)
                page.insert_text((30, 50), f'Original {i+1}')
            doc.save(self.src)
        self.original = self.src.read_bytes()
        self.cancel = threading.Event()
        self.ctx = Ctx('test', lambda *a: None, lambda *a: None, self.cancel)

    def tearDown(self):
        self.assertEqual(self.src.read_bytes(), self.original)
        self.tmp.cleanup()

    def run_op(self, action, **args):
        return tools.execute(self.ctx, dict(src=str(self.src), out_dir=str(self.root), action=action, **args))

    def test_compression_without_output_folder_preserves_original(self):
        from garage_engine import img
        source=self.root/'photo.jpg'
        Image.new('RGB',(180,120),'blue').save(source)
        original=source.read_bytes()
        args=dict(paths=[str(source)],preset='max',format='jpeg',strip_exif=True)
        first=img.compress(self.ctx,args)['files'][0]['dst']
        second=img.compress(self.ctx,args)['files'][0]['dst']
        self.assertEqual(Path(first).parent,self.root)
        self.assertNotEqual(first,second)
        self.assertNotEqual(first,str(source))
        self.assertEqual(source.read_bytes(),original)
        self.assertTrue(Path(first).is_file())

    def test_organize_and_collision(self):
        result = self.run_op('organize', order=[4, 2], rotate={'4': 90})
        with tools.fitz.open(result['dst']) as doc:
            self.assertEqual(len(doc), 2)
            self.assertEqual(doc[0].rotation, 90)
            self.assertIn('Original 4', doc[0].get_text())
        self.assertNotEqual(result['dst'], self.run_op('organize', order=[4, 2])['dst'])
        same = self.run_op('organize', dst=str(self.src))
        self.assertNotEqual(same['dst'], str(self.src))
        with self.assertRaises(ValueError): self.run_op('organize', order=[])
        with self.assertRaises(ValueError): self.run_op('organize', delete='1-4')
        with self.assertRaises(ValueError): self.run_op('organize', rotate={'1': 45})
        rotated = self.run_op('organize', delete='2-3', shared_rotation=90)
        with tools.fitz.open(rotated['dst']) as doc:
            self.assertEqual(len(doc), 2)
            self.assertTrue(all(p.rotation == 90 for p in doc))

    def test_export_to_filesystem_without_hardlinks(self):
        with patch.object(tools.os, 'link', side_effect=OSError(errno.EOPNOTSUPP, 'unsupported')):
            first = self.run_op('organize')
            second = self.run_op('organize')
        self.assertNotEqual(first['dst'],second['dst'])
        with tools.fitz.open(first['dst']) as doc: self.assertEqual(len(doc),4)

    def test_ranges_and_export(self):
        self.assertEqual(tools.selection('1-2, 4', 4), [1,2,4])
        for value in ['0', '2-1', '1-5', 'abc', '1,']:
            with self.assertRaises(ValueError): tools.selection(value, 4)
        result = self.run_op('export', pages='2,4', dpi=72, format='png')
        self.assertEqual(len(result['files']), 2)
        with Image.open(result['files'][0]) as image: self.assertEqual(image.size, (300,400))
        second = self.run_op('export', pages='1', format='jpeg')
        self.assertNotEqual(result['dir'], second['dir'])

    def test_numbering_and_watermark(self):
        result = self.run_op('numbers', pages='2,4', number_style='roman', number_start=3, show_total=True)
        with tools.fitz.open(result['dst']) as doc:
            self.assertIn('iii / 2', doc[1].get_text())
            self.assertIn('iv / 2', doc[3].get_text())
            self.assertNotIn('/ 2', doc[0].get_text())
        marked = self.run_op('watermark', text='DRAFT')
        with tools.fitz.open(marked['dst']) as doc:
            self.assertIn('DRAFT', doc[0].get_text())
            self.assertNotEqual(doc[0].get_pixmap().samples, tools.fitz.open(self.src)[0].get_pixmap().samples)
        preview = self.run_op('watermark', text='DRAFT', preview=True)
        with Image.open(preview['dst']) as image: self.assertGreater(image.width, 0)
        with self.assertRaises(ValueError): self.run_op('watermark', text='नेपाली')

    def test_rotated_preview_matches_export(self):
        rotated = self.run_op('organize', rotate={'1':90})['dst']
        args = dict(src=rotated, out_dir=str(self.root), action='numbers', pages='1', text='Page {page}', position='bottom-center')
        exported = tools.execute(self.ctx,args)
        preview = tools.execute(self.ctx,dict(args, preview=True, preview_page=1))
        with tools.fitz.open(exported['dst']) as doc:
            self.assertIn('Page 1',doc[0].get_text())
            pix = doc[0].get_pixmap(dpi=96,alpha=False)
            with Image.open(preview['dst']) as image:
                self.assertEqual(image.tobytes(),pix.samples)

    def test_preview_of_later_selected_page_matches_export(self):
        args=dict(src=str(self.src),out_dir=str(self.root),action='numbers',pages='2,4',number_style='roman',number_start=3,show_total=True)
        exported=tools.execute(self.ctx,args)
        preview=tools.execute(self.ctx,dict(args,preview=True,preview_page=4))
        with tools.fitz.open(exported['dst']) as doc:
            self.assertIn('iv / 2',doc[3].get_text())
            pix=doc[3].get_pixmap(dpi=96,alpha=False)
            with Image.open(preview['dst']) as image:self.assertEqual(image.tobytes(),pix.samples)

    def test_image_order_fit_orientation_and_paper(self):
        sources=[]
        for n, color in enumerate(['red','blue']):
            path=self.root/f'{n}.jpg'
            im=Image.new('RGB',(80,40),color)
            exif=im.getexif(); exif[274]=6
            im.save(path,exif=exif); sources.append(str(path))
        result=self.run_op('images',srcs=sources)
        with tools.fitz.open(result['dst']) as doc:
            self.assertEqual(len(doc),2)
            self.assertEqual((doc[0].rect.width,doc[0].rect.height),(40,80))
            pixel=doc[0].get_pixmap().pixel(10,10)
            self.assertGreater(pixel[0],pixel[2])
        fixed=self.run_op('images',srcs=sources,page_size='a4',margin=20)
        with tools.fitz.open(fixed['dst']) as doc: self.assertAlmostEqual(doc[0].rect.width,595,delta=1)
        with self.assertRaises(ValueError):self.run_op('images',srcs=sources,page_size='a4',margin=999)

    def test_passwords_permissions_and_cancel(self):
        locked=self.run_op('protect',new_password='secret',permissions=4)
        with tools.fitz.open(locked['dst']) as doc:
            self.assertTrue(doc.needs_pass)
            self.assertFalse(doc.authenticate('wrong'))
            self.assertTrue(doc.authenticate('secret'))
            self.assertFalse(doc.permissions & tools.fitz.PDF_PERM_COPY)
        with self.assertRaises(ValueError): tools.inspect(self.ctx,dict(src=locked['dst'],password='wrong'))
        unlocked=tools.execute(self.ctx,dict(action='unlock',src=locked['dst'],source_password='secret',out_dir=str(self.root)))
        with tools.fitz.open(unlocked['dst']) as doc: self.assertFalse(doc.needs_pass)
        replaced=tools.execute(self.ctx,dict(action='protect',src=locked['dst'],source_password='secret',new_password='changed',out_dir=str(self.root)))
        with tools.fitz.open(replaced['dst']) as doc: self.assertTrue(doc.authenticate('changed'))
        self.cancel.set()
        before=set(self.root.iterdir())
        with self.assertRaises(Cancelled):self.run_op('export')
        self.assertEqual(before,set(self.root.iterdir()))

if __name__ == '__main__': unittest.main()
