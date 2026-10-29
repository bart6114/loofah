#!/usr/bin/env python3
"""Time a build stage and preserve its Cargo reports before cache cleanup."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


def main():
    stage, *command = sys.argv[1:]
    if command and command[0] == "--":
        command.pop(0)
    root = Path(os.environ["RUNNER_TEMP"]) / "build-metrics"
    root.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    result = subprocess.run(command, check=False)
    duration = time.monotonic() - started
    (root / f"{stage}.json").write_text(
        json.dumps(
            {"stage": stage, "seconds": duration, "exit_code": result.returncode}
        )
        + "\n"
    )
    if stage in ("cli", "desktop"):
        target = Path(
            os.environ.get(
                "CARGO_TARGET_DIR",
                "target" if stage == "cli" else "apps/desktop/src-tauri/target",
            )
        )
        timings = target / "cargo-timings"
        if timings.exists():
            shutil.copytree(timings, root / stage, dirs_exist_ok=True)
        # Cargo hides successful build-script output, including Swift's compile log.
        for unit in target.glob("release/build/transcribe-soniqo-*"):
            for name in ("output", "stderr"):
                source = unit / name
                if source.is_file():
                    destination = root / stage / "native-logs" / unit.name / name
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(source, destination)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
        summary.write(f"- {stage}: {duration:.1f}s (exit {result.returncode})\n")
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
