"""Report lifetime rolling-installer downloads, including archived asset counts."""

import json
import subprocess

from archive_dmg_download_metrics import platform_totals


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True)


def main():
    repo = gh(
        "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"
    ).strip()
    release = json.loads(gh("api", f"repos/{repo}/releases/tags/updater"))
    pages = json.loads(
        gh(
            "api",
            "--paginate",
            "--slurp",
            f"repos/{repo}/releases/{release['id']}/assets?per_page=100",
        )
    )
    assets = [asset for page in pages for asset in page]
    snapshots = [
        json.loads(
            gh("release", "download", "updater", "--pattern", asset["name"], "-O", "-")
        )
        for asset in assets
        if asset["name"].startswith(
            ("dmg-download-metrics-", "windows-download-metrics-")
        )
        and asset["name"].endswith(".json")
    ]
    current = [
        {
            "asset_id": asset["id"],
            "asset_name": asset["name"],
            "downloads": asset["download_count"],
        }
        for asset in assets
    ]
    print(json.dumps(platform_totals(snapshots, current), indent=2))


if __name__ == "__main__":
    main()
