#!/usr/bin/env bash
# Sets the app's version for a release: Cargo.toml, Cargo.lock and the versioned install
# commands in README.md. Usage: script/set-version.sh 0.5.0   (then update CHANGELOG.md)
set -euo pipefail
new="${1:?usage: script/set-version.sh <version>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
old="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
[[ "$old" != "$new" ]] || { echo "already $new"; exit 0; }
sed -i.bak "s/^version = \"$old\"/version = \"$new\"/" Cargo.toml
# Only the release download links in the README (other mentions of a version are left alone).
sed -i.bak "/releases\/download\//s/$(printf '%s' "$old" | sed 's/\./\\./g')/$new/g" README.md
rm -f Cargo.toml.bak README.md.bak
cargo update -w -q
echo "$old -> $new (Cargo.toml, Cargo.lock, README.md)"
grep -n "releases/download/" README.md
