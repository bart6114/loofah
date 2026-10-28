import argparse
import json
import os
from pathlib import Path


def snapshot_filename(*, asset_id: int, downloads: int) -> str:
    return f"dmg-download-metrics-{asset_id}-{downloads}.json"


def build_snapshot(
    *,
    asset_id: int,
    asset_name: str,
    downloads: int,
    archived_at: str,
) -> dict:
    if asset_id <= 0:
        raise ValueError("asset_id must be positive")
    if downloads < 0:
        raise ValueError("downloads must be non-negative")
    return {
        "schema_version": 1,
        "asset_id": asset_id,
        "asset_name": asset_name,
        "downloads": downloads,
        "archived_at": archived_at,
    }


def lifetime_total(
    snapshots: list[dict],
    *,
    current_asset_id: int,
    current_downloads: int,
) -> int:
    downloads_by_asset: dict[int, int] = {}
    for snapshot in snapshots:
        asset_id = snapshot["asset_id"]
        downloads_by_asset[asset_id] = max(
            downloads_by_asset.get(asset_id, 0), snapshot["downloads"]
        )
    downloads_by_asset[current_asset_id] = max(
        downloads_by_asset.get(current_asset_id, 0), current_downloads
    )
    return sum(downloads_by_asset.values())


def platform_totals(
    snapshots: list[dict], current_assets: list[dict]
) -> dict[str, int]:
    names = {
        "Loofah_latest_aarch64.dmg": "macos",
        "Loofah_latest_x86_64-setup.exe": "windows_x64",
        "Loofah_latest_aarch64-setup.exe": "windows_arm64",
    }
    counts: dict[int, tuple[str, int]] = {}
    for record in snapshots + current_assets:
        platform = names.get(record["asset_name"])
        if platform is None:
            continue
        asset_id = record["asset_id"]
        previous = counts.get(asset_id, (platform, 0))[1]
        counts[asset_id] = (platform, max(previous, record["downloads"]))
    totals = dict.fromkeys(names.values(), 0)
    for platform, count in counts.values():
        totals[platform] += count
    totals["windows"] = totals["windows_x64"] + totals["windows_arm64"]
    return totals


def write_snapshot(
    path: Path,
    *,
    asset_id: int,
    asset_name: str,
    downloads: int,
    archived_at: str,
) -> None:
    snapshot = build_snapshot(
        asset_id=asset_id,
        asset_name=asset_name,
        downloads=downloads,
        archived_at=archived_at,
    )
    temporary = path.with_suffix(f"{path.suffix}.tmp")
    temporary.write_text(f"{json.dumps(snapshot, indent=2)}\n")
    os.replace(temporary, path)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--path", required=True, type=Path)
    parser.add_argument("--asset-id", required=True, type=int)
    parser.add_argument("--asset-name", required=True)
    parser.add_argument("--downloads", required=True, type=int)
    parser.add_argument("--archived-at", required=True)
    args = parser.parse_args()
    write_snapshot(
        args.path,
        asset_id=args.asset_id,
        asset_name=args.asset_name,
        downloads=args.downloads,
        archived_at=args.archived_at,
    )


if __name__ == "__main__":
    main()
