#!/usr/bin/env bash
# Moves the pins of the SearXNG that Pistachio downloads on request (crates/searxng): SearXNG to the
# newest commit, uv to the newest release, and the packages to what that SearXNG asks for. Needs uv
# and curl. Then run the live test (about 80 MB):
#
#   cargo test -p searxng --test live -- --ignored
set -euo pipefail
cd "$(dirname "$0")/.."
catalog=crates/searxng/src/catalog.rs
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

sha256() { if command -v sha256sum >/dev/null; then sha256sum; else shasum -a 256; fi | cut -d' ' -f1; }
size() { wc -c <"$1" | tr -d ' '; }
latest() { curl -fsSL "https://api.github.com/repos/$1"; }

commit=$(latest searxng/searxng/commits/master | sed -n 's/^  "sha": "\([0-9a-f]\{40\}\)".*/\1/p' | head -1)
uv=$(latest astral-sh/uv/releases/latest | sed -n 's/^  "tag_name": "\([^"]*\)".*/\1/p' | head -1)
python=$(sed -n 's/^pub const PYTHON_VERSION: &str = "\(.*\)";/\1/p' "$catalog")
[[ -n $commit && -n $uv && -n $python ]] || { echo "could not find the newest versions" >&2; exit 1; }

curl -fsSL -o "$work/searxng.tgz" "https://codeload.github.com/searxng/searxng/tar.gz/$commit"
tar_sha=$(gunzip -c "$work/searxng.tgz" | sha256)
tar -xzf "$work/searxng.tgz" -C "$work" --strip-components 1 "searxng-$commit/requirements.txt"
uv pip compile "$work/requirements.txt" --no-config --universal --python-version "${python%.*}" \
  --generate-hashes --no-annotate --no-header --quiet -o "$work/lock.txt"
cp "$work/lock.txt" crates/searxng/src/requirements.txt

# set <what the line starts with> <new value>: the first such line, after $from.
replace() {
  perl -0pi -e "s/\Q$1\E/$2/" "$catalog"
}
const() { sed -n "s/^pub const $1: [^=]*= \"\{0,1\}\([^\";]*\)\"\{0,1\};.*/\1/p" "$catalog"; }
digits() { perl -pe '1 while s/^(\d+)(\d{3})/$1_$2/' <<<"$1"; }

replace "$(const SEARXNG_COMMIT)" "$commit"
replace "$(perl -0ne 'print $1 if /SEARXNG_TAR_SHA256: &str =\s*"([0-9a-f]{64})"/' "$catalog")" "$tar_sha"
replace "SEARXNG_SIZE: u64 = $(const SEARXNG_SIZE)" "SEARXNG_SIZE: u64 = $(digits "$(size "$work/searxng.tgz")")"
replace "UV_VERSION: &str = \"$(const UV_VERSION)\"" "UV_VERSION: \\&str = \"$uv\""
for target in $(sed -n 's/^ *target: "\(.*\)",/\1/p' "$catalog"); do
  curl -fsSL -o "$work/uv.tgz" "https://github.com/astral-sh/uv/releases/download/$uv/uv-$target.tar.gz"
  SHA=$(sha256 <"$work/uv.tgz") SIZE=$(digits "$(size "$work/uv.tgz")") TARGET=$target perl -0pi -e '
    s/(target: "\Q$ENV{TARGET}\E",\s*uv_sha256: ")[0-9a-f]{64}(",\s*uv_size: )[0-9_]+/$1$ENV{SHA}$2$ENV{SIZE}/' "$catalog"
done
cargo fmt -p searxng
git --no-pager diff --stat -- crates/searxng
