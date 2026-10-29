import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("measure_build.py")


class BuildMeasurementTests(unittest.TestCase):
    def measure(self, root, stage, code, **extra_env):
        env = {
            **os.environ,
            "RUNNER_TEMP": str(root / "runner"),
            "GITHUB_STEP_SUMMARY": str(root / "summary.md"),
            **extra_env,
        }
        return subprocess.run(
            [sys.executable, str(SCRIPT), stage, "--", sys.executable, "-c", code],
            env=env,
            cwd=root,
            capture_output=True,
        )

    def test_failure_still_records_time_and_preserves_exit_code(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = self.measure(root, "bundle", "raise SystemExit(7)")
            self.assertEqual(result.returncode, 7, result.stderr)
            report = json.loads((root / "runner/build-metrics/bundle.json").read_text())
            self.assertEqual(report["exit_code"], 7)
            self.assertGreater(report["seconds"], 0)

    def test_shared_target_reports_do_not_overwrite_each_other(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "shared"
            timings = target / "cargo-timings"
            timings.mkdir(parents=True)
            for stage in ("cli", "desktop"):
                (timings / "cargo-timing.html").write_text(stage)
                result = self.measure(root, stage, "pass", CARGO_TARGET_DIR=str(target))
                self.assertEqual(result.returncode, 0, result.stderr)
            for stage in ("cli", "desktop"):
                self.assertEqual(
                    (
                        root / f"runner/build-metrics/{stage}/cargo-timing.html"
                    ).read_text(),
                    stage,
                )

    def test_native_logs_survive_failed_build_and_remain_separate_per_stage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            unit = "transcribe-soniqo-123"
            logs = target / "release/build" / unit
            logs.mkdir(parents=True)
            for stage in ("cli", "desktop"):
                (logs / "output").write_text(f"Swift compiled {stage}")
                (logs / "stderr").write_text("diagnostic")
                result = self.measure(
                    root, stage, "raise SystemExit(7)", CARGO_TARGET_DIR=str(target)
                )
                self.assertEqual(result.returncode, 7, result.stderr)
            for stage in ("cli", "desktop"):
                saved = root / "runner/build-metrics" / stage / "native-logs" / unit
                self.assertEqual(
                    (saved / "output").read_text(), f"Swift compiled {stage}"
                )
                self.assertEqual((saved / "stderr").read_text(), "diagnostic")

    def test_legacy_desktop_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            timings = root / "apps/desktop/src-tauri/target/cargo-timings"
            timings.mkdir(parents=True)
            (timings / "cargo-timing.html").write_text("legacy")
            result = self.measure(root, "desktop", "pass")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                (root / "runner/build-metrics/desktop/cargo-timing.html").read_text(),
                "legacy",
            )


if __name__ == "__main__":
    unittest.main()
