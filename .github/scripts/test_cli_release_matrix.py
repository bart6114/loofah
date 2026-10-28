import unittest

from cli_release_matrix import TARGETS, missing_targets


class ReleaseMatrixTests(unittest.TestCase):
    def complete_assets(self):
        return {
            f"loof-1.2.3-{triple}.tar.gz{suffix}"
            for triple in TARGETS
            for suffix in ("", ".sha256")
        }

    def test_standalone_builds_every_missing_platform(self):
        self.assertEqual(
            {row["triple"] for row in missing_targets("v1.2.3", set())}, set(TARGETS)
        )

    def test_parallel_desktop_build_excludes_only_apple_silicon(self):
        self.assertEqual(
            {row["triple"] for row in missing_targets("v1.2.3", set(), True)},
            set(TARGETS) - {"aarch64-apple-darwin"},
        )

    def test_completed_release_needs_no_build(self):
        self.assertEqual(missing_targets("v1.2.3", self.complete_assets()), [])

    def test_missing_checksum_requires_rebuild(self):
        assets = self.complete_assets()
        assets.remove("loof-1.2.3-x86_64-apple-darwin.tar.gz.sha256")
        self.assertEqual(
            missing_targets("v1.2.3", assets),
            [{"triple": "x86_64-apple-darwin", "runner": "macos-15-intel"}],
        )

    def test_previous_version_does_not_satisfy_release(self):
        self.assertEqual(len(missing_targets("v1.2.4", self.complete_assets())), 4)


if __name__ == "__main__":
    unittest.main()
