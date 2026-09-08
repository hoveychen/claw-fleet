"""Integrity and publication-order tests; never contacts a cloud account."""
import hashlib
import io
import json
from pathlib import Path
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

    def test_invalid_public_origins(self):
        for value in ('http://example.com','https://u:p@example.com','https://example.com/?token=x','https://example.com/../bad'):
            with self.subTest(value=value),self.assertRaises(ValueError): d.validate_base_url(value)

if __name__=='__main__': unittest.main()
