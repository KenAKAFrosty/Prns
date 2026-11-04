from pathlib import Path
import hashlib
import gzip
import json
import shutil
import tempfile
import unittest
from validation.security import app_maven

ROOT = Path(__file__).resolve().parents[2]

class AppMavenEvidenceTests(unittest.TestCase):
    def copy_evidence(self, root):
        shutil.copytree(ROOT/'release/licenses/app-maven', root/'release/licenses/app-maven')
        destination=root/'applications/tools/native-bindings'
        destination.mkdir(parents=True)
        for name in ('android_maven.py', 'android-maven-baseline.json'):
            shutil.copyfile(ROOT/'applications/tools/native-bindings'/name, destination/name)

    def test_actual_closure_retains_nonstandard_terms_and_fails_unapproved_review(self):
        report=app_maven.report(ROOT)
        names={d['name'] for p in report['packages'] for d in p['declarations']}
        self.assertIn('ML Kit Terms of Service', names)
        self.assertEqual(sum(any(d['name'] == 'ML Kit Terms of Service' for d in p['declarations'])
                             for p in report['packages']), 8)
        self.assertEqual(sum(any('Android Software Development Kit' in d['name'] for d in p['declarations'])
                             for p in report['packages']), 4)
        self.assertTrue(any('Android Software Development Kit' in name for name in names))
        self.assertTrue(any(p['declared_at']!=p['coordinate'] for p in report['packages']))
        review = report['manual_review']
        coordinates = {item['name']: item['coordinates'] for item in review['declarations']}
        self.assertEqual(len(coordinates['ML Kit Terms of Service']), 8)
        self.assertEqual(coordinates['Android Software Development Kit License'], [
            'com.google.android.gms:play-services-base:18.5.0',
            'com.google.android.gms:play-services-basement:18.10.0',
            'com.google.android.gms:play-services-tasks:18.2.1',
            'com.google.android.odml:image:1.0.0-beta1'])
        self.assertEqual(coordinates['LGPL-2.1-or-later'], ['net.java.dev.jna:jna:5.18.1'])
        with self.assertRaisesRegex(ValueError,'manual release-policy review pending') as failure:
            app_maven.audit(report)
        self.assertIn('com.google.mlkit:barcode-scanning:17.3.0', str(failure.exception))
        self.assertIn('net.java.dev.jna:jna:5.18.1', str(failure.exception))
        self.assertNotIn('unresolved', report)

    def test_manual_review_is_not_cleared_by_absence_of_named_declarations(self):
        report = app_maven.report(ROOT)
        report['manual_review']['declarations'] = []
        with self.assertRaisesRegex(ValueError, 'manual release-policy review pending'):
            app_maven.audit(report)

    def test_changed_baseline_cannot_reuse_old_pom_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);self.copy_evidence(root)
            path=root/app_maven.BASELINE;data=json.loads(path.read_text())
            data['configurations']['releaseRuntimeClasspath'].append('z:new:1')
            path.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError,'differs'):
                app_maven.report(root)

    def test_hash_and_declared_licenses_both_bind_preserved_pom(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);self.copy_evidence(root)
            path=root/app_maven.EVIDENCE;data=json.loads(path.read_text())
            item=next(v for v in data['documents'].values() if v['declared_licenses'])
            pom=path.parent/item['source']['evidence_copy']
            original=pom.read_bytes();pom.write_bytes(original+b' ')
            with self.assertRaisesRegex(ValueError,'POM evidence changed'):
                app_maven.report(root)
            pom.write_bytes(original)
            item['declared_licenses'][0]['name']='invented permission'
            path.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError,'declarations disagree'):
                app_maven.report(root)

    def test_json_cannot_relabel_a_pom_coordinate_or_its_license_parent(self):
        for mutation, message in (('coordinate','coordinate disagrees'),('parent','parent disagrees')):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);self.copy_evidence(root)
                path=root/app_maven.EVIDENCE;data=json.loads(path.read_text())
                item=next(v for v in data['documents'].values() if v['parent'])
                item[mutation]='forged:permission:1'
                path.write_text(json.dumps(data))
                with self.assertRaisesRegex(ValueError,message):
                    app_maven.report(root)

    def test_notice_ranges_use_bytes_and_preserve_exact_component_text(self):
        raw = 'prefixé MIT\nother'.encode()
        index = json.dumps({'component': {'start': 6, 'length': len('é MIT'.encode())}}).encode()
        self.assertEqual(app_maven.notice_ranges(index, raw), [('component', 'é MIT')])
        self.assertEqual(app_maven.notice_ranges(b'{}', b''), [])

    def test_notice_ranges_reject_ambiguous_invalid_or_truncated_sources(self):
        cases = [
            (b'{"a":{"start":0,"length":2},"a":{"start":0,"length":2}}', b'ok'),
            (b'{"a":{"start":true,"length":1}}', b'ok'),
            (b'{"a":{"start":-1,"length":1}}', b'ok'),
            (b'{"a":{"start":0,"length":3}}', b'ok'),
            (b'{"a":{"start":0,"length":2},"b":{"start":1,"length":1}}', b'ok'),
            (b'{"a":{"start":1,"length":1}}', 'é'.encode()),
            (b'{}', b'unindexed license'),
        ]
        for index, contents in cases:
            with self.subTest(index=index), self.assertRaises((ValueError, UnicodeDecodeError)):
                app_maven.notice_ranges(index, contents)

    def test_notice_sources_reject_escape_and_bounded_decompression_overflow(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);directory=(root/app_maven.EVIDENCE).parent
            directory.mkdir(parents=True)
            with self.assertRaisesRegex(ValueError, 'escapes'):
                app_maven._source(root, {'path':'../../../outside'})
            raw=b'x'*(app_maven.MAX_NOTICE_BYTES+1)
            packed=gzip.compress(raw,mtime=0);(directory/'notice.gz').write_bytes(packed)
            source={'path':'notice.gz','encoding':'gzip','bytes':len(raw),
                    'file_sha256':hashlib.sha256(packed).hexdigest(),
                    'sha256':hashlib.sha256(raw).hexdigest()}
            with self.assertRaisesRegex(ValueError, 'bytes changed'):
                app_maven._source(root, source)

    def test_exact_artifact_and_pom_custody_are_required_without_approving_policy(self):
        self.assertIn(ROOT/'.gitattributes', app_maven.inputs(ROOT))
        actual = app_maven.report(ROOT)
        self.assertEqual(actual['artifact_evidence'], {'artifacts':180, 'pom_origins':204})
        self.assertTrue(any('Apache-2.0 OR LGPL-2.1-or-later' in row['text'] for row in actual['notices']))
        self.assertTrue(any(row['name'] == 'Android SDK Platforms' for row in actual['notices']))
        with self.assertRaisesRegex(ValueError, 'manual release-policy review pending'):
            app_maven.audit(actual)
        for mutation in ('artifact_hash', 'missing_artifact', 'notice_bytes', 'origin_url'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);self.copy_evidence(root)
                path=root/app_maven.ARTIFACTS;data=json.loads(path.read_text())
                if mutation=='artifact_hash':
                    data['artifacts'][0]['sha256']='0'*64
                elif mutation=='missing_artifact':
                    data['artifacts'].pop()
                elif mutation=='notice_bytes':
                    source=next(n['source'] for a in data['artifacts'] for n in a['notices'])
                    file=path.parent/source['path'];file.write_bytes(file.read_bytes()+b' ')
                else:
                    file=path.parent/data['pom_origins']['path'];origins=json.loads(file.read_text())
                    item=next(o for o in origins['documents'] if o['status']=='exact-pom-match')
                    item['verified']['url']='https://example.invalid/false-origin.pom'
                    file.write_text(json.dumps(origins))
                    data['pom_origins']['sha256']=hashlib.sha256(file.read_bytes()).hexdigest()
                path.write_text(json.dumps(data))
                with self.assertRaisesRegex(ValueError, 'Maven') as failure:
                    app_maven.report(root)
                self.assertNotIn('manual release-policy review', str(failure.exception))
