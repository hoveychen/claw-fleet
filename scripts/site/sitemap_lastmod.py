#!/usr/bin/env python3
"""Stamp each sitemap entry with the commit time of the page it points at.

Run this against a checkout right before publishing. It cannot live in
build.py: build.py generates the pages, so at that moment the commit that
changes them does not exist yet and every date would be one commit stale --
stale exactly for the page that just changed, which is the only page a crawler
cares about.

`<lastmod>` is only worth writing if it is true. Google ignores a lastmod it
catches disagreeing with the content it crawled, so this script refuses to
guess: no git, a shallow clone, or a page with no commit of its own all mean
the entry ships without a lastmod rather than with an invented one.
"""
from pathlib import Path
import argparse
import re
import subprocess


def page_file(url_path):
    """The file on disk a sitemap path is published from.

    Derived rather than hand-listed: a map would go stale every time the site
    grows a page, and the failure is silent (that page just never gets a date).
    A page's own commit time is the honest answer -- deriving it from
    content/*.json instead would miss a change to the generator or to a
    screenshot the page embeds.
    """
    if url_path.endswith('/'):
        return url_path.lstrip('/') + 'index.html'
    return url_path.lstrip('/')


def sitemap_paths(sitemap, origin_token='__SITE_ORIGIN__'):
    """Every URL path the sitemap lists, in order."""
    paths = []
    for loc in re.findall(r'<loc>([^<]+)</loc>', sitemap.read_text(encoding='utf-8')):
        paths.append(loc.split(origin_token, 1)[-1] if origin_token in loc else loc)
    return paths


def git(root, *args):
    result = subprocess.run(('git', '-C', str(root)) + args,
                            capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else None


def is_usable_repo(root):
    """A shallow clone would report HEAD as every file's last change.

    actions/checkout defaults to depth 1, so this is the failure mode that
    would silently stamp "changed just now" onto all four pages.
    """
    if git(root, 'rev-parse', '--is-inside-work-tree') != 'true':
        return False
    return git(root, 'rev-parse', '--is-shallow-repository') == 'false'


def page_lastmod(repo_root, docs, url_paths):
    """{url path: YYYY-MM-DD} for the pages git can actually date."""
    if not is_usable_repo(repo_root):
        return {}
    dates = {}
    for url_path in url_paths:
        target = (docs / page_file(url_path)).resolve()
        stamp = git(repo_root, 'log', '-1', '--format=%cs', '--', str(target))
        if stamp:
            dates[url_path] = stamp
    return dates


def stamp(sitemap, dates, origin_token='__SITE_ORIGIN__'):
    """Insert <lastmod> after each <loc> we have a date for. Idempotent."""
    body = sitemap.read_text(encoding='utf-8')
    if '<lastmod>' in body:
        body = re.sub(r'<lastmod>[^<]*</lastmod>', '', body)
    stamped = 0

    def insert(match):
        nonlocal stamped
        loc = match.group(1)
        path = loc.split(origin_token, 1)[-1] if origin_token in loc else loc
        date = dates.get(path)
        if not date:
            return match.group(0)
        stamped += 1
        return f'{match.group(0)}<lastmod>{date}</lastmod>'

    body = re.sub(r'<loc>([^<]+)</loc>', insert, body)
    sitemap.write_text(body, encoding='utf-8')
    return stamped


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2],
                        help='Repository root (must not be a shallow clone)')
    parser.add_argument('--docs', type=Path, help='Site directory; defaults to <root>/docs')
    args = parser.parse_args()
    docs = args.docs or args.root / 'docs'
    sitemap = docs / 'sitemap.xml'
    paths = sitemap_paths(sitemap)
    dates = page_lastmod(args.root, docs, paths)
    if not dates:
        print('No usable git history (shallow clone or not a repo); '
              'sitemap published without lastmod rather than with invented dates')
    count = stamp(sitemap, dates)
    print(f'Stamped {count} of {len(paths)} sitemap entries')
    for path, date in sorted(dates.items()):
        print(f'  {path} -> {date}')
