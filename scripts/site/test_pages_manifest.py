"""Pages manifest tests; all inputs are local fixtures, never the network."""
import copy
import unittest

import pages_manifest as p
from test_distribute import fixture


ROOT = p.distribute.ROOT


class PagesManifestTests(unittest.TestCase):
    def setUp(self):
        self.release, _ = fixture()
        self.release["tag_name"] = "v2.7.0"
        for asset in self.release["assets"]:
            asset["browser_download_url"] = asset["browser_download_url"].replace(
                "/v2.6.0/", "/v2.7.0/"
            )
        self.mirror = {
            "schema": 1,
            "version": "v2.7.0",
            "china": {
                "provider": "Eternized Lab · Shenzhen",
                "assets": {
                    asset["name"]: {
                        "url": (
                            "https://fleet.eternizedlab.com/releases/v2.7.0/"
                            + asset["name"]
                        ),
                        "sha256": asset["digest"].removeprefix("sha256:"),
                        "size": asset["size"],
                    }
                    for asset in self.release["assets"]
                },
            },
        }

    def test_stale_mirror_never_makes_pages_report_the_old_version(self):
        stale = copy.deepcopy(self.mirror)
        stale["version"] = "v2.6.3"
        for asset in stale["china"]["assets"].values():
            asset["url"] = asset["url"].replace("/v2.7.0/", "/v2.6.3/")

        manifest = p.build_manifest(
            self.release, stale, "https://fleet.eternizedlab.com"
        )

        self.assertEqual(manifest["version"], "v2.7.0")
        self.assertEqual(
            manifest["mirror_manifest_url"],
            "https://fleet.eternizedlab.com/downloads.json",
        )
        self.assertNotIn("china", manifest)

    def test_matching_complete_mirror_is_exposed_without_rewriting_it(self):
        before = copy.deepcopy(self.mirror)

        manifest = p.build_manifest(
            self.release, self.mirror, "https://fleet.eternizedlab.com"
        )

        self.assertEqual(manifest["version"], "v2.7.0")
        self.assertEqual(manifest["china"], before["china"])
        self.assertEqual(self.mirror, before)

    def test_matching_but_incomplete_mirror_is_not_exposed(self):
        incomplete = copy.deepcopy(self.mirror)
        incomplete["china"]["assets"].pop(next(iter(incomplete["china"]["assets"])))

        manifest = p.build_manifest(
            self.release, incomplete, "https://fleet.eternizedlab.com"
        )

        self.assertEqual(manifest["version"], "v2.7.0")
        self.assertNotIn("china", manifest)

    def test_mirror_asset_must_stay_under_its_versioned_public_origin(self):
        unsafe = copy.deepcopy(self.mirror)
        first = next(iter(unsafe["china"]["assets"].values()))
        first["url"] = "https://evil.example/fleet-linux-x64"

        manifest = p.build_manifest(
            self.release, unsafe, "https://fleet.eternizedlab.com"
        )

        self.assertEqual(manifest["version"], "v2.7.0")
        self.assertNotIn("china", manifest)


class SelectReleaseTests(unittest.TestCase):
    """The manifest must follow the newest *downloadable* release.

    A tag push creates the release record immediately, so `releases/latest` is
    assetless for as long as the Release workflow takes to sign and upload —
    the window that broke v2.10.3's Pages deploy on 2026-09-18.
    """

    def release(self, tag, published, *, assets=True):
        release, _ = fixture()
        release["tag_name"] = tag
        release["published_at"] = published
        if assets:
            for asset in release["assets"]:
                asset["browser_download_url"] = asset["browser_download_url"].replace(
                    "/v2.6.0/", f"/{tag}/"
                )
        else:
            release["assets"] = []
        return release

    def test_freshly_tagged_release_without_assets_is_skipped(self):
        complete = self.release("v2.10.3", "2026-09-17T20:02:37Z")
        just_tagged = self.release("v2.10.4", "2026-09-18T03:30:27Z", assets=False)

        picked = p.select_release([just_tagged, complete])

        self.assertEqual(picked["tag_name"], "v2.10.3")

    def test_newest_complete_release_wins_regardless_of_list_order(self):
        older = self.release("v2.10.2", "2026-09-16T22:05:08Z")
        newer = self.release("v2.10.3", "2026-09-17T20:02:37Z")

        picked = p.select_release([older, newer])

        self.assertEqual(picked["tag_name"], "v2.10.3")

    def test_prereleases_and_drafts_are_never_picked(self):
        stable = self.release("v2.10.3", "2026-09-17T20:02:37Z")
        prerelease = self.release("v2.11.0", "2026-09-18T01:00:00Z")
        prerelease["prerelease"] = True
        draft = self.release("v2.12.0", "2026-09-18T02:00:00Z")
        draft["draft"] = True

        picked = p.select_release([draft, prerelease, stable])

        self.assertEqual(picked["tag_name"], "v2.10.3")

    def test_no_downloadable_release_is_an_error_rather_than_a_blank_manifest(self):
        with self.assertRaises(ValueError):
            p.select_release([self.release("v2.10.4", "2026-09-18T03:30:27Z", assets=False)])


class PublicationContractTests(unittest.TestCase):
    def test_release_calls_reusable_pages_workflow_after_publish(self):
        pages = (ROOT / ".github/workflows/pages.yml").read_text()
        release = (ROOT / ".github/workflows/release.yml").read_text()

        self.assertIn("workflow_call:", pages)
        self.assertIn("python3 scripts/site/pages_manifest.py", pages)
        self.assertIn("needs: [resolve, publish, china-distribution]", release)
        self.assertIn("uses: ./.github/workflows/pages.yml", release)

    def test_site_requires_live_mirror_to_match_the_github_version(self):
        script = (ROOT / "docs/site.js").read_text()
        self.assertIn("manifest.version !== expectedVersion", script)
        self.assertIn("url.href !== expectedURL.href", script)
        self.assertIn("fetch(liveURL", script)
        self.assertIn("note.dataset.version.replace(", script)

    def test_generated_pages_have_distinct_github_and_mirror_messages(self):
        for name in ("index.html", "zh/index.html"):
            with self.subTest(name=name):
                page = (ROOT / "docs" / name).read_text()
                self.assertIn('id="source-note" data-version=', page)
                self.assertIn('data-ready=', page)

    def test_shenzhen_manifest_allows_only_the_pages_origin(self):
        nginx = (ROOT / "docs/deploy/fleet-selfhost.nginx.conf").read_text()
        self.assertIn(
            'add_header Access-Control-Allow-Origin "https://hoveychen.github.io" always;',
            nginx,
        )

if __name__ == "__main__":
    unittest.main()
