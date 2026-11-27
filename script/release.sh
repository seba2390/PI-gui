#!/usr/bin/env bash
# Releases a version in one step: sets the version, dates the changelog section, commits, pushes,
# opens a pull request into main and turns on auto-merge. GitHub does the rest (merge once the
# required checks pass, then tag-release.yml and release.yml), so nothing has to be watched.
#
#   script/release.sh 0.5.0 --docs-review /tmp/docs-review.md   # from the branch with the changes
#   script/release.sh 0.5.0 --dry-run    # check everything, change and push nothing
#
# --docs-review names the summary of the documentation review done before every release (see
# Releasing in CONTRIBUTING.md); it goes into the pull request. Keep it outside the repository.
#
# --skip-checks leaves out fmt, clippy and the tests (CI runs them too). Running it again after
# a failure goes on where it stopped.
set -euo pipefail

usage="usage: script/release.sh <version> --docs-review <file> [--dry-run] [--skip-checks]"
die() { echo "release.sh: $*" >&2; exit 1; }
step() { echo "==> $*"; }

version="" dry_run="" skip_checks="" docs_review=""
while (($#)); do
  case $1 in
    --dry-run) dry_run=1 ;;
    --skip-checks) skip_checks=1 ;;
    --docs-review) (($# > 1)) || die "$usage"; docs_review=$2; shift ;;
    --docs-review=*) docs_review=${1#--docs-review=} ;;
    -*) die "$usage" ;;
    *) [[ -z "$version" ]] || die "$usage"; version=$1 ;;
  esac
  shift
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$usage"
tag="v$version"

# Read before changing directory, so a relative path is the caller's.
review=""
if [[ -n "$docs_review" ]]; then
  [[ -f "$docs_review" ]] || die "$docs_review: no such file"
  review="$(<"$docs_review")"
  [[ -n "${review//[[:space:]]/}" ]] || die "$docs_review is empty: record the documentation review in it"
elif [[ -z "$dry_run" ]]; then
  die "review the documentation first and pass its summary with --docs-review <file> (see Releasing in CONTRIBUTING.md)"
fi
cd "$(dirname "$0")/.."

step "Checking that $version can be released"
for tool in git gh cargo python3; do
  command -v "$tool" >/dev/null || die "$tool is not installed"
done
[[ -z "$(git status --porcelain)" ]] || die "there are uncommitted changes: commit them first"
branch="$(git symbolic-ref --short -q HEAD)" || die "not on a branch"
gh auth status >/dev/null 2>&1 || die "gh is not signed in (gh auth login)"
git fetch -q --tags origin main
[[ -z "$(git tag --list "$tag")" ]] || die "$tag is already released"
git merge-base --is-ancestor origin/main HEAD ||
  die "$branch is behind main: merge origin/main first"

# Anything that fails before the commit leaves the files as they were.
files=(CHANGELOG.md Cargo.toml Cargo.lock README.md)
restore() { git checkout -q -- "${files[@]}"; }
trap restore EXIT

step "Setting the version and dating the changelog section"
script/set-version.sh "$version" >/dev/null
python3 - "$version" <<'PY'
import datetime, re, sys

version = sys.argv[1]
text = open("CHANGELOG.md", encoding="utf-8").read()


def die(message):
    sys.exit(f"release.sh: CHANGELOG.md: {message}")


unreleased = re.search(r"^## \[Unreleased\]\n(.*?)(?=^## \[)", text, re.M | re.S)
if not unreleased:
    die("there is no Unreleased section before a released one")
changes = unreleased.group(1).strip()
if re.search(rf"^## \[{re.escape(version)}\]", text, re.M):
    if changes:
        die(f"move what is under Unreleased to the {version} section")
else:
    if not changes:
        die("nothing is under Unreleased: describe what changed first")
    link = re.search(r"^\[Unreleased\]: (\S+/compare/)(v\S+)\.\.\.HEAD$", text, re.M)
    if not link:
        die("there is no [Unreleased] compare link")
    base, last = link.groups()
    tag = f"v{version}"
    text = text.replace(
        link.group(0),
        f"[Unreleased]: {base}{tag}...HEAD\n[{version}]: {base}{last}...{tag}",
    )
    heading = f"## [Unreleased]\n\n## [{version}] - {datetime.date.today().isoformat()}\n"
    text = text.replace("## [Unreleased]\n", heading, 1)
    open("CHANGELOG.md", "w", encoding="utf-8").write(text)
PY
[[ "$(script/release-tag.sh)" == "$tag" ]] || die "tag-release.yml would not release $tag"
changes="$(script/release-notes.sh "$version" | sed "1,/^## What's new in /d")"

if [[ -z "$skip_checks" ]]; then
  step "Running CI's checks (a few minutes)"
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked --quiet -- -D warnings
  cargo test --workspace --locked --quiet
fi

if [[ -n "$dry_run" ]]; then
  git --no-pager diff --stat -- "${files[@]}"
  echo "$changes"
  [[ -n "$review" ]] || echo "(the real run needs --docs-review <file>: review the documentation first)"
  step "$version can be released from $branch (dry run: nothing was changed or pushed)"
  exit 0
fi

if [[ "$branch" == main ]]; then
  git switch -q -c "$version"
  branch=$version
fi
if ! git diff --quiet -- "${files[@]}"; then
  step "Committing"
  git commit -q -m "Release $version" -- "${files[@]}"
fi
trap - EXIT

step "Pushing $branch"
git push -q -u origin "$branch"

body="## What and why

**Merging this PR publishes $version** (\`tag-release.yml\`).

$changes

## Documentation review

$review"
url="$(gh pr list --head "$branch" --base main --state open --json url --jq '.[0].url // ""')"
if [[ -z "$url" ]]; then
  step "Opening a pull request"
  url="$(gh pr create --base main --head "$branch" --title "Release $version" --body "$body" | tail -n 1)"
else
  step "Updating the pull request's description"
  gh pr edit "$url" --body "$body" >/dev/null
fi

step "Turning on auto-merge"
gh pr merge "$url" --auto --merge

cat <<DONE

$url
merges by itself once the required checks pass. Then GitHub tags $tag, builds the downloads,
publishes the release and updates the Homebrew cask. Nothing else needs to be run. If a required
check fails, nothing is released: push a fix to $branch.
DONE
