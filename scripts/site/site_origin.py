#!/usr/bin/env python3
"""Rewrite the site origin token into the origin a deployment is served from.

The landing page is published to two origins with byte-identical files:
GitHub Pages (https://hoveychen.github.io/claw-fleet) and the China mirror
(https://fleet.eternizedlab.com, synced by selfhost.py). SEO needs absolute
URLs in a few places -- `og:url`, hreflang alternates, JSON-LD `url`/`@id`,
sitemap.xml, robots.txt -- and an absolute URL cannot be written once and be
correct on both. Baking one domain in would make the mirror declare the other
domain as its canonical, which is exactly how a mirror gets dropped from Baidu
while nobody notices the origin site looks fine.

So the generator writes `__SITE_ORIGIN__` and each publishing path substitutes
its own origin: the Pages workflow before upload, distribute.prepare() while it
copies files for the mirror. The committed files in docs/ therefore carry no
domain at all, and adding a third origin costs one flag.
"""
from pathlib import Path
import urllib.parse

TOKEN = '__SITE_ORIGIN__'
# Only text formats that can carry the token. A .png containing the byte
# sequence is not a URL, and rewriting binaries would corrupt them.
TEXT_SUFFIXES = ('.html', '.xml', '.txt', '.json', '.js', '.css')


def validate_origin(value):
    """Accept an https origin, optionally with a path prefix, and no trailing slash.

    Pages serves the site from a sub-path (/claw-fleet), the mirror from a bare
    host, so both shapes are legitimate. Anything with a query, a fragment or
    credentials would produce URLs that no crawler treats as canonical.
    """
    u = urllib.parse.urlparse(value)
    if u.scheme != 'https' or not u.netloc or u.username or u.password or u.query or u.fragment:
        raise ValueError('Site origin must be an HTTPS origin or origin+path, without credentials, query or fragment')
    if '..' in u.path.split('/'):
        raise ValueError('Site origin path must not contain ..')
    return value.rstrip('/')


def apply(text, origin):
    """Substitute the token in one document."""
    return text.replace(TOKEN, validate_origin(origin))


def is_text(path):
    return path.suffix.lower() in TEXT_SUFFIXES


def apply_tree(root, origin):
    """Rewrite every text file under `root` in place. Returns the paths changed."""
    origin = validate_origin(origin)
    changed = []
    for path in sorted(Path(root).rglob('*')):
        if not path.is_file() or not is_text(path):
            continue
        original = path.read_text(encoding='utf-8')
        if TOKEN not in original:
            continue
        path.write_text(original.replace(TOKEN, origin), encoding='utf-8')
        changed.append(path)
    return changed


def assert_no_token(root):
    """Fail loudly if a published tree still carries the token.

    A leftover `__SITE_ORIGIN__` is not a visible break -- the page renders and
    only the machine-readable half is wrong -- so it has to be an error, not
    something to notice later in Search Console.
    """
    stragglers = [p for p in sorted(Path(root).rglob('*'))
                  if p.is_file() and is_text(p) and TOKEN in p.read_text(encoding='utf-8')]
    if stragglers:
        raise ValueError('Site origin token left unsubstituted in: '
                         + ', '.join(str(p) for p in stragglers))


def contains_token(reference):
    """True when a src/href value is token-based, i.e. absolute once published.

    distribute.site_files() crawls src/href to decide what to mirror; a
    token URL is not a relative path and must not be looked for on disk.
    """
    return TOKEN in reference


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True, help='Directory to rewrite in place')
    parser.add_argument('--origin', required=True, help='https origin (optionally with path prefix) this tree is served from')
    args = parser.parse_args()
    for path in apply_tree(args.root, args.origin):
        print(path)
    assert_no_token(args.root)
