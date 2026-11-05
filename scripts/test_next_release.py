import unittest
from next_release import next_release


class ReleaseVersionTests(unittest.TestCase):
    def test_numeric_order_and_ignored_unstable_tags(self):
        self.assertEqual(next_release(["v0.2.9", "v0.2.10", "v9.0.0-beta", "notes"], []), "v0.2.11")

    def test_no_previous_tags(self):
        self.assertEqual(next_release([], []), "v0.0.1")

    def test_retry_reuses_tag_on_commit(self):
        self.assertEqual(next_release(["v0.2.3", "v0.2.4"], ["v0.2.3"]), "v0.2.3")

    def test_new_major_version(self):
        self.assertEqual(next_release(["v0.9.99", "v1.0.0"], []), "v1.0.1")


class FirmwareChangesTests(unittest.TestCase):
    def test_full_push_range_and_ignored_website_and_docs(self):
        import subprocess
        import tempfile
        from pathlib import Path
        from next_release import firmware_changed
        with tempfile.TemporaryDirectory() as folder:
            def git(*args):
                return subprocess.check_output(["git", "-C", folder, *args], text=True).strip()
            git("init", "-q")
            git("config", "user.email", "test@example.invalid")
            git("config", "user.name", "Test")
            def commit(path):
                file = Path(folder) / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text(path)
                git("add", ".")
                git("commit", "-qm", "test")
                return git("rev-parse", "HEAD")
            base = commit("README.md")
            website = commit("tools/autoflash/src/main.ts")
            docs = commit("apps/timer/README.md")
            self.assertFalse(firmware_changed(base, docs, folder))
            self.assertFalse(firmware_changed(website, docs, folder))
            commit("apps/timer/src/main.rs")
            head = commit("other.md")
            self.assertTrue(firmware_changed(base, head, folder))
            self.assertTrue(firmware_changed("0" * 40, head, folder))
            with self.assertRaises(subprocess.CalledProcessError):
                firmware_changed("missing-ref", head, folder)
