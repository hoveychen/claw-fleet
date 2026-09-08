#!/usr/bin/env python3
"""Encode every current screenshot to WebP beside its PNG.

The screenshots are the page's whole weight: results-en.png alone is 385 KB
against a 33 KB document. They are lazy-loaded, so they do not hold up LCP --
they just cost every visitor the bytes, and the visitors on the China mirror
pay for them over the slowest link we have.

The PNG stays in the repo and stays the `<img src>`: the generator emits a
<picture> with the WebP as a <source>, so a browser that cannot decode WebP
still gets the screenshot, and "open full-size" still hands out a PNG anyone
can save and paste anywhere.

Re-run after re-capturing screenshots (scripts/site/capture.mjs). Idempotent:
an existing .webp newer than its .png is left alone.
"""
from pathlib import Path
import argparse
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
SHOTS = ROOT / 'docs/screenshots/current'
# 82 is where cwebp stops being visibly lossy on flat UI screenshots (large
# areas of one colour, thin text); -m 6 spends encode time, not decode time.
QUALITY = '82'
METHOD = '6'


def encode(png, quality=QUALITY, method=METHOD):
    webp = png.with_suffix('.webp')
    if webp.exists() and webp.stat().st_mtime >= png.stat().st_mtime:
        return webp, False
    subprocess.run(['cwebp', '-quiet', '-q', quality, '-m', method,
                    str(png), '-o', str(webp)], check=True)
    return webp, True


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--shots', type=Path, default=SHOTS)
    parser.add_argument('--force', action='store_true', help='Re-encode even if the WebP is current')
    args = parser.parse_args()
    if not shutil.which('cwebp'):
        sys.exit('cwebp not found: brew install webp (or apt install webp)')
    total_png = total_webp = 0
    for png in sorted(args.shots.glob('*.png')):
        if args.force:
            png.with_suffix('.webp').unlink(missing_ok=True)
        webp, wrote = encode(png)
        total_png += png.stat().st_size
        total_webp += webp.stat().st_size
        print(f'{"encoded" if wrote else "current"} {webp.name}: '
              f'{png.stat().st_size // 1024} KB -> {webp.stat().st_size // 1024} KB')
    if total_png:
        print(f'total {total_png // 1024} KB -> {total_webp // 1024} KB '
              f'({100 - total_webp * 100 // total_png}% smaller)')
