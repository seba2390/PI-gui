#!/usr/bin/env bash
# Prints the tag (`v0.5.0`) this commit should be released as, or nothing when it is not a new
# release. It is one when Cargo.toml's version has a dated section in CHANGELOG.md and is newer
# than every tag; a tag that already points at this commit counts too, so that a failed release
# can be run again. Used by .github/workflows/tag-release.yml.
set -euo pipefail
cd "$(dirname "$0")/.."
version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
tag="v$version"
if ! grep -qE "^## \[${version//./\\.}\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md; then
  echo "CHANGELOG.md has no dated section for $version: nothing to release" >&2
  exit 0
fi
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  if [[ "$(git rev-list -n 1 "$tag")" == "$(git rev-parse HEAD)" ]]; then
    echo "$tag"
  else
    echo "$tag is already released: nothing to release" >&2
  fi
  exit 0
fi
newest="$( (git tag --list 'v[0-9]*'; echo "$tag") | sort -V | tail -n 1)"
if [[ "$newest" != "$tag" ]]; then
  echo "$tag is older than $newest: nothing to release" >&2
  exit 0
fi
echo "$tag"
