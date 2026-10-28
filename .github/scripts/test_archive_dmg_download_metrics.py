import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("archive_dmg_download_metrics.py")
spec = importlib.util.spec_from_file_location(
    "archive_dmg_download_metrics", MODULE_PATH
)
assert spec is not None
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)


class ArchiveDmgDownloadMetricsTests(unittest.TestCase):
    def test_platform_totals_preserve_history_without_double_counting(self):
        def row(asset_id, name, count):
            return {"asset_id": asset_id, "asset_name": name, "downloads": count}

        mac = "Loofah_latest_aarch64.dmg"
        x64 = "Loofah_latest_x86_64-setup.exe"
        arm = "Loofah_latest_aarch64-setup.exe"
        snapshots = [row(1, mac, 10), row(2, x64, 4), row(2, x64, 7), row(3, arm, 5)]
        current = [
            row(1, mac, 12),
            row(4, x64, 2),
            row(3, arm, 6),
            row(5, "latest-windows.json", 99),
        ]
        self.assertEqual(
            module.platform_totals(snapshots, current),
            {
                "macos": 12,
                "windows_x64": 9,
                "windows_arm64": 6,
                "windows": 15,
            },
        )

    def test_builds_immutable_snapshot(self) -> None:
        snapshot = module.build_snapshot(
            asset_id=123,
            asset_name="Loofah_latest_aarch64.dmg",
            downloads=7,
            archived_at="2026-09-01T08:00:00Z",
        )

        self.assertEqual(
            snapshot,
            {
                "schema_version": 1,
                "asset_id": 123,
                "asset_name": "Loofah_latest_aarch64.dmg",
                "downloads": 7,
                "archived_at": "2026-09-01T08:00:00Z",
            },
        )

    def test_snapshot_filename_identifies_asset_and_count(self) -> None:
        self.assertEqual(
            module.snapshot_filename(asset_id=123, downloads=7),
            "dmg-download-metrics-123-7.json",
        )

    def test_rejects_invalid_counts(self) -> None:
        with self.assertRaisesRegex(ValueError, "asset_id"):
            module.build_snapshot(
                asset_id=0,
                asset_name="Loofah_latest_aarch64.dmg",
                downloads=7,
                archived_at="2026-09-01T08:00:00Z",
            )
        with self.assertRaisesRegex(ValueError, "downloads"):
            module.build_snapshot(
                asset_id=123,
                asset_name="Loofah_latest_aarch64.dmg",
                downloads=-1,
                archived_at="2026-09-01T08:00:00Z",
            )

    def test_writes_snapshot_atomically(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dmg-download-metrics-123-7.json"

            module.write_snapshot(
                path,
                asset_id=123,
                asset_name="Loofah_latest_aarch64.dmg",
                downloads=7,
                archived_at="2026-09-01T08:00:00Z",
            )

            self.assertEqual(json.loads(path.read_text())["downloads"], 7)
            self.assertFalse(path.with_suffix(".json.tmp").exists())

    def test_lifetime_total_uses_highest_snapshot_per_asset(self) -> None:
        snapshots = [
            {"asset_id": 123, "downloads": 7},
            {"asset_id": 123, "downloads": 9},
            {"asset_id": 456, "downloads": 2},
        ]

        total = module.lifetime_total(
            snapshots,
            current_asset_id=789,
            current_downloads=3,
        )

        self.assertEqual(total, 14)

    def test_cli_writes_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dmg-download-metrics-123-7.json"
            result = subprocess.run(
                [
                    sys.executable,
                    str(MODULE_PATH),
                    "--path",
                    str(path),
                    "--asset-id",
                    "123",
                    "--asset-name",
                    "Loofah_latest_aarch64.dmg",
                    "--downloads",
                    "7",
                    "--archived-at",
                    "2026-09-01T08:00:00Z",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(path.read_text())["asset_id"], 123)


if __name__ == "__main__":
    unittest.main()
