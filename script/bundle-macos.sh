#!/usr/bin/env bash
# Wraps a built pi-gui binary in a minimal macOS .app bundle.
# Usage: script/bundle-macos.sh [debug|release]   (default: debug)
set -euo pipefail
profile="${1:-debug}"
root="$(cd "$(dirname "$0")/.." && pwd)"
binary="$root/target/$profile/pi-gui"
app="$root/target/$profile/PI.app"
version="$(grep -m1 '^version' "$root/Cargo.toml" | cut -d'"' -f2)"

[[ -x "$binary" ]] || { echo "build first: cargo build ${profile/debug/}" >&2; exit 1; }
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/pi-gui"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>PI</string>
  <key>CFBundleDisplayName</key><string>PI</string>
  <key>CFBundleIdentifier</key><string>dev.pi-gui.app</string>
  <key>CFBundleExecutable</key><string>pi-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1 || true
echo "$app"
