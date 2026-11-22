#!/usr/bin/env bash
# Edits the take recorded by record.sh into docs/media/demo.mp4 and docs/media/demo-hero.gif
# (see post.py for what it does). Usage: script/demo/post.sh [post.py options]
set -euo pipefail
exec python3 "$(dirname "$0")/post.py" "$@"
