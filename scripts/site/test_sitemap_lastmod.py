#!/usr/bin/env python3
"""Tests for the publish-time sitemap lastmod stamping."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import seo
import sitemap_lastmod as sl

REPO = Path(__file__).resolve().parents[2]


def write_sitemap(root):
    path = root / 'sitemap.xml'
    path.write_text(seo.sitemap([('', 'zh/'), ('benchmark.html', 'zh/benchmark.html')]))
    return path


class StampTests(unittest.TestCase):
    def test_only_pages_with_a_date_get_a_lastmod(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_sitemap(Path(tmp))
            count = sl.stamp(path, {'/': '2026-09-01', '/zh/': '2026-09-02'})
            body = path.read_text()
            self.assertEqual(count, 2)
            self.assertIn('<loc>__SITE_ORIGIN__/</loc><lastmod>2026-09-01</lastmod>', body)
            self.assertIn('<loc>__SITE_ORIGIN__/zh/</loc><lastmod>2026-09-02</lastmod>', body)
            # The two we had no date for ship without one rather than with a guess.
            self.assertEqual(body.count('<lastmod>'), 2)
            self.assertIn('<loc>__SITE_ORIGIN__/benchmark.html</loc><xhtml:link', body)

    def test_no_dates_at_all_leaves_the_sitemap_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_sitemap(Path(tmp))
            before = path.read_text()
            self.assertEqual(sl.stamp(path, {}), 0)
            self.assertEqual(path.read_text(), before)

    def test_running_twice_does_not_stack_dates(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_sitemap(Path(tmp))
            sl.stamp(path, {'/': '2026-09-01'})
            sl.stamp(path, {'/': '2026-09-05'})
            body = path.read_text()
            self.assertEqual(body.count('<lastmod>'), 1)
            self.assertIn('<lastmod>2026-09-05</lastmod>', body)

    def test_the_stamped_sitemap_is_still_well_formed_and_ordered(self):
        # sitemaps.org requires lastmod to follow loc inside <url>.
        import xml.etree.ElementTree as ET
        with tempfile.TemporaryDirectory() as tmp:
            path = write_sitemap(Path(tmp))
            sl.stamp(path, {'/': '2026-09-01', '/zh/': '2026-09-02',
                            '/benchmark.html': '2026-09-03', '/zh/benchmark.html': '2026-09-04'})
            ns = {'s': 'http://www.sitemaps.org/schemas/sitemap/0.9'}
            root = ET.fromstring(path.read_text())
            for entry in root.findall('s:url', ns):
                tags = [child.tag.split('}')[-1] for child in entry]
                self.assertEqual(tags[:2], ['loc', 'lastmod'])


class RepoDatingTests(unittest.TestCase):
    def test_a_shallow_clone_yields_no_dates_instead_of_todays_date(self):
        # actions/checkout defaults to depth 1, where every file's last commit
        # is HEAD; stamping that would claim all four pages changed just now.
        with tempfile.TemporaryDirectory() as tmp:
            shallow = Path(tmp) / 'shallow'
            done = subprocess.run(['git', 'clone', '--depth', '1', '--no-local',
                                   f'file://{REPO}', str(shallow)],
                                  capture_output=True, text=True)
            if done.returncode != 0:
                self.skipTest('git clone unavailable: ' + done.stderr.strip()[:120])
            self.assertFalse(sl.is_usable_repo(shallow))
            self.assertEqual(sl.page_lastmod(shallow, shallow / 'docs'), {})

    def test_a_non_repo_yields_no_dates(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertFalse(sl.is_usable_repo(Path(tmp)))
            self.assertEqual(sl.page_lastmod(Path(tmp), Path(tmp)), {})

    def test_this_checkout_dates_every_page(self):
        dates = sl.page_lastmod(REPO, REPO / 'docs')
        self.assertEqual(set(dates), set(sl.PAGE_FILES))
        for path, date in dates.items():
            with self.subTest(path=path):
                self.assertRegex(date, r'^\d{4}-\d{2}-\d{2}$')

    def test_the_committed_sitemap_carries_no_dates(self):
        # Stamping happens at publish time; a date committed here would be one
        # commit stale exactly for the page that just changed.
        self.assertNotIn('<lastmod>', (REPO / 'docs/sitemap.xml').read_text())

    def test_the_workflow_stamps_before_staging_and_clones_deep(self):
        workflow = (REPO / '.github/workflows/pages.yml').read_text()
        self.assertIn('fetch-depth: 0', workflow)
        stamp_at = workflow.index('sitemap_lastmod.py')
        stage_at = workflow.index('stage_pages.py')
        self.assertLess(stamp_at, stage_at)


if __name__ == '__main__':
    unittest.main()
