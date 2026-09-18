#!/usr/bin/env python3
"""Build the GitHub Pages download manifest from the latest stable release.

GitHub is the version source of truth.  A self-hosted mirror is included only
when it has caught up to the exact same immutable release.
"""
import argparse
import copy
import json
import os
from pathlib import Path
import re
import urllib.request

import distribute


def _validated_china(release_assets, mirror, public_url, tag):
    """Return a defensive copy of a matching mirror, or ``None``."""
    try:
        if mirror["schema"] != 1 or mirror["version"] != tag:
            return None
        china = mirror["china"]
        assets = china["assets"]
        if not isinstance(china.get("provider"), str) or not china["provider"]:
            return None
        if not isinstance(assets, dict) or not distribute.REQUIRED <= assets.keys():
            return None
        if not assets.keys() <= release_assets.keys():
            return None
        prefix = public_url + "/releases/" + tag + "/"
        for name, mirrored in assets.items():
            official = release_assets[name]
            if mirrored["url"] != prefix + name:
                return None
            if mirrored["size"] != official["size"]:
                return None
            if mirrored["sha256"] != official["digest"].removeprefix("sha256:"):
                return None
            if not re.fullmatch(r"[a-f0-9]{64}", mirrored["sha256"]):
                return None
        return copy.deepcopy(china)
    except (KeyError, TypeError):
        return None


def build_manifest(release, mirror, public_url):
    """Combine the latest stable release with an optional caught-up mirror."""
    tag, release_assets = distribute.validate_release(release)
    public_url = distribute.validate_base_url(public_url)
    result = {
        "schema": 1,
        "version": tag,
        "mirror_manifest_url": public_url + "/downloads.json",
    }
    china = _validated_china(release_assets, mirror, public_url, tag)
    if china is not None:
        result["china"] = china
    return result


def select_release(releases):
    """Return the newest stable release whose assets are all published.

    `releases/latest` is not usable here: GitHub flips a release to "latest"
    the moment its record exists, which on this repo is the instant the tag is
    pushed — long before the Release workflow signs and uploads the assets. On
    2026-09-18 v2.10.3's Pages deploy failed that way, because tag v2.10.4 had
    landed 92 seconds earlier and its assetless record was already "latest".
    Walking newest-first and taking the first release that validates keeps the
    manifest on the newest version users can actually download, and it can
    never move the site backwards past a complete release.
    """
    ordered = sorted(
        releases,
        key=lambda release: release.get("published_at") or release.get("created_at") or "",
        reverse=True,
    )
    rejected = []
    for release in ordered:
        try:
            distribute.validate_release(release)
        except (ValueError, KeyError, TypeError) as error:
            rejected.append(f'{release.get("tag_name")}: {error}')
            continue
        if rejected:
            print("Skipped newer releases that are not downloadable yet:")
            for line in rejected:
                print("  -", line)
        return release
    raise ValueError("No stable release carries a complete asset set: " + "; ".join(rejected))


def fetch_json(url):
    headers = {"User-Agent": "Claw-Fleet-Pages", "Accept": "application/vnd.github+json"}
    if os.environ.get("GH_TOKEN") and url.startswith("https://api.github.com/"):
        headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--releases-url",
        default=f"https://api.github.com/repos/{distribute.REPO}/releases?per_page=30",
    )
    parser.add_argument("--mirror-url", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    release = select_release(fetch_json(args.releases_url))
    public_url = args.mirror_url.removesuffix("/downloads.json").rstrip("/")
    try:
        mirror = fetch_json(args.mirror_url)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print("Mirror manifest unavailable; publishing GitHub-only manifest:", type(error).__name__)
        mirror = {}
    manifest = build_manifest(release, mirror, public_url)
    args.output.write_text(json.dumps(manifest, indent=2) + "\n")
    if "china" in manifest:
        print("Pages manifest includes matching mirror:", manifest["version"])
    else:
        print("Pages manifest uses GitHub only; mirror is not caught up:", manifest["version"])


if __name__ == "__main__":
    main()
