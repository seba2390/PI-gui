#!/usr/bin/env bash
# Wraps a built pistachio binary in a minimal macOS .app bundle.
# Usage: script/bundle-macos.sh [debug|release]   (default: debug)
set -euo pipefail
profile="${1:-debug}"
root="$(cd "$(dirname "$0")/.." && pwd)"
binary="$root/target/$profile/pistachio"
app="$root/target/$profile/Pistachio.app"
version="$(grep -m1 '^version' "$root/Cargo.toml" | cut -d'"' -f2)"

[[ -x "$binary" ]] || { echo "build first: cargo build ${profile/debug/}" >&2; exit 1; }
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/pistachio"
cp "$root/crates/app/assets/app-icon/icon.icns" "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Pistachio</string>
  <key>CFBundleDisplayName</key><string>Pistachio</string>
  <key>CFBundleIdentifier</key><string>dev.pistachio.app</string>
  <key>CFBundleExecutable</key><string>pistachio</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Pistachio listens while you dictate a message. Speech is transcribed on this Mac and is not sent anywhere.</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1 || true
echo "$app"
