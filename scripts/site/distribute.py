#!/usr/bin/env python3
"""Prepare an official GitHub release for COS. No remote writes without --publish."""
import argparse
import hashlib
import json
import mimetypes
import os
from pathlib import Path
import http.client
import re
import shutil
import time
import urllib.parse
import urllib.request

import site_origin

ROOT = Path(__file__).resolve().parents[2]
REPO = 'hoveychen/claw-fleet'
# GitHub Releases drops connections partway through this asset set on the
# Shenzhen mirror's route often enough that a single-shot download never
# finished: on 2026-09-08 five consecutive unattended rounds died with
# RemoteDisconnected or a read timeout after 1-6 of the 8 assets, and each
# death restarted the whole 230 MB from zero, so the mirror could not advance
# to a new release at all.
DOWNLOAD_ATTEMPTS = 8
RETRY_DELAY = 5
REQUIRED = {'claw-fleet-macos.pkg', 'claw-fleet-windows-x64-setup.exe', 'fleet-linux-x64', 'fleet-linux-arm64'}
# The Android APK is ALLOWED but deliberately not REQUIRED: releases cut before
# the APK job existed carry no such asset, and putting it in REQUIRED would make
# `--tag latest` refuse to refresh the China website until the next release.
# The site degrades per link (docs/site.js), so a mirror without it is fine.
ALLOWED = REQUIRED | {'fleet-macos', 'fleet-windows-x64.exe', 'claw-fleet-webui.tar.gz', 'claw-fleet-android.apk'}
# Static site files that must exist in the source tree. Everything else in the
# copy list below is best-effort, because selfhost.py reads it against a
# previously published deployment (see prepare).
REQUIRED_SITE_FILES = {'index.html', 'zh/index.html', 'site.css', 'site.js', 'locale.js'}


def validate_base_url(value):
    u = urllib.parse.urlparse(value)
    if u.scheme != 'https' or not u.netloc or u.username or u.password or u.query or u.fragment or '..' in u.path.split('/'):
        raise ValueError('Public URL must be an HTTPS origin or path without credentials, query or fragment')
    return value.rstrip('/')


def validate_release(release):
    tag = release['tag_name']
    if not re.fullmatch(r'v\d+\.\d+\.\d+', tag) or release.get('draft') or release.get('prerelease'):
        raise ValueError('Only stable, published vX.Y.Z releases may be mirrored')
    assets = {a['name']: a for a in release['assets'] if a['name'] in ALLOWED}
    if not REQUIRED <= assets.keys():
        raise ValueError(f'Missing release assets: {sorted(REQUIRED - assets.keys())}')
    for a in assets.values():
        expected = f'https://github.com/{REPO}/releases/download/{tag}/{a["name"]}'
        if a['browser_download_url'] != expected:
            raise ValueError('Unexpected asset origin or path')
        if not isinstance(a['size'], int) or a['size'] <= 0:
            raise ValueError('Invalid asset size')
        if not re.fullmatch(r'sha256:[a-f0-9]{64}', a.get('digest') or ''):
            raise ValueError('GitHub SHA-256 digest is required; refusing an unverified mirror')
    return tag, assets


def verify(path, asset):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    if path.stat().st_size != asset['size'] or 'sha256:' + digest.hexdigest() != asset['digest']:
        raise ValueError(f'Size or SHA-256 mismatch: {path.name}')
    return digest.hexdigest()


def download(asset, path, temporary):
    """Fetch one asset into `temporary`, resuming what is already on disk.

    Bytes survive an attempt, so a connection that dies mid-asset costs only
    the remainder rather than the whole set. The SHA-256 check is what makes
    resuming safe to do at all: a server that ignores the Range request and
    answers with the whole body would splice garbage onto the partial, so that
    case restarts, and a partial that is full-length but fails verification is
    thrown away rather than resumed onto — keeping it would make every later
    attempt fail identically.
    """
    failure = None
    for attempt in range(DOWNLOAD_ATTEMPTS):
        offset = temporary.stat().st_size if temporary.exists() else 0
        if offset > asset['size']:
            # Longer than the asset: it can never become the asset.
            temporary.unlink()
            offset = 0
        try:
            if offset < asset['size']:
                headers = {'User-Agent': 'Claw-Fleet-distribution'}
                if offset:
                    headers['Range'] = f'bytes={offset}-'
                request = urllib.request.Request(asset['browser_download_url'], headers=headers)
                with urllib.request.urlopen(request, timeout=120) as response:
                    # Only consulted when we asked to resume; a plain 200 first
                    # attempt keeps working against any response object.
                    resumed = bool(offset) and getattr(response, 'status', 200) == 206
                    with temporary.open('ab' if resumed else 'wb') as stream:
                        shutil.copyfileobj(response, stream)
        except (OSError, http.client.HTTPException) as error:
            failure = error
        else:
            written = temporary.stat().st_size if temporary.exists() else 0
            if written < asset['size']:
                failure = ValueError(f'Truncated download at {written}/{asset["size"]} bytes: {path.name}')
            else:
                try:
                    verify(temporary, asset)
                    return
                except ValueError as error:
                    failure = error
                    temporary.unlink()
        if attempt + 1 < DOWNLOAD_ATTEMPTS:
            print(f'Retrying {path.name} after {type(failure).__name__}: {failure}', flush=True)
            time.sleep(RETRY_DELAY)
    raise failure


