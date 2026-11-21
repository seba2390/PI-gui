#!/usr/bin/env bash
# Reports the resident memory of a running PI-gui and its pi children.
# Usage: script/measure-memory.sh [pid]   (default: newest pi-gui process)
set -euo pipefail
pid="${1:-$(pgrep -n -f 'pi-gui$|MacOS/pi-gui' || true)}"
[[ -n "$pid" ]] || { echo "pi-gui is not running" >&2; exit 1; }

rss_kb() { ps -o rss= -p "$1" | tr -d ' '; }
mb() { awk -v kb="$1" 'BEGIN { printf "%.1f MB", kb / 1024 }'; }

echo "pi-gui (pid $pid): $(mb "$(rss_kb "$pid")") RSS"
if [[ "$(uname)" == "Darwin" ]] && command -v footprint >/dev/null; then
  # Physical footprint is what Activity Monitor shows as "Memory".
  footprint "$pid" 2>/dev/null | awk '
    /Footprint:/ { for (i = 1; i <= NF; i++) if ($i == "Footprint:") print "  physical footprint: " $(i+1) " " $(i+2) }
    /IOSurface|MALLOC_SMALL|MALLOC_LARGE|graphics/ { printf "    %-10s %s\n", $1 " " $2, substr($0, index($0, $5)) }'
fi
total=0
for child in $(pgrep -P "$pid" || true); do
  kb="$(rss_kb "$child")"; total=$((total + kb))
  echo "  child $child ($(ps -o comm= -p "$child" | xargs basename)): $(mb "$kb")"
done
[[ $total -gt 0 ]] && echo "  children total: $(mb "$total")"
