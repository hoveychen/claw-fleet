#!/usr/bin/env python3
"""Mirror new stable releases, and the site pages of that release, into the self-hosted site."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import tarfile
import tempfile
import urllib.request
import uuid

import distribute
import mirror_site


def version(tag):
    if not isinstance(tag, str) or not re.fullmatch(r'v\d+\.\d+\.\d+', tag):
        raise ValueError('Invalid current stable version')
    return tuple(map(int, tag[1:].split('.')))


def upstream_release():
    """The newest release the mirror can actually copy.

    Deliberately the release list rather than `releases/latest`: a just-pushed
    tag is already "latest" while its assets are still being built, and a round
    that starts on one has nothing to download and only aborts.
    """
    request = urllib.request.Request(
        f'https://api.github.com/repos/{distribute.REPO}/releases?per_page=30',
        headers={'User-Agent': 'Claw-Fleet-selfhost', 'Accept': 'application/vnd.github+json'},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return distribute.newest_downloadable_release(json.load(response))


def fetch_site_docs(tag, workdir):
    """Download the repository at `tag` and return the path of its `docs/`.

    The source archive comes from codeload because that is the GitHub route
    the Shenzhen box can reach: raw.githubusercontent.com does not resolve
    there, and the contents API allows 60 unauthenticated requests an hour,
    fewer than the site has files. The whole archive took ~3s there (measured
    2026-10-22, ~50 MB at v3.1.0); only `docs/` is extracted.
    """
    request = urllib.request.Request(
        f'https://codeload.github.com/{distribute.REPO}/tar.gz/refs/tags/{tag}',
        headers={'User-Agent': 'Claw-Fleet-selfhost'},
    )
    archive = workdir / 'source.tar.gz'
    with urllib.request.urlopen(request, timeout=120) as response, archive.open('wb') as out:
        shutil.copyfileobj(response, out)
    extracted = workdir / 'source'
    with tarfile.open(archive) as tar:
        members = [m for m in tar.getmembers() if m.name.split('/')[1:2] == ['docs']]
        if not members:
            raise ValueError(f'No docs/ in the {tag} source archive')
        tar.extractall(extracted, members=members, filter='data')
    (top,) = extracted.iterdir()
    return top / 'docs'


def staging_directory(deployments, tag):
    """Reuse this tag's abandoned stage so a killed round is not wasted.

    Downloading the asset set over this route takes hours, far longer than any
    single unattended round survives, so the only way the mirror can reach a
    new release is by accumulating bytes across rounds. Only a stage for the
    same tag is reused — a leftover from an older tag holds URLs and checksums
    that no longer apply.

    When several are lying around, the one holding the most downloaded bytes
    wins, not the most recent: a fresh round that just started has the newest
    mtime and almost nothing in it, while the round killed before it may hold
    most of the set.
    """
    def staged_bytes(stage):
        return sum(p.stat().st_size for p in (stage / 'releases' / tag).glob('*') if p.is_file())

    existing = sorted(deployments.glob(f'.stage-{tag}-*'), key=staged_bytes)
    if existing:
        stage = existing[-1]
        print(f'Resuming staged {tag} at {stage} ({staged_bytes(stage)} bytes downloaded)', flush=True)
        return stage
    return Path(tempfile.mkdtemp(prefix=f'.stage-{tag}-', dir=deployments))


def sync(root, public_url, release, *, rebuild_current=False, fetch_docs=None):
    """Caller holds the update lock. Failed preparation never changes current.

    With `fetch_docs`, the new deployment's pages are rebuilt from the
    release's own sources; without it they are carried over from the live
    deployment. Carrying over was the only behaviour until 2026-10-22, and it
    left the mirror serving the 2026-09-08 pages for six weeks of releases.
    """
    root = root.resolve()
    public_url = distribute.validate_base_url(public_url)
    deployments = root / 'deployments'
    current_link = root / 'current'
    if not current_link.is_symlink():
        raise ValueError('Bootstrap current as a deployment symlink before enabling updates')
    current = current_link.resolve(strict=True)
    if not current.is_relative_to(deployments.resolve()) or current == deployments.resolve():
        raise ValueError('Current must be inside the deployment directory')
    saved = json.loads((current / 'downloads.json').read_text())
    tag, assets = distribute.validate_release(release)
    old_tag = saved['version']
    if version(tag) < version(old_tag):
        print(f'Skipped older upstream {tag}; serving {old_tag}', flush=True)
        return False
    if tag == old_tag:
        # Never replace immutable URLs with changed bytes, even during a rebuild.
        existing = saved['china']['assets']
        if set(existing) != set(assets) or any(
            existing[name]['size'] != asset['size'] or
            'sha256:' + existing[name]['sha256'] != asset['digest']
            for name, asset in assets.items()
        ):
            raise ValueError('Upstream changed an already published immutable release')
        if not rebuild_current:
            print(f'Up to date: {tag}; no downloads or site changes', flush=True)
            return False
    stage = staging_directory(deployments, tag)
    switch = root / ('.current-' + uuid.uuid4().hex)
    try:
        # Immutable historical binaries share disk blocks, never get rewritten.
        # SHA256SUMS is copied because prepare rewrites the current checksum file.
        def copy_history(src, dst):
            if Path(dst).exists():
                return dst  # Already linked by an earlier round into this stage.
            if Path(src).name == 'SHA256SUMS':
                return shutil.copy2(src, dst)
            os.link(src, dst)
            return dst
        shutil.copytree(current / 'releases', stage / 'releases', copy_function=copy_history,
                        dirs_exist_ok=True)
        distribute.prepare(release, stage, public_url, site_root=current,
                           provider=saved['china'].get('provider', 'Self-hosted'))
        if fetch_docs:
            with tempfile.TemporaryDirectory() as work:
                docs = fetch_docs(tag, Path(work))
                # Overwrites the carried-over pages in place. Safe: only
                # releases/ in the stage is hard-linked to older deployments.
                mirror_site.build(docs, stage, public_url, tag, repo_root=docs.parent)
            print(f'Rebuilt site pages from {tag} sources', flush=True)
        # mkdtemp is 0700: nginx must be able to traverse the ready deployment.
        stage.chmod(0o755)
        ready = deployments / ('auto-' + tag + '-' + uuid.uuid4().hex[:12])
        stage.rename(ready)
        switch.symlink_to(ready)
        os.replace(switch, current_link)
        print(f'Activated {tag}: {ready}; previous site retained at {current}', flush=True)
        for orphan in deployments.glob('.stage-*'):
            # The update lock means no other sync is running, so any staging
            # directory still here was abandoned by a killed round.
            shutil.rmtree(orphan, ignore_errors=True)
        return True
    finally:
        if switch.is_symlink():
            switch.unlink()
        # The stage is deliberately NOT removed on failure. On this mirror's
        # route the asset set takes hours, so a round that dies partway is the
        # normal case, and deleting its bytes made net progress permanently
        # zero: every 45-minute systemd timeout threw away everything and the
        # next round started from scratch, so a new release could never land.
        # A retained stage is safe because prepare re-verifies every file it
        # finds there and refetches anything that fails.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--public-url', required=True)
    parser.add_argument('--rebuild-current', action='store_true',
                        help='Reverify and atomically rebuild the same immutable release for deployment validation')
    args = parser.parse_args()
    os.umask(0o022)
    with (args.root / '.update.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print('Another update is running; skipped', flush=True)
            return
        sync(args.root, args.public_url, upstream_release(), rebuild_current=args.rebuild_current,
             fetch_docs=fetch_site_docs)


if __name__ == '__main__':
    main()
