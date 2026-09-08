"""Integrity and publication-order tests; never contacts a cloud account."""
import hashlib
import io
import json
from pathlib import Path
import shutil
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
import distribute as d


def fixture():
    body = b'official release bytes\n'
    release = {'tag_name':'v2.6.0','draft':False,'prerelease':False,'assets':[
        {'name':name, 'size':len(body), 'digest':'sha256:'+hashlib.sha256(body).hexdigest(),
         'browser_download_url':f'https://github.com/{d.REPO}/releases/download/v2.6.0/{name}'}
        for name in sorted(d.REQUIRED)]}
    return release, body


class DistributionTests(unittest.TestCase):
    def test_only_stable_complete_official_releases(self):
        release,_ = fixture()
        d.validate_release(release)
        for change in ({'draft':True}, {'prerelease':True}, {'tag_name':'../../escape'}, {'assets':[]}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                d.validate_release(release | change)
        release['assets'][0]['browser_download_url'] = 'https://untrusted.example/file'
        with self.assertRaises(ValueError): d.validate_release(release)

    def test_missing_digest_is_rejected(self):
        release,_ = fixture()
        release['assets'][0]['digest'] = None
        with self.assertRaises(ValueError): d.validate_release(release)

    def test_bad_payload_does_not_write_manifest(self):
        release,_ = fixture()
        with tempfile.TemporaryDirectory() as tmp, patch.object(d.urllib.request,'urlopen',return_value=io.BytesIO(b'corrupt')):
            with self.assertRaises(ValueError): d.prepare(release,Path(tmp),'https://dl.example.com/fleet')
            self.assertFalse((Path(tmp)/'downloads.json').exists())

    def test_downloads_are_verified_and_manifest_is_published_last(self):
        release,body = fixture()
        with tempfile.TemporaryDirectory() as tmp:
            out=Path(tmp)
            with patch.object(d.urllib.request,'urlopen',side_effect=lambda *a,**k:io.BytesIO(body)):
                manifest=d.prepare(release,out,'https://dl.example.com/fleet')
            saved=json.loads((out/'downloads.json').read_text())
            self.assertEqual(saved,manifest)
            self.assertEqual(set(saved['china']['assets']),d.REQUIRED)
            for asset in saved['china']['assets'].values():
                self.assertTrue(asset['url'].startswith('https://dl.example.com/fleet/releases/v2.6.0/'))
            uploaded=[]
            client=types.SimpleNamespace(upload_file=lambda **kwargs:uploaded.append(kwargs['Key']))
            sdk=types.SimpleNamespace(CosConfig=lambda **kwargs:None,CosS3Client=lambda config:client)
            def head(request,**kwargs):
                path=out / request.full_url.split('/fleet/')[1]
                response=io.BytesIO(); response.headers={'Content-Length':str(path.stat().st_size)}
                return response
            env={k:'test' for k in ('COS_BUCKET','COS_REGION','COS_SECRET_ID','COS_SECRET_KEY')}
            with patch.dict(sys.modules,{'qcloud_cos':sdk}), patch.dict(d.os.environ,env), patch.object(d.urllib.request,'urlopen',side_effect=head):
                d.publish(out,manifest,'https://dl.example.com/fleet')
            self.assertEqual(uploaded[-1],'fleet/downloads.json')
            self.assertTrue(all(key.startswith('fleet/releases/') for key in uploaded[:5]))
            uploaded.clear()
            with patch.dict(sys.modules,{'qcloud_cos':sdk}), patch.dict(d.os.environ,env), patch.object(d.urllib.request,'urlopen',side_effect=OSError('public domain unavailable')):
                with self.assertRaises(OSError): d.publish(out,manifest,'https://dl.example.com/fleet')
            self.assertNotIn('fleet/downloads.json',uploaded)
            self.assertNotIn('fleet/index.html',uploaded)

    def test_android_apk_is_optional_but_mirrored_when_present(self):
        # Releases cut before the APK job existed must still mirror, so its
        # absence is not an error — but when it is there it has to reach COS,
        # otherwise Chinese users are left with the GitHub direct download.
        release,body = fixture()
        d.validate_release(release)
        self.assertNotIn('claw-fleet-android.apk', {a['name'] for a in release['assets']})
        release['assets'].append(dict(
            release['assets'][0],
            name='claw-fleet-android.apk',
            browser_download_url=f'https://github.com/{d.REPO}/releases/download/v2.6.0/claw-fleet-android.apk'))
        with tempfile.TemporaryDirectory() as tmp, patch.object(d.urllib.request,'urlopen',side_effect=lambda *a,**k:io.BytesIO(body)):
            manifest=d.prepare(release,Path(tmp),'https://dl.example.com/fleet')
        asset=manifest['china']['assets'].get('claw-fleet-android.apk')
        self.assertIsNotNone(asset)
        self.assertTrue(asset['url'].startswith('https://dl.example.com/fleet/releases/v2.6.0/'))

    def test_new_site_file_absent_from_an_older_site_root(self):
        # selfhost.py mirrors with site_root=<live deployment>. A deployment
        # published before icon-android.svg existed must still sync, or the
        # unattended updater wedges on FileNotFoundError.
        release,body = fixture()
        with tempfile.TemporaryDirectory() as tmp:
            older=Path(tmp)/'older'
            shutil.copytree(d.ROOT/'docs', older, ignore=shutil.ignore_patterns('releases'),
                            dirs_exist_ok=False)
            (older/'icon-android.svg').unlink()
            out=Path(tmp)/'out'
            with patch.object(d.urllib.request,'urlopen',side_effect=lambda *a,**k:io.BytesIO(body)):
                d.prepare(release,out,'https://dl.example.com/fleet',site_root=older)
            self.assertFalse((out/'icon-android.svg').exists())
            self.assertTrue((out/'index.html').is_file())
        # A genuinely required file missing is still a hard error.
        with tempfile.TemporaryDirectory() as tmp:
            broken=Path(tmp)/'broken'; broken.mkdir()
            with self.assertRaises(FileNotFoundError):
                d.prepare(release,Path(tmp)/'out2','https://dl.example.com/fleet',site_root=broken)

    def test_site_file_list_tracks_what_the_pages_reference(self):
        # The hand-maintained tuple this replaced had already gone stale:
        # agents-* and relay-* screenshots were on both pages and not in it.
        names = d.site_files(d.ROOT / 'docs')
        self.assertEqual(names[:2], ['index.html', 'zh/index.html'])
        # Sub-pages are followed transitively, with their own stylesheets.
        self.assertIn('benchmark.html', names)
        self.assertIn('zh/benchmark.html', names)
        self.assertIn('benchmark.css', names)
        for name in names:
            with self.subTest(name=name):
                self.assertTrue((d.ROOT / 'docs' / name).is_file(), name)
        for required in ('site.css', 'site.js', 'locale.js', 'icon-android.svg',
                         'screenshots/current/agents-en.png', 'screenshots/current/relay-zh.png'):
            self.assertIn(required, names)
        self.assertEqual(len(names), len(set(names)))
        pages=[n for n in names if n.endswith('.html')]
        self.assertEqual(names[:len(pages)], pages)  # pages first, then assets

    def test_site_reference_cannot_escape_the_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'zh').mkdir()
            (root / 'index.html').write_text('<img src="../../etc/passwd">')
            (root / 'zh/index.html').write_text('<img src="x.png">')
            with self.assertRaises(ValueError): d.site_files(root)

    def test_invalid_public_origins(self):
        for value in ('http://example.com','https://u:p@example.com','https://example.com/?token=x','https://example.com/../bad'):
            with self.subTest(value=value),self.assertRaises(ValueError): d.validate_base_url(value)

if __name__=='__main__': unittest.main()
