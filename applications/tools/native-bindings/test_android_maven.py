from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest

import android_maven as maven


class AndroidMavenTests(unittest.TestCase):
    def fixture(self):
        return {'schema_version': 1,
                'configurations': {name: ['example:library:1'] for name in maven.CONFIGURATIONS},
                'project_components': {name: ["project :app"] for name in maven.CONFIGURATIONS},
                'artifacts': {name: [{'coordinate': 'example:library:1', 'file_name': 'library-1.aar', 'sha256': 'a' * 64}] for name in maven.CONFIGURATIONS}}

    def check(self, actual, baseline=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            raw, expected, output = (root / name for name in ('raw.json', 'baseline.json', 'report.json'))
            raw.write_text(json.dumps(actual))
            expected.write_text(json.dumps(baseline or self.fixture()))
            return maven.check_report(raw, output, expected)

    def test_records_separate_profiles_hashes_and_explicit_resolution_limits(self):
        actual = self.fixture()
        actual['configurations']['releaseRuntimeClasspath'].append('example:release-only:2')
        report = self.check(actual, deepcopy(actual))
        self.assertEqual(report['union'], ['example:library:1', 'example:release-only:2'])
        self.assertNotEqual(report['configurations']['debugRuntimeClasspath'], report['configurations']['releaseRuntimeClasspath'])
        self.assertEqual(len(report['baseline_sha256']), 64)
        self.assertIn('not compiled', report['qualification'])
        self.assertEqual(report['artifacts']['debugRuntimeClasspath'][0]['sha256'], 'a' * 64)

    def test_variant_drift_is_not_hidden_by_an_unchanged_union(self):
        actual, baseline = self.fixture(), self.fixture()
        actual['configurations']['debugRuntimeClasspath'].append('example:extra:1')
        baseline['configurations']['releaseRuntimeClasspath'].append('example:extra:1')
        actual['configurations']['debugRuntimeClasspath'].sort()
        baseline['configurations']['releaseRuntimeClasspath'].sort()
        with self.assertRaisesRegex(ValueError, 'debugRuntimeClasspath.*releaseRuntimeClasspath'):
            maven.compare(actual, baseline)

    def test_release_profile_cannot_be_omitted(self):
        actual = self.fixture()
        del actual['configurations']['releaseRuntimeClasspath']
        with self.assertRaisesRegex(ValueError, 'Both exact'):
            self.check(actual)

    def test_duplicate_unsorted_or_malformed_coordinates_fail(self):
        for values in (['example:library:1'] * 2, ['z:a:1', 'a:a:1'], ['not-a-coordinate'], []):
            with self.subTest(values=values):
                actual = self.fixture()
                actual['configurations']['debugRuntimeClasspath'] = values
                with self.assertRaises(ValueError):
                    self.check(actual)

    def test_unknown_unhashed_or_duplicate_artifact_evidence_fails(self):
        for field, value in (('coordinate', 'unlisted:artifact:1'), ('sha256', ''), ('file_name', '/private/cache/file.aar')):
            with self.subTest(field=field):
                actual = self.fixture()
                actual['artifacts']['debugRuntimeClasspath'][0][field] = value
                with self.assertRaises(ValueError):
                    self.check(actual)
        actual = self.fixture()
        actual['artifacts']['debugRuntimeClasspath'] *= 2
        with self.assertRaisesRegex(ValueError, 'Duplicate artifact'):
            self.check(actual)

    def test_gradle_task_resolves_both_profiles_without_assemble_or_sign_tasks(self):
        script = maven.gradle_init()
        for name in maven.CONFIGURATIONS:
            self.assertIn(name, script)
        self.assertIn('UnresolvedDependencyResult', script)
        self.assertIn('ModuleComponentIdentifier', script)
        self.assertNotIn('dependsOn', script)
        self.assertNotIn('assembleRelease', script)


if __name__ == '__main__':
    unittest.main()
