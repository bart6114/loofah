#!/bin/zsh
# Generate the menu-bar tray icon set from the canonical Loofah mark.
# States: default, recording pulse frames, degraded (!), and update (down arrow).
set -euo pipefail
cd "$(dirname "$0")/.."   # plugins/tray

MARK=../../apps/desktop/src-tauri/icons/src/loofah-mark-1024.png
CANVAS=160
# The source has a nearly transparent fringe that makes a normal trim shrink the visible mark.
MARK_CROP=$(magick "$MARK" -alpha extract -threshold 1% -format '%@' info:)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

render_mark() { # $1 size, $2 output, $3 optional x offset, $4 optional y offset
  local size=$1 out=$2 x=${3:-0} y=${4:-0}
  magick "$MARK" -crop "$MARK_CROP" +repage \
    -channel RGB -fill white -colorize 100 +channel \
    -resize "${size}x${size}" \
    -background none -gravity center -extent "${CANVAS}x${CANVAS}" \
    -roll "+${x}+${y}" "$out"
}

add_badge() { # $1 base, $2 output, $3 badge cutout drawing
  local base=$1 out=$2 cutout=$3
  magick "$base" \
    \( -size "${CANVAS}x${CANVAS}" xc:none -fill white -draw 'circle 121,121 121,79' \) \
    -compose DstOut -composite -compose Over \
    -fill white -draw 'circle 121,121 121,84' \
    \( -size "${CANVAS}x${CANVAS}" xc:none -fill white -draw "$cutout" \) \
    -compose DstOut -composite -compose Over "$out"
}

render_mark 150 icons/tray_default.png

# A subtle size pulse remains legible at macOS menu-bar scale while preserving
# the complete mark in every frame.
render_mark 130 icons/tray_recording_0.png
render_mark 140 icons/tray_recording_1.png
render_mark 150 icons/tray_recording_2.png
render_mark 140 icons/tray_recording_3.png

render_mark 126 "$T/badge-base.png" -13 -13
add_badge "$T/badge-base.png" icons/tray_degraded.png \
  'roundrectangle 116,95 126,126 5,5 circle 121,137 121,132'
add_badge "$T/badge-base.png" icons/tray_update.png \
  'roundrectangle 116,94 126,124 5,5 polygon 105,119 137,119 121,139'

echo "tray icons regenerated"
