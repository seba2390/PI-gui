#!/usr/bin/env bash
# Prints the GitHub release notes (Markdown) for a version: how to install it, then its section
# of CHANGELOG.md. Usage: script/release-notes.sh 0.5.0
set -euo pipefail
version="${1:?usage: script/release-notes.sh <version>}"
cd "$(dirname "$0")/.."
changes="$(awk -v heading="## [$version]" '
  index($0, heading) == 1 { on = 1; next }
  on && (/^## / || /^\[[^]]+\]: /) { exit }
  on' CHANGELOG.md)"
[[ -n "${changes//[[:space:]]/}" ]] || { echo "CHANGELOG.md has no section for $version" >&2; exit 1; }
cat <<NOTES
## Install

You need [pi](https://pi.dev) (\`npm i -g @earendil-works/pi-coding-agent\`, Node.js 22.19+).

Already using Pistachio? Use **Update** in the update prompt (or **Check for Updates…**).

- **macOS (Apple Silicon)** with [Homebrew](https://brew.sh):
  \`\`\`bash
  brew install --cask seba2390/tap/pistachio
  \`\`\`
  Or download \`Pistachio-v$version-macos-arm64.zip\`, unzip it and move \`Pistachio.app\` to Applications. It isn't notarized yet, so allow the first launch once with:
  \`\`\`bash
  xattr -dr com.apple.quarantine /Applications/Pistachio.app
  \`\`\`
- **Linux (x86_64 and ARM64):**
  - Debian, Ubuntu: \`sudo apt install ./pistachio_$version-1_amd64.deb\` (ARM: \`_arm64.deb\`)
  - Fedora and other RPM-based systems: \`sudo dnf install ./pistachio-$version-1.x86_64.rpm\` (ARM: \`.aarch64.rpm\`)
  - Anything else: unpack \`pistachio-v$version-linux-x86_64.tar.gz\` (ARM: \`linux-aarch64\`) and run \`./install.sh\`.
- **Windows (x86_64, experimental):** download \`pistachio-v$version-windows-x86_64.zip\`, unzip it and run \`pistachio.exe\`.

Each download includes \`THIRD-PARTY-LICENSES.txt\`.

## What's new in $version
$changes
NOTES
