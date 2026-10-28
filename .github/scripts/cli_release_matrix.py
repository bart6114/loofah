#!/usr/bin/env python3
"""Select missing CLI assets without rebuilding the desktop-provided sidecar."""

import json
import os
import sys

TARGETS = {
    "aarch64-apple-darwin": "macos-26",
    "x86_64-apple-darwin": "macos-15-intel",
    "x86_64-unknown-linux-gnu": "ubuntu-22.04",
    "aarch64-unknown-linux-gnu": "ubuntu-22.04-arm",
}


def missing_targets(tag, assets, skip_apple_silicon=False):
    version = tag.removeprefix("v")
    missing = []
    for triple, runner in TARGETS.items():
        if skip_apple_silicon and triple == "aarch64-apple-darwin":
            continue
        asset = f"loof-{version}-{triple}.tar.gz"
        if asset not in assets or f"{asset}.sha256" not in assets:
            missing.append({"triple": triple, "runner": runner})
    return missing


if __name__ == "__main__":
    targets = missing_targets(
        sys.argv[1],
        set(sys.stdin.read().splitlines()),
        os.environ.get("SKIP_APPLE_SILICON") == "true",
    )
    print(f"build_required={str(bool(targets)).lower()}")
    print(f"matrix={json.dumps({'include': targets})}")
