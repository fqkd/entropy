#!/usr/bin/env python3
"""Immutable approved assets and sync guard (runs only in scratch repositories)."""
from pathlib import Path
import hashlib
import json
import shutil
import subprocess
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
QMK = ROOT.parent / 'vial-qmk-icons-pr'

class Artwork(unittest.TestCase):
    def test_both_asset_representations(self):
        master = ROOT / 'tools/artwork/layer-navigation'
        manifest = json.loads((master / 'provenance.json').read_text())
        for name, digest in manifest['sha256'].items():
            self.assertEqual(hashlib.sha256((master / name).read_bytes()).hexdigest(), digest)
        catalog = (ROOT / 'assets/firmware-builtin-icons.bin').read_bytes()
        self.assertEqual(len(catalog), 22 * 154)
        self.assertEqual(hashlib.sha256(catalog[:20*154]).hexdigest(), manifest['unchanged_catalog_prefix_sha256'])
        for i, direction in enumerate(('prev', 'next')):
            golden = (master / f'layer_{direction}_icon.bin').read_bytes()
            self.assertEqual((ROOT / f'assets/display-layer-{direction}.bin').read_bytes(), golden)
            self.assertEqual(catalog[(20+i)*154:(21+i)*154], golden)

    def test_sync_regenerates_both_representations(self):
        with tempfile.TemporaryDirectory(prefix='entropy-artwork-') as tmp:
            tmp = Path(tmp)
            shutil.copytree(ROOT / 'tools', tmp / 'tools')
            shutil.copytree(ROOT / 'assets', tmp / 'assets')
            (tmp / 'src').mkdir()
            for direction in ('prev', 'next'):
                (tmp / f'assets/display-layer-{direction}.bin').write_bytes(bytes(154))
            subprocess.run(['python3', str(tmp / 'tools/sync_firmware_builtin_icons.py'), str(QMK / 'keyboards/ergohaven/macropad/builtin_key_icons.h'), str(QMK / 'keyboards/ergohaven/macropad/layer_navigation_icons.h')], check=True, capture_output=True)
            self.assertEqual((tmp / 'assets/firmware-builtin-icons.bin').read_bytes(), (ROOT / 'assets/firmware-builtin-icons.bin').read_bytes())
            for direction in ('prev', 'next'):
                self.assertEqual((tmp / f'assets/display-layer-{direction}.bin').read_bytes(), (ROOT / f'tools/artwork/layer-navigation/layer_{direction}_icon.bin').read_bytes())

    def test_sync_rejects_unapproved_layer_header(self):
        with tempfile.TemporaryDirectory(prefix='entropy-artwork-') as tmp:
            tmp = Path(tmp)
            shutil.copytree(ROOT / 'tools', tmp / 'tools')
            (tmp / 'assets').mkdir(); (tmp / 'src').mkdir()
            header = tmp / 'unapproved.h'
            header.write_bytes(subprocess.check_output(['git', 'show', '12816bc441:keyboards/ergohaven/macropad/layer_navigation_icons.h'], cwd=QMK))
            result = subprocess.run(['python3', str(tmp / 'tools/sync_firmware_builtin_icons.py'), str(QMK / 'keyboards/ergohaven/macropad/builtin_key_icons.h'), str(header)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, 'sync accepted wrong rhombus icons')
            self.assertIn('Unapproved layer', result.stderr)

if __name__ == '__main__': unittest.main()
