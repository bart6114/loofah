"""Report native outputs separately from the cache action's archive-hit result."""

import argparse
import json
import os
from pathlib import Path


def inventory(targets):
    units = []
    for target in targets:
        for scratch in sorted(
            target.glob("release/build/transcribe-soniqo-*/out/swift-rs/soniqo-swift")
        ):
            profile = scratch / "release"
            if not profile.is_dir():
                profile = scratch / "arm64-apple-macosx/release"
            files = {}
            for name in ("libsoniqo-swift.a", "mlx.metallib"):
                path = profile / name
                stat = path.stat() if path.is_file() else None
                files[name] = (
                    {"bytes": stat.st_size, "mtime_ns": stat.st_mtime_ns}
                    if stat
                    else None
                )
            complete = all(value and value["bytes"] > 0 for value in files.values())
            units.append(
                {"scratch": str(scratch), "files": files, "complete": complete}
            )
    return units


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=("restored", "built"))
    parser.add_argument("--require-units", type=int, default=0)
    args = parser.parse_args()
    targets = (
        [Path(os.environ["CARGO_TARGET_DIR"])]
        if os.environ.get("CARGO_TARGET_DIR")
        else [Path("target"), Path("apps/desktop/src-tauri/target")]
    )
    units = inventory(targets)
    report = {
        "phase": args.phase,
        "runner_image": os.environ.get("ImageVersion"),
        "units": units,
    }
    output = json.dumps(report, indent=2) + "\n"
    root = Path(os.environ["RUNNER_TEMP"]) / "build-metrics"
    root.mkdir(parents=True, exist_ok=True)
    (root / f"native-cache-{args.phase}.json").write_text(output)
    print(output)
    complete = sum(unit["complete"] for unit in units)
    if complete < args.require_units:
        raise SystemExit(
            f"Expected {args.require_units} complete native build units; found {complete}"
        )


if __name__ == "__main__":
    main()
