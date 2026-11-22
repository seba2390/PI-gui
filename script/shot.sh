#!/usr/bin/env bash
# Dev helper: relaunch the bundled debug app in a folder with panels open, then screenshot.
# Usage: script/shot.sh <cwd> <out.png> [panels, e.g. terminal,diff] [wait-seconds]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cwd="$1"; out="$2"; panels="${3:-}"; wait="${4:-3}"
pkill -f "Pistachio.app/Contents/MacOS/pistachio" 2>/dev/null || true
sleep 0.5
(cd "$cwd" && PISTACHIO_OPEN="$panels" nohup "$root/target/debug/Pistachio.app/Contents/MacOS/pistachio" >"$out.log" 2>&1 &)
sleep "$wait"
screencapture -x "$out"
echo "$out"
