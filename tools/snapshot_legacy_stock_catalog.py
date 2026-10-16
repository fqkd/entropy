#!/usr/bin/env python3
"""Rebuild the pre-thinning 188-icon compatibility catalog, not picker artwork.

Requires Pillow and CairoSVG. Inputs are public source/archive/build artifacts;
never takes a user's pictograms.json. Only the compatibility .bin is written.
"""
import argparse
import hashlib
import io
from pathlib import Path
import runpy
import subprocess
import tarfile
import tempfile

import cairosvg
from PIL import Image, ImageDraw


def git(repo, revision, path):
    return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=repo).decode()


def pack(bits):
    return bytes(sum(0x80 >> j for j in range(8)
                     if i + j < len(bits) and bits[i + j])
                 for i in range(0, len(bits), 8))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--firmware-repo', type=Path, required=True)
    parser.add_argument('--lucide-tar', type=Path, required=True)
    parser.add_argument('--historical-executable', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory() as temp:
        temp = Path(temp)
        source = git(root, '8d2814e', 'src/pictograms.rs')
        body = source[source.index('struct Icon35('):source.index('fn legacy_35px_gear_bitmap')]
        rust = ('const PICTOGRAM_BYTES: usize=154; '
                'const BUILTIN_PICTOGRAM_KEYS:[&str;38]=["";38];\n' + body +
                '\nfn main(){use std::io::Write;for i in 0..38 {'
                'std::io::stdout().write_all(&builtin_pictogram_bitmap(i)).unwrap();}}')
        (temp / 'native.rs').write_text(rust)
        subprocess.run(['rustc', '--edition=2021', str(temp / 'native.rs'),
                        '-o', str(temp / 'native')], check=True)
        native = subprocess.check_output([str(temp / 'native')])
        action = runpy.run_path(str(root / 'tools/generate_action_icons.py'), run_name='snapshot')
        firmware = runpy.run_path(str(args.firmware_repo / 'tools/generate_builtin_key_icons.py'),
                                 run_name='snapshot')
        symbols = list(dict.fromkeys(icon for layers in action['MAP'].values()
                                     for _, line in layers for icon in line.split() if icon != '_'))
        with tarfile.open(args.lucide_tar, 'r:gz') as archive:
            def raster(symbol, mouse=None):
                # Historical default Lucide stroke=2, not the new stroke=1.7.
                svg = archive.extractfile(f'package/icons/{symbol}.svg').read()
                rgba = Image.open(io.BytesIO(cairosvg.svg2png(
                    bytestring=svg, output_width=35, output_height=35))).convert('RGBA')
                if mouse:
                    region = {'mouse_left': (11, 12, 15, 17),
                              'mouse_middle': (16, 10, 19, 15),
                              'mouse_right': (20, 12, 24, 17)}[mouse]
                    ImageDraw.Draw(rgba).rounded_rectangle(region, radius=1, fill=(0, 0, 0, 255))
                return pack([rgba.getpixel((x, y))[3] >= 72
                             for y in range(35) for x in range(35)])
            actions = b''.join(raster(action['ALIASES'].get(s, s)) for s in symbols)
            fw = b''.join(raster(symbol, name if name.startswith('mouse_') else None)
                          for name, symbol in firmware['ICONS'].items())
        # The three-tier predecessor did not survive the squashed PR source.
        # Its two contiguous stock entries survive in the shipped public ELF.
        executable = args.historical_executable.read_bytes()
        assert hashlib.sha256(executable).hexdigest() == (
            'bd4d7111c835d39fc7b30f28d2ab1769077114f8698a5df85e2b89be5cf6ba3a')
        layers = executable[2756175:2756175 + 2 * 154]
        source = git(args.firmware_repo, '077eff8801',
                     'keyboards/ergohaven/macropad/screen_layout.c')
        body = ('static void integration_icon_pixel' +
                source.split('static void integration_icon_pixel', 1)[1].split(
                    'static void screen_layout_set_integration_icon', 1)[0])
        c = ('#include <stdint.h>\n#include <stdbool.h>\n#include <stdio.h>\n'
             '#define EH_ICON_RENDER_SIZE 35\n#define EH_ICON_RENDER_STRIDE 5\n' + body +
             '\nint main(void){for(uint8_t v=1;v<=11;++v){uint8_t b[175]={0};'
             'integration_icon_base(b,v);fwrite(b,1,175,stdout);}return 0;}')
        (temp / 'integration.c').write_text(c)
        subprocess.run(['cc', '-std=c99', '-Wno-unused-function', str(temp / 'integration.c'),
                        '-o', str(temp / 'integration')], check=True)
        rows = subprocess.check_output([str(temp / 'integration')])
        integrations = b''.join(pack([
            bool(rows[k * 175 + y * 5 + x // 8] & (0x80 >> (x % 8)))
            for y in range(35) for x in range(35)]) for k in range(11))
        result = native + actions + fw + layers + integrations
        assert len(result) == 188 * 154
        output = root / 'assets/legacy-stock-catalog-pre-thinning.bin'
        output.write_bytes(result)
        print(f'{output}: {len(result)} bytes, sha256={hashlib.sha256(result).hexdigest()}')


if __name__ == '__main__':
    main()