def site_files(root):
    """Every page reachable from the two entry documents, plus their assets.

    Derived rather than hand-listed, because a hand-listed tuple goes stale
    silently every time the site grows. It already had: the
    `screenshots/current/agents-*` and `relay-*` images were on both pages and
    in nobody's list, and `benchmark.html` arrived later with its own
    stylesheet. An unattended mirror sync copies only what this returns, so
    anything it misses is a 404 on the mirror while the origin site looks fine.

    Pages are followed transitively so a new sub-page joins the mirror the
    moment it is linked. Returns pages first, then assets.
    """
    # 404.html is a page nothing links to: nginx serves it through error_page,
    # GitHub Pages for any missing path. Listing it here (rather than among the
    # assets) also keeps this function's pages-then-assets order intact.
    pages = ['index.html', 'zh/index.html', '404.html']
    # sitemap.xml and robots.txt are listed rather than discovered: no page
    # links to them, and a mirror without them is a mirror no crawler is told
    # how to index.
    assets = ['site.css', 'site.js', 'locale.js', 'sitemap.xml', 'robots.txt']
    pending = list(pages)
    while pending:
        page = pending.pop(0)
        source = root / page
        if not source.is_file():
            continue
        prefix = page.rpartition('/')[0]
        for reference in re.findall(r'(?:src|href)="([^"]+)"', source.read_text()):
            reference = reference.split('?')[0].split('#')[0]
            if not reference or ':' in reference or reference.startswith('//') or reference.endswith('/'):
                continue
            # `__SITE_ORIGIN__/...` becomes absolute at publish time (hreflang,
            # canonical). It is not a path on disk, so looking for it here would
            # invent a file that can never be mirrored.
            if site_origin.contains_token(reference):
                continue
            if reference.startswith('../'):
                candidate = reference[3:]
            elif reference.startswith('./'):
                candidate = reference[2:]
            elif prefix:
                candidate = prefix + '/' + reference
            else:
                candidate = reference
            # Refuse anything that would escape the site root.
            if '..' in candidate.split('/') or candidate.startswith('/'):
                raise ValueError('Unsafe site reference: ' + reference)
            if candidate.endswith('.html'):
                if candidate not in pages:
                    pages.append(candidate)
                    pending.append(candidate)
            elif candidate not in assets:
                assets.append(candidate)
    return pages + assets


