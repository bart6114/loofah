"""Select expensive CI jobs from the complete commit diff."""

import os
import subprocess
from pathlib import Path


def scope(paths: list[str]) -> dict[str, bool]:
    result = {
        name: False for name in ("frontend", "desktop_rust", "windows", "cli", "docs")
    }
    for path in paths:
        if path.startswith(("docs/", "skills/")):
            result["docs"] = True
            continue
        if path.startswith(".agents/skills/") or path.endswith(".md"):
            continue
        if path.startswith(("apps/desktop/src/", "apps/desktop/public/")):
            result["frontend"] = True
            continue
        if (
            path.startswith("apps/desktop/")
            and Path(path).suffix in {".ts", ".tsx", ".css", ".json"}
            and "src-tauri/" not in path
        ):
            result["frontend"] = True
            continue
        if path.startswith(("packages/", "apps/desktop/")) and Path(path).suffix in {
            ".ts",
            ".tsx",
            ".css",
        }:
            result["frontend"] = True
            continue
        if path.startswith(("apps/cli/", "crates/", "plugins/")) or path in {
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
        }:
            result["desktop_rust"] = True
            result["windows"] = True
            result["cli"] = True
            continue
        # Build configuration, lockfiles, actions and unknown paths fail closed.
        return {name: True for name in result}
    return result


def changed_paths() -> list[str] | None:
    import json

    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    if os.environ["GITHUB_EVENT_NAME"] == "pull_request":
        base = event["pull_request"]["base"]["sha"]
    elif os.environ["GITHUB_EVENT_NAME"] == "push":
        base = event.get("before")
    else:
        return None
    if not base or set(base) == {"0"}:
        return None
    head = os.environ["GITHUB_SHA"]
    diff = subprocess.run(
        ["git", "diff", "--name-only", "-z", base, head],
        check=False,
        capture_output=True,
    )
    if diff.returncode:
        return None
    return [path.decode() for path in diff.stdout.split(b"\0") if path]


if __name__ == "__main__":
    paths = changed_paths()
    selected = (
        scope(paths)
        if paths
        else {
            name: True
            for name in ("frontend", "desktop_rust", "windows", "cli", "docs")
        }
    )
    print(f"Changed paths: {paths if paths is not None else 'full validation'}")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for name, enabled in selected.items():
            output.write(f"{name}={str(enabled).lower()}\n")
            print(f"{name}={str(enabled).lower()}")
