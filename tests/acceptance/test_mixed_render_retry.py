"""Negative witnesses for failed report paths and empty retained scratch."""
import tempfile
import unittest
from pathlib import Path
from mixed_render_retry import FAILED_STEMS, failed_artifact, assert_retained_scratch


class RecoveryWitnessTests(unittest.TestCase):
    def test_failed_files_and_nested_companion_assets_are_rejected(self):
        for stem in FAILED_STEMS:
            for path in [stem + '.html', stem + '.pdf',
                         'published/' + stem + '_files/libs/foo.js',
                         'published/nested/' + stem + '.qmd']:
                with self.subTest(path=path):
                    self.assertTrue(failed_artifact(path, allow_root_source=True))
        self.assertFalse(failed_artifact('published/04-kept_files/libs/foo.js'))

    def test_only_intended_root_sources_get_the_source_exception(self):
        for stem in FAILED_STEMS:
            path = stem + '.qmd'
            self.assertFalse(failed_artifact(path, allow_root_source=True))
            self.assertTrue(failed_artifact(path))
            self.assertTrue(failed_artifact(stem + '-unexpected.qmd', allow_root_source=True))

    def test_missing_empty_scratch_is_not_preservation(self):
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory) / 'scratch'
            scratch.mkdir()
            assert_retained_scratch(scratch, {})
            scratch.rmdir()
            with self.assertRaisesRegex(AssertionError, 'disappeared'):
                assert_retained_scratch(scratch, {})


if __name__ == '__main__':
    unittest.main()