def prepare(release, output, public_url, *, site_root=None, provider='Tencent Cloud COS'):
    tag, assets = validate_release(release)
    public_url = validate_base_url(public_url)
    output.mkdir(parents=True, exist_ok=True)
    root = site_root or ROOT / 'docs'
    for name in site_files(root):
        source = root / name
        # selfhost.py passes site_root=<the live deployment>, so this list is
        # also read against a site published before the file existed. A newly
        # added asset must therefore be allowed to be absent there, or adding
        # one to this list wedges the mirror's unattended updater on a
        # FileNotFoundError until someone redeploys the pages by hand.
        if not source.exists() and name not in REQUIRED_SITE_FILES:
            continue
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        # Text files carry `__SITE_ORIGIN__` wherever SEO needs an absolute URL
        # (canonical, hreflang, og:url, JSON-LD, sitemap) and for the version
        # this release publishes. The mirror serves a different origin than
        # Pages, so substituting here is what keeps the mirror pointing at
        # itself instead of declaring github.io canonical.
        if site_origin.is_text(source):
            target.write_text(
                site_origin.apply(source.read_text(encoding='utf-8'), public_url, tag),
                encoding='utf-8')
            shutil.copystat(source, target)
        else:
            shutil.copy2(source, target)
    # The mirror re-publishes a previously published site against a newer
    # release, so the version in its structured data is retargeted rather than
    # substituted from a token. Prove it landed instead of trusting the regex.
    site_origin.assert_version(output, tag)
    manifest = {'schema': 1, 'version': tag, 'china': {'provider': provider, 'assets': {}}}
    checksum_lines = []
    for name, asset in sorted(assets.items()):
        path = output / 'releases' / tag / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists():
            try:
                verify(path, asset)
            except ValueError:
                # selfhost.py hands us a staging directory retained from an
                # earlier round, so a file sitting here is not proof of a good
                # download — a round killed mid-write can leave a bad one, and
                # trusting it would wedge every later round on the same error.
                print(f'Discarding unverifiable staged {name}', flush=True)
                path.unlink()
        if not path.exists():
            temporary = path.with_name(name + '.partial')
            download(asset, path, temporary)
            temporary.replace(path)
        digest = verify(path, asset)
        key = path.relative_to(output).as_posix()
        manifest['china']['assets'][name] = {'url': public_url + '/' + key, 'sha256': digest, 'size': asset['size']}
        checksum_lines.append(f'{digest}  {name}\n')
        print(f'Verified {name}: {asset["size"]} bytes')
    (output / 'releases' / tag / 'SHA256SUMS').write_text(''.join(checksum_lines))
    (output / 'downloads.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


def publish(output, manifest, public_url):
    # Import only for actual publication: preparation needs Python's stdlib only.
    from qcloud_cos import CosConfig, CosS3Client
    bucket = os.environ['COS_BUCKET']
    client = CosS3Client(CosConfig(Region=os.environ['COS_REGION'], SecretId=os.environ['COS_SECRET_ID'],
                                 SecretKey=os.environ['COS_SECRET_KEY'], Scheme='https'))
    prefix = urllib.parse.urlparse(public_url).path.strip('/')
    def upload(path, cache):
        relative = path.relative_to(output).as_posix()
        key = '/'.join(filter(None, [prefix, relative]))
        mime = mimetypes.guess_type(path.name)[0] or 'application/octet-stream'
        extra = {'ContentDisposition': 'attachment'} if relative.startswith('releases/') else {}
        client.upload_file(Bucket=bucket, Key=key, LocalFilePath=str(path), ContentType=mime,
                           CacheControl=cache, **extra)
        return key
    # Only this release and the static allowlist are published; never sweep an
    # output directory for arbitrary files or delete historical releases.
    release_files = [output/'releases'/manifest['version']/name for name in manifest['china']['assets']]
    release_files.append(output/'releases'/manifest['version']/'SHA256SUMS')
    for path in release_files:
        upload(path, 'public, max-age=31536000, immutable')
    # Probe anonymously through the real public domain, not just authenticated COS.
    for path in release_files:
        url = public_url + '/' + path.relative_to(output).as_posix()
        with urllib.request.urlopen(urllib.request.Request(url, method='HEAD'), timeout=30) as response:
            if int(response.headers.get('Content-Length', '-1')) != path.stat().st_size:
                raise ValueError('Public download size verification failed: ' + path.name)
    # Upload dependencies first, both HTML documents next, manifest last.
    # Same derived list prepare copied, but assets before pages: a visitor must
    # never load an HTML document whose stylesheet or images are not up yet.
    names = site_files(output)
    ordered = [n for n in names if not n.endswith('.html')] + [n for n in names if n.endswith('.html')]
    site_paths = [output/p for p in ordered]
    for path in site_paths:
        if not path.is_file():
            continue
        upload(path, 'public, max-age=300, must-revalidate')
    upload(output/'downloads.json', 'no-cache, max-age=0, must-revalidate')
    print('Published verified release and website:', public_url)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', default='latest', help='latest or a stable vX.Y.Z release')
    parser.add_argument('--output', type=Path, required=True, help='Preparation directory outside docs/')
    parser.add_argument('--public-url', required=True, help='Your configured HTTPS custom domain, with optional prefix')
    parser.add_argument('--publish', action='store_true', help='Upload via COS_* environment credentials')
    args = parser.parse_args()
    if args.tag != 'latest' and not re.fullmatch(r'v\d+\.\d+\.\d+', args.tag):
        parser.error('--tag must be latest or vX.Y.Z')
    public_url = validate_base_url(args.public_url)
    output = args.output.resolve()
    if output == ROOT or output.is_relative_to(ROOT / 'docs') or ROOT.is_relative_to(output):
        parser.error('Output must be a separate build directory, not the repository or docs/')
    if args.publish:
        missing = [k for k in ('COS_BUCKET','COS_REGION','COS_SECRET_ID','COS_SECRET_KEY') if not os.environ.get(k)]
        if missing:
            parser.error('Missing environment configuration: ' + ', '.join(missing))
    endpoint = 'latest' if args.tag == 'latest' else 'tags/' + args.tag
    headers = {'User-Agent': 'Claw-Fleet-distribution', 'Accept': 'application/vnd.github+json'}
    if os.environ.get('GH_TOKEN'):
        headers['Authorization'] = 'Bearer ' + os.environ['GH_TOKEN']
    request = urllib.request.Request(f'https://api.github.com/repos/{REPO}/releases/{endpoint}', headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        release = json.load(response)
    manifest = prepare(release, output, public_url)
    if args.publish:
        publish(output, manifest, public_url)
    else:
        print('Prepared locally; no remote writes. Output:', output)


if __name__ == '__main__':
    main()
