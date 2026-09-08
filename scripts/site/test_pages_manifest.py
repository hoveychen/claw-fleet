"""Pages manifest tests; all inputs are local fixtures, never the network."""
import copy
import unittest

import pages_manifest as p
from test_distribute import fixture


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

        self.assertEqual(manifest, {"schema": 1, "version": "v2.7.0"})

    def test_mirror_asset_must_stay_under_its_versioned_public_origin(self):
        unsafe = copy.deepcopy(self.mirror)
        first = next(iter(unsafe["china"]["assets"].values()))
        first["url"] = "https://evil.example/fleet-linux-x64"

        manifest = p.build_manifest(
            self.release, unsafe, "https://fleet.eternizedlab.com"
        )

        self.assertEqual(manifest, {"schema": 1, "version": "v2.7.0"})


if __name__ == "__main__":
    unittest.main()
