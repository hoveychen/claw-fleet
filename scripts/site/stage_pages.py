#!/usr/bin/env python3
"""Stage the files GitHub Pages should publish, with this origin's URLs resolved.

docs/ is a working directory as much as a web root: it also holds design notes,
product research, site reviews and the mirror's systemd units. Uploading the
whole folder published all of it -- indexable thin pages next to the landing
page, and operational config anyone could read.

So Pages gets the same derived list the mirror gets (distribute.site_files(),
which follows src/href from the two entry pages), plus the few files nothing
links to but the site still needs.
"""
from pathlib import Path
import argparse
import shutil

import distribute
import site_origin

# Not reachable from a src/href on the pages, and needed anyway:
#   downloads.json -- site.js fetches it at runtime; the Pages workflow
#                     regenerates it from the latest release before upload.
#   sitemap.xml / robots.txt -- crawler entry points by definition (they are
#                     already in distribute's list; named here so this file
#                     reads as the full contract).
#   player/ -- the promo player, published at /player/ and shared as a link
#              even though the landing page does not link to it. Dropping it
#              would 404 a URL that is already out in the world.
EXTRA_FILES = ('downloads.json', 'sitemap.xml', 'robots.txt')
EXTRA_TREES = ('player',)
# Search-console ownership proofs have to sit at the site root under a name the
# console picks, so they cannot be listed one by one ahead of time. Dropping the
# file into docs/ is all it should take -- a whitelist that silently withholds
# it turns "verify your site" into an afternoon of confusion.
# Narrow patterns on purpose: a catch-all like '*.txt' would quietly publish
# whatever else lands in docs/, which is the thing this whitelist exists to stop.
VERIFICATION_GLOBS = ('google*.html', 'baidu_verify_*.html', 'baidu_verify_*.txt',
                      'sogousiteverification.txt', 'BingSiteAuth.xml',
                      'yandex_*.html', '*_verify.html')


def payload(root):
    """Site-root-relative paths Pages should publish, in a stable order."""
    names = [name for name in distribute.site_files(root) if (root / name).is_file()]
    for name in EXTRA_FILES:
        if (root / name).is_file() and name not in names:
            names.append(name)
    for pattern in VERIFICATION_GLOBS:
        for path in sorted(root.glob(pattern)):
            name = path.name
            if path.is_file() and name not in names:
                names.append(name)
    for tree in EXTRA_TREES:
        for path in sorted((root / tree).rglob('*')):
            if path.is_file():
                names.append(str(path.relative_to(root)))
    return names


def stage(root, output, origin):
    output.mkdir(parents=True, exist_ok=True)
    published = payload(root)
    for name in published:
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        source = root / name
        if site_origin.is_text(source):
            target.write_text(site_origin.apply(source.read_text(encoding='utf-8'), origin),
                              encoding='utf-8')
        else:
            shutil.copy2(source, target)
    site_origin.assert_no_token(output)
    return published


def excluded(root, published):
    kept = set(published)
    return sorted(str(p.relative_to(root)) for p in root.rglob('*')
                  if p.is_file() and str(p.relative_to(root)) not in kept)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=distribute.ROOT / 'docs')
    parser.add_argument('--output', type=Path, required=True, help='Staging directory to upload')
    parser.add_argument('--origin', required=True, help='https origin this deployment is served from')
    args = parser.parse_args()
    if args.output.is_relative_to(args.root):
        parser.error('Staging directory must sit outside the site root')
    published = stage(args.root, args.output, site_origin.validate_origin(args.origin))
    print(f'Publishing {len(published)} files from {args.root} to {args.output}')
    for name in published:
        print(f'  + {name}')
    left = excluded(args.root, published)
    print(f'Withheld {len(left)} files (not part of the site):')
    for name in left:
        print(f'  - {name}')
