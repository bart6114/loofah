#!/usr/bin/env bash
set -euo pipefail
asset_name="$1"
prefix="$2"
release_id="$(gh api "repos/$GH_REPO/releases/tags/updater" --jq .id)"
snapshot_dir="$(mktemp -d)"
trap 'rm -rf "$snapshot_dir"' EXIT
for attempt in 1 2 3; do
  asset="$(gh api --paginate "repos/$GH_REPO/releases/$release_id/assets?per_page=100" --jq ".[] | select(.name == \"$asset_name\") | {id, download_count}")"
  [ -n "$asset" ] || exit 0
  asset_id="$(jq -r .id <<< "$asset")"
  downloads="$(jq -r .download_count <<< "$asset")"
  snapshot_name="$prefix-$asset_id-$downloads.json"
  python3 .github/scripts/archive_dmg_download_metrics.py \
    --path "$snapshot_dir/$snapshot_name" --asset-id "$asset_id" \
    --asset-name "$asset_name" --downloads "$downloads" \
    --archived-at "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  existing="$(gh api --paginate "repos/$GH_REPO/releases/$release_id/assets?per_page=100" --jq ".[] | select(.name == \"$snapshot_name\") | .id")"
  if [ -z "$existing" ]; then gh release upload updater "$snapshot_dir/$snapshot_name"; fi
  gh release download updater --pattern "$snapshot_name" -O - | jq -e \
    --argjson id "$asset_id" --argjson count "$downloads" --arg name "$asset_name" \
    '.schema_version == 1 and .asset_id == $id and .downloads == $count and .asset_name == $name' >/dev/null
  observed="$(gh api "repos/$GH_REPO/releases/assets/$asset_id" --jq .download_count)"
  if [ "$observed" = "$downloads" ]; then exit 0; fi
done
echo "Could not archive a stable download count for $asset_name" >&2
exit 1
