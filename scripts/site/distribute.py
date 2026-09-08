#!/usr/bin/env python3
"""Prepare an official GitHub release for COS. No remote writes without --publish."""
import argparse
import hashlib
import json
import mimetypes
import os
from pathlib import Path
import re
import shutil
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
REPO = 'hoveychen/claw-fleet'
REQUIRED = {'claw-fleet-macos.pkg', 'claw-fleet-windows-x64-setup.exe', 'fleet-linux-x64', 'fleet-linux-arm64'}
# The Android APK is ALLOWED but deliberately not REQUIRED: releases cut before
# the APK job existed carry no such asset, and putting it in REQUIRED would make
# `--tag latest` refuse to refresh the China website until the next release.
# The site degrades per link (docs/site.js), so a mirror without it is fine.
ALLOWED = REQUIRED | {'fleet-macos', 'fleet-windows-x64.exe', 'claw-fleet-webui.tar.gz', 'claw-fleet-android.apk'}


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


def prepare(release, output, public_url, *, site_root=None, provider='Tencent Cloud COS'):
    tag, assets = validate_release(release)
    public_url = validate_base_url(public_url)
    output.mkdir(parents=True, exist_ok=True)
    for name in ('index.html', 'zh/index.html', 'site.css', 'site.js', 'locale.js', 'icon.png', 'hero.png',
                 'icon-apple.svg', 'icon-windows.svg', 'icon-linux.svg', 'icon-android.svg',
                 'screenshots/current/work-en.png', 'screenshots/current/work-zh.png',
                 'screenshots/current/review-en.png', 'screenshots/current/review-zh.png',
                 'screenshots/current/results-en.png', 'screenshots/current/results-zh.png', 'screenshots/current/mobile-en.png',
                 'screenshots/current/mobile-zh.png'):
        target = output / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2((site_root or ROOT / 'docs') / name, target)
    manifest = {'schema': 1, 'version': tag, 'china': {'provider': provider, 'assets': {}}}
    checksum_lines = []
    for name, asset in sorted(assets.items()):
        path = output / 'releases' / tag / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            temporary = path.with_name(name + '.partial')
            request = urllib.request.Request(asset['browser_download_url'], headers={'User-Agent': 'Claw-Fleet-distribution'})
            with urllib.request.urlopen(request, timeout=120) as response, temporary.open('wb') as stream:
                shutil.copyfileobj(response, stream)
            verify(temporary, asset)
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
    site_paths = [output/p for p in ('site.css','site.js','locale.js','icon.png','hero.png','icon-apple.svg',
        'icon-windows.svg','icon-linux.svg','icon-android.svg','screenshots/current/work-en.png','screenshots/current/work-zh.png',
        'screenshots/current/review-en.png','screenshots/current/review-zh.png','screenshots/current/results-en.png', 'screenshots/current/results-zh.png',
        'screenshots/current/mobile-en.png','screenshots/current/mobile-zh.png',
        'index.html','zh/index.html')]
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
