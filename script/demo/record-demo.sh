#!/usr/bin/env bash
# Re-records the README demo in one go: clean setup, recording, editing.
# Leave the mouse and keyboard alone while it records (about 5–15 minutes, depending on the
# agent). Usage: script/demo/record-demo.sh [storyboard.toml]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
"$here/setup.sh"
"$here/record.sh" "${1:-$here/readme-demo.toml}"
"$here/post.sh"
