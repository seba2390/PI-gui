#!/usr/bin/env bash
# Regenerates crates/app/assets/app-icon/{icon.icns,icon-512.png} from icon.svg (macOS only).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="$root/crates/app/assets/app-icon"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
set_dir="$work/icon.iconset"; mkdir -p "$set_dir"
for size in 16 32 128 256 512; do
  swift "$root/script/render-icon.swift" "$dir/icon.svg" "$set_dir/icon_${size}x${size}.png" "$size"
  swift "$root/script/render-icon.swift" "$dir/icon.svg" "$set_dir/icon_${size}x${size}@2x.png" "$((size * 2))"
done
iconutil -c icns "$set_dir" -o "$dir/icon.icns"
cp "$set_dir/icon_256x256@2x.png" "$dir/icon-512.png"   # Linux .desktop / window icon
echo "$dir/icon.icns"
