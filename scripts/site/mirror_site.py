#!/usr/bin/env python3
"""Build the site tree for the China mirror, ready to rsync into a deployment.

Why this exists rather than a few lines in a shell history: the mirror needs
three things done to the built site that Pages does not, and the third one has
been hand-rolled (and once nearly forgotten) every time.

1. `__SITE_ORIGIN__` / `__SITE_VERSION__` resolved to the mirror's own origin
   and the release it serves, so the mirror is its own canonical rather than
   declaring github.io -- see site_origin.py.
2. The ICP footer. The mirror is hosted in mainland China and every page it
   serves carries a filing number by law. It is not in the built site because
   the GitHub Pages copy is not the filed site; a deploy that forgets it drops
   a legal requirement silently, and nothing on the page looks wrong.
3. Only the site files. `downloads.json` and `releases/` belong to the mirror
   (its own manifest, its own verified packages) and must be left alone, which
   is why this writes a tree to rsync *over* a clone of the live deployment
   rather than a whole deployment.

The deployment-wide DEPLOY-SHA256SUMS is deliberately *not* written here: it
covers `releases/` too, and those files only exist on the server. Regenerate it
there after rsync, per docs/china-selfhost.md.
"""
from pathlib import Path
import argparse
import shutil

import distribute
import sitemap_lastmod
import site_origin

# The filing number the mirror serves under. Present on every page of the
# mirror, absent from the Pages build.
ICP_NUMBER = '粤ICP备2026103741号-1'
ICP_HTML = ('<p><a href="https://beian.miit.gov.cn/" rel="noopener noreferrer">'
            f'{ICP_NUMBER}</a></p>')


def add_icp_footer(html, icp_html=ICP_HTML, number=ICP_NUMBER):
    """Put the filing link in the page footer, or give the page one.

    404.html has no footer of its own, and it is a page the mirror serves like
    any other, so it needs the notice too.
    """
    if number in html:
        return html
    if '</footer>' in html:
        return html.replace('</footer>', icp_html + '</footer>', 1)
    return html.replace('</body>', f'<footer class="wrap doc-footer">{icp_html}</footer></body>', 1)


def build(docs, output, origin, version, repo_root=distribute.ROOT):
    """Write the mirror's site files into `output`. Returns the paths written."""
    origin = site_origin.validate_origin(origin)
    version = site_origin.validate_version(version)
    output.mkdir(parents=True, exist_ok=True)
    names = [name for name in distribute.site_files(docs) if (docs / name).is_file()]
    # A mirror manifest overwritten by the Pages one would advertise GitHub
    # URLs and drop the verified China assets.
    assert 'downloads.json' not in names, 'the mirror keeps its own downloads.json'
    for name in names:
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        source = docs / name
        if site_origin.is_text(source):
            body = site_origin.apply(source.read_text(encoding='utf-8'), origin, version)
            if name.endswith('.html'):
                body = add_icp_footer(body)
            target.write_text(body, encoding='utf-8')
        else:
            shutil.copy2(source, target)
    site_origin.assert_no_token(output)
    site_origin.assert_version(output, version)
    # Checked before anything optional runs: this is the one that carries a
    # legal consequence, and a page that ships without the notice looks fine.
    missing = [name for name in names
               if name.endswith('.html') and ICP_NUMBER not in (output / name).read_text(encoding='utf-8')]
    if missing:
        raise ValueError('ICP footer missing from: ' + ', '.join(missing))
    # Same lastmod treatment Pages gets, applied to the copy rather than to the
    # checkout: real commit dates where git can supply them, nothing where it
    # cannot. Without this the mirror's sitemap would be the one with no dates.
    sitemap = output / 'sitemap.xml'
    if sitemap.is_file():
        paths = sitemap_lastmod.sitemap_paths(sitemap, origin_token=origin)
        sitemap_lastmod.stamp(sitemap, sitemap_lastmod.page_lastmod(repo_root, docs, paths),
                              origin=origin)
    return names


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--docs', type=Path, default=distribute.ROOT / 'docs')
    parser.add_argument('--output', type=Path, required=True, help='Directory to rsync from')
    parser.add_argument('--origin', default='https://fleet.eternizedlab.com')
    parser.add_argument('--version', required=True, help='Release the mirror serves, e.g. v2.7.0')
    args = parser.parse_args()
    if args.output.is_relative_to(args.docs):
        parser.error('Output must sit outside the site root')
    names = build(args.docs, args.output, args.origin, args.version)
    pages = [n for n in names if n.endswith('.html')]
    print(f'{len(names)} files for {args.origin} at {args.version} '
          f'({len(pages)} pages, all carrying {ICP_NUMBER})')
    print(f'Next: rsync -rlt {args.output}/ <host>:<new-deployment>/  '
          '(clone the live deployment with cp -al first), regenerate '
          'DEPLOY-SHA256SUMS on the server, then switch the symlink under flock.')
