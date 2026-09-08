"""Integrity and publication-order tests; never contacts a cloud account."""
import hashlib
import http.client
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


class Origin:
    """A GitHub Releases stand-in that drops the connection where told.

    `script` is consumed one entry per request: an int cuts the response off
    after that many bytes, 'fail' raises the exact exception the mirror kept
    hitting, and None serves the rest. `honor_range` off models a server that
    answers a Range request with 200 and the whole body.
    """

    def __init__(self, body, script, *, honor_range=True):
        self.body, self.script, self.honor_range = body, list(script), honor_range
        self.ranges = []

    def __call__(self, request, **kwargs):
        header = request.headers.get('Range')
        self.ranges.append(header)
        step = self.script.pop(0) if self.script else None
        if step == 'fail':
            raise http.client.RemoteDisconnected('Remote end closed connection without response')
        start = int(header.split('=')[1].split('-')[0]) if header and self.honor_range else 0
        chunk = self.body[start:]
        response = io.BytesIO(chunk if step is None else chunk[:step])
        response.status = 206 if header and self.honor_range else 200
        response.headers = {}
        return response


class ResumeTests(unittest.TestCase):
    """Downloading one asset must survive the drops seen on the mirror's route.

    Five consecutive unattended rounds died partway through the 230 MB asset
    set (RemoteDisconnected, or a read timeout), and because the whole round
    restarted from zero the mirror could not advance at all.
    """

    BODY = b'official release bytes, long enough to cut in half\n'

    def setUp(self):
        patcher = patch.object(d.time, 'sleep', lambda *_: None)
        patcher.start()
        self.addCleanup(patcher.stop)

    def asset(self, body=None):
        body = self.BODY if body is None else body
        return {'name': 'fleet-linux-x64', 'size': len(body),
                'digest': 'sha256:' + hashlib.sha256(body).hexdigest(),
                'browser_download_url':
                    f'https://github.com/{d.REPO}/releases/download/v2.6.0/fleet-linux-x64'}

    def run_download(self, origin, asset=None):
        asset = asset or self.asset()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / asset['name']
            partial = path.with_name(asset['name'] + '.partial')
            with patch.object(d.urllib.request, 'urlopen', side_effect=origin):
                d.download(asset, path, partial)
            return partial.read_bytes()

    def test_a_mid_asset_disconnect_resumes_from_the_bytes_already_on_disk(self):
        origin = Origin(self.BODY, [20, None])
        self.assertEqual(self.run_download(origin), self.BODY)
        self.assertEqual(origin.ranges, [None, 'bytes=20-'])

    def test_a_connection_that_dies_before_any_bytes_is_retried(self):
        origin = Origin(self.BODY, ['fail', 'fail', None])
        self.assertEqual(self.run_download(origin), self.BODY)
        self.assertEqual(origin.ranges, [None, None, None])

    def test_a_server_ignoring_range_restarts_instead_of_splicing(self):
        # Appending a 200 response to a half-written file yields a longer file
        # whose SHA-256 is wrong — silently unpublishable, so never append it.
        origin = Origin(self.BODY, [20, None], honor_range=False)
        self.assertEqual(self.run_download(origin), self.BODY)
        self.assertEqual(origin.ranges, [None, 'bytes=20-'])

    def test_verified_bad_bytes_are_discarded_rather_than_resumed_onto(self):
        # A full-length but corrupt partial can never be repaired by resuming:
        # keeping it would make every later attempt fail the same way.
        corrupt = b'x' * len(self.BODY)
        origin = Origin(corrupt, [None])
        real = Origin(self.BODY, [None])

        def serve(request, **kwargs):
            return (origin if origin.script else real)(request, **kwargs)

        self.assertEqual(self.run_download(serve), self.BODY)
        self.assertEqual(real.ranges, [None])  # started over, did not resume

    def test_exhausted_attempts_raise_the_last_failure(self):
        origin = Origin(self.BODY, ['fail'] * d.DOWNLOAD_ATTEMPTS)
        with self.assertRaises(OSError):
            self.run_download(origin)

    def test_a_partial_longer_than_the_asset_is_thrown_away(self):
        asset = self.asset()
        origin = Origin(self.BODY, [None])
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / asset['name']
            partial = path.with_name(asset['name'] + '.partial')
            partial.write_bytes(self.BODY + b'trailing junk')
            with patch.object(d.urllib.request, 'urlopen', side_effect=origin):
                d.download(asset, path, partial)
            self.assertEqual(partial.read_bytes(), self.BODY)
        self.assertEqual(origin.ranges, [None])

    def test_prepare_still_writes_the_manifest_when_every_asset_dropped_once(self):
        release, _ = fixture()
        body = release['assets'][0]
        self.assertEqual(body['size'], len(b'official release bytes\n'))
        payload = b'official release bytes\n'
        script = []
        for _ in release['assets']:
            script += [10, None]
        origin = Origin(payload, script)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            with patch.object(d.urllib.request, 'urlopen', side_effect=origin):
                manifest = d.prepare(release, out, 'https://dl.example.com/fleet')
            self.assertEqual(set(manifest['china']['assets']), d.REQUIRED)
            self.assertTrue((out / 'downloads.json').is_file())
        self.assertEqual(origin.ranges.count('bytes=10-'), len(release['assets']))


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
        # Full-length but wrong bytes, so this fails verification rather than
        # looking truncated: the retry path must give up instead of publishing.
        release,body = fixture()
        corrupt = b'x' * len(body)
        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(d.time,'sleep',lambda *_: None), \
             patch.object(d.urllib.request,'urlopen',side_effect=lambda *a,**k:io.BytesIO(corrupt)):
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

    def test_each_deployment_gets_its_own_origin_substituted(self):
        # Both origins publish byte-identical files, so an absolute URL has to
        # be resolved at publish time or the mirror declares github.io its
        # canonical and drops out of Baidu while the origin site looks fine.
        release, body = fixture()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'site'
            shutil.copytree(d.ROOT / 'docs', root, ignore=shutil.ignore_patterns('releases'))
            (root / 'index.html').write_text(
                f'<link rel="canonical" href="{d.site_origin.TOKEN}/index.html">'
                f'<link rel="alternate" hreflang="zh-CN" href="{d.site_origin.TOKEN}/zh/index.html">'
                '<img src="./icon.png">')
            out = Path(tmp) / 'out'
            with patch.object(d.urllib.request, 'urlopen', side_effect=lambda *a, **k: io.BytesIO(body)):
                d.prepare(release, out, 'https://dl.example.com/fleet', site_root=root)
            published = (out / 'index.html').read_text()
            self.assertNotIn(d.site_origin.TOKEN, published)
            self.assertIn('https://dl.example.com/fleet/index.html', published)
            self.assertIn('https://dl.example.com/fleet/zh/index.html', published)
            # Binary assets still arrive byte-for-byte.
            self.assertEqual((out / 'icon.png').read_bytes(), (root / 'icon.png').read_bytes())

    def test_a_token_reference_is_not_looked_for_on_disk(self):
        # site_files() crawls src/href; a token URL is absolute once published,
        # not a path, so treating it as one would invent an unmirrorable file.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'zh').mkdir()
            for name in ('site.css', 'site.js', 'locale.js'):
                (root / name).write_text('')
            (root / 'index.html').write_text(
                f'<link rel="alternate" href="{d.site_origin.TOKEN}/zh/index.html"><img src="icon.png">')
            (root / 'zh/index.html').write_text('')
            (root / 'icon.png').write_bytes(b'')
            names = d.site_files(root)
            self.assertNotIn(f'{d.site_origin.TOKEN}/zh/index.html', names)
            self.assertFalse(any(d.site_origin.TOKEN in name for name in names))
            self.assertIn('icon.png', names)

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
