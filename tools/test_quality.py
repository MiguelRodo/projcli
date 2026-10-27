"""Lightweight source-selection and formatting-isolation regressions."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock
import json
import quality


class ChangedSourceTests(unittest.TestCase):
    def test_status_parser_handles_spaces_renames_deletion_and_copy(self):
        raw = b"R100\0old file.rs\0new file.rs\0D\0gone.R\0C75\0one.R\0two.R\0M\0a\nb.R\0"
        self.assertEqual(quality.parse_changes(raw),
                         [("R", "old file.rs", "new file.rs"), ("D", None, "gone.R"),
                          ("C", "one.R", "two.R"), ("M", None, "a\nb.R")])

    def test_real_git_selection_and_no_applicable_sources(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            environment = dict(os.environ, GIT_CONFIG_NOSYSTEM="1",
                               GIT_CONFIG_GLOBAL=str(root / "missing-config"),
                               GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                               GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
            # No inherited routing/index overrides may redirect fixture operations.
            environment = {key: value for key, value in environment.items()
                           if not key.startswith("GIT_") or key in {
                               "GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL", "GIT_AUTHOR_NAME",
                               "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL"}}
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, env=environment).decode().strip()
            git("init", "-b", "main")
            (root / "old file.rs").write_text("pub fn old() {}\n")
            (root / "gone.R").write_text("gone <- 1\n")
            (root / "README.md").write_text("fixture\n")
            git("add", ".");git("commit", "-m", "base");base = git("rev-parse", "HEAD")
            git("mv", "old file.rs", "new file.rs")
            git("rm", "gone.R")
            (root / "code with spaces.R").write_text("value <- 1\n")
            (root / "line\nbreak.R").write_text("other <- 2\n")
            git("add", ".");git("commit", "-m", "head");head = git("rev-parse", "HEAD")
            result = quality.selection(root, base, head)
            self.assertEqual(result["rust"], ["new file.rs"])
            self.assertEqual(result["r"], ["code with spaces.R", "line\nbreak.R"])
            self.assertTrue(result["clippy"])
            git("commit", "--allow-empty", "-m", "unchanged")
            empty = quality.selection(root, head, git("rev-parse", "HEAD"))
            self.assertEqual(empty["rust"] + empty["r"], []);self.assertFalse(empty["clippy"])

    def test_explicit_base_cannot_bypass_pr_head_check(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            event = root / "event.json"
            event.write_text(json.dumps({"pull_request": {
                "base": {"sha": "a" * 40}, "head": {"sha": "b" * 40}}}))
            with mock.patch.dict(os.environ, {"GITHUB_EVENT_PATH": str(event)}):
                with mock.patch.object(quality, "git", return_value=("c" * 40).encode()):
                    with self.assertRaisesRegex(ValueError, "declared pull-request head"):
                        quality.refs(root, "a" * 40)

    def test_stdin_rustfmt_never_visits_or_modifies_unchanged_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            (root / "rustfmt.toml").write_text('edition = "2024"\nstyle_edition = "2024"\n')
            (root / "parent.rs").write_text("mod child;pub fn parent( ){ }\n")
            child = root / "child.rs"
            child.write_text("intentionally invalid Rust, must not be read or changed\n")
            before = child.read_bytes()
            result = quality.formatted(root, {"rust": ["parent.rs"], "r": []}, "unused")
            self.assertIn(b"mod child;", result["parent.rs"])
            self.assertEqual(child.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
