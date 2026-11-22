#!/usr/bin/env bash
# Records one take of a storyboard with OpenScreen: launches the debug app bundle in demo mode
# (isolated agent dir from setup.sh), records its window, and stops when the storyboard ends.
# Only the app process started here is killed, never an installed Pistachio.
# Usage: script/demo/record.sh [storyboard.toml]   (default: script/demo/readme-demo.toml)
# Output in $DEMO_WORK (default /tmp/pistachio-demo): take.openscreen, timeline.jsonl,
# record.json (OpenScreen's result plus the clock offset between the video and the timeline).
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
work="${DEMO_WORK:-/tmp/pistachio-demo}"
project="${DEMO_PROJECT:-/tmp/demo/snake}"
storyboard="$(cd "$(dirname "${1:-$root/script/demo/readme-demo.toml}")" && pwd)/$(basename "${1:-readme-demo.toml}")"
app="$root/target/debug/Pistachio.app/Contents/MacOS/pistachio"
openscreen="${OPENSCREEN:-/Applications/Openscreen.app/Contents/MacOS/Openscreen}"
title="$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb")).get("window_title","Pistachio Demo"))' "$storyboard")"

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

[[ -d "$work/agent" ]] || die "run script/demo/setup.sh first"
[[ -x "$app" ]] || die "no app bundle; run script/demo/setup.sh first"
[[ -x "$openscreen" ]] || die "OpenScreen not found at $openscreen"

timeline="$work/timeline.jsonl"
go="$work/go"
events="$work/record.ndjson"
rm -f "$timeline" "$go" "$events" "$work/take.openscreen" "$work/record.json"

app_pid=""
rec_pid=""
cleanup() {
    [[ -n "$rec_pid" ]] && kill -INT "$rec_pid" 2>/dev/null || true
    # Only the app this script started (and its pi children).
    if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
        children="$(pgrep -P "$app_pid" || true)"
        kill "$app_pid" 2>/dev/null || true
        [[ -n "$children" ]] && kill $children 2>/dev/null || true
    fi
}
trap cleanup EXIT

say "Launching the demo app ($storyboard)"
(
    cd "$project"
    exec env PI_CODING_AGENT_DIR="$work/agent" \
        PISTACHIO_DEMO="$storyboard" \
        PISTACHIO_DEMO_TIMELINE="$timeline" \
        PISTACHIO_DEMO_START_FILE="$go" \
        RUST_LOG="warn,pistachio=info" \
        "$app"
) >"$work/app.log" 2>&1 &
app_pid=$!

wait_for() { # file pattern timeout_s
    local deadline=$((SECONDS + $3))
    until grep -q "$2" "$1" 2>/dev/null; do
        ((SECONDS < deadline)) || return 1
        kill -0 "$app_pid" 2>/dev/null || die "the app exited (see $work/app.log)"
        sleep 0.1
    done
}
wait_for "$timeline" '"event":"ready"' 60 || die "the app did not get ready (see $work/app.log)"

say "Recording the window \"$title\""
"$openscreen" record --window "$title" --project "$work/take.openscreen" --json \
    >"$events" 2>"$work/record.err" </dev/null &
rec_pid=$!
deadline=$((SECONDS + 60))
until grep -q '"Recording started"\|"recording"' "$events" 2>/dev/null; do
    grep -q '"event":"error"' "$events" 2>/dev/null && die "OpenScreen: $(grep '"event":"error"' "$events")"
    kill -0 "$rec_pid" 2>/dev/null || die "OpenScreen exited (see $work/record.err)"
    ((SECONDS < deadline)) || die "OpenScreen did not start recording"
    sleep 0.05
done
recording_started=$(now_ms)
sleep 1
touch "$go"

say "Playing the storyboard"
deadline=$((SECONDS + ${DEMO_TIMEOUT:-2400}))
until grep -q '"event":"done"\|"event":"error"' "$timeline" 2>/dev/null; do
    ((SECONDS < deadline)) || die "the storyboard did not finish"
    kill -0 "$app_pid" 2>/dev/null || die "the app exited (see $work/app.log)"
    sleep 0.5
done
sleep 1.5
kill -INT "$rec_pid"
wait "$rec_pid" || true
rec_pid=""
if grep -q '"event":"error"' "$timeline"; then
    die "the storyboard failed: $(grep '"event":"error"' "$timeline")"
fi

python3 - "$events" "$work/record.json" "$recording_started" <<'PY'
import json, sys
events, out, started = sys.argv[1], sys.argv[2], int(sys.argv[3])
done = [e for e in map(json.loads, open(events)) if e.get("event") == "done"]
if not done or not done[-1].get("success"):
    sys.exit(f"OpenScreen failed: {done[-1] if done else 'no result'}")
result = done[-1]
result.pop("projectData", None)
result["recording_started_epoch_ms"] = started
json.dump(result, open(out, "w"), indent=2)
print(f"video: {result.get('screenVideoPath')} ({result.get('durationMs', 0) / 1000:.1f} s)")
PY
say "Recorded: $work/take.openscreen"
