import importlib.util
import tempfile
import subprocess
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("release_assets", "scripts/release-assets.py")
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)

class AssetCollectionTests(unittest.TestCase):
    def fixture(self, root):
        binary = root / "built"
        binary.write_bytes(b"compiled artifact")
        for platform in assets.PLATFORMS:
            assets.stage(binary, platform, root / "artifacts" / platform, commit="a" * 40)
        return root / "artifacts"

    def test_collects_exact_verified_versioned_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = self.fixture(root)
            output = root / "release"
            assets.collect(source, output, "v" + assets.version(), "a" * 40)
            for platform in assets.PLATFORMS:
                self.assertEqual((output / assets.asset_name(assets.version(), platform)).read_bytes(), b"compiled artifact")
            self.assertEqual(len((output / "checksums.txt").read_text().splitlines()), 4)

    def test_rejects_corrupt_missing_duplicate_and_mismatched_artifacts(self):
        for case in ["corrupt", "missing", "duplicate", "version", "tag", "commit"]:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = self.fixture(root)
                linux = source / "linux_amd64"
                binary = linux / assets.asset_name(assets.version(), "linux_amd64")
                if case == "corrupt": binary.write_bytes(b"corrupted bytes")
                elif case == "missing": binary.unlink()
                elif case == "duplicate":
                    duplicate = source / "duplicate"
                    duplicate.mkdir()
                    (duplicate / binary.name).write_bytes(binary.read_bytes())
                elif case == "version": (linux / "version.txt").write_text("0.0.0-mismatch")
                elif case == "commit": (linux / "source-commit.txt").write_text("b" * 40)
                tag = "v0.0.0-mismatch" if case == "tag" else "v" + assets.version()
                with self.assertRaises(ValueError): assets.collect(source, root / "release", tag, "a" * 40)


    def test_same_named_branch_cannot_impersonate_tag_checkout(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            def git(*args):
                return subprocess.check_output(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args], cwd=root, text=True).strip()
            git("init", "--quiet")
            (root / "file").write_text("tag bytes")
            git("add", "file")
            git("commit", "--quiet", "-m", "tag state")
            tag = "v1.2.3-preview.1"
            git("tag", tag)
            tag_commit = git("rev-parse", "HEAD")
            git("checkout", "--quiet", "-b", tag)
            (root / "file").write_text("branch bytes")
            git("commit", "--quiet", "-am", "branch state")
            with self.assertRaises(ValueError): assets.verify_tag_checkout(tag, root)
            git("checkout", "--quiet", "--detach", "refs/tags/" + tag)
            self.assertEqual(assets.verify_tag_checkout(tag, root), tag_commit)
