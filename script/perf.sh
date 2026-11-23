#!/usr/bin/env bash
# Measures Pistachio's response times: startup, navigation (clicks, hovers, scrolling, mode
# switches, panels, settings, deleting a session) and quitting.
#
# Usage: script/perf.sh [release|debug] [runs] [baseline.json]
#   Builds the app, then runs the PISTACHIO_BENCH navigation script `runs` times (default 3)
#   against generated sessions in a private pi folder (your own sessions are never touched)
#   and prints the median of each measurement. Pass the summary.json of an earlier run to
#   compare against it.
#
# The app window opens and takes focus while it runs (about 30 s per run): keystrokes typed
# meanwhile land in it. Each run moves one generated session to the Trash (the delete step).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
profile="${1:-release}"
runs="${2:-3}"
baseline="${3:-}"

cd "$root"
if [[ "$profile" == "release" ]]; then
  cargo build --release -p pistachio
else
  cargo build -p pistachio
fi
bin="$root/target/$profile/pistachio"

work="$(mktemp -d "${TMPDIR:-/tmp}/pistachio-perf.XXXXXX")"
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }

for run in $(seq 1 "$runs"); do
  rm -rf "$work/agent" "$work/projects"
  python3 "$root/script/perf/make-sessions.py" "$work/agent" "$work/projects"
  for project in "$work/projects"/*; do
    git -C "$project" init -q && git -C "$project" add -A && \
      git -C "$project" -c user.name=bench -c user.email=bench@localhost commit -qm init
    echo "// changed" >>"$project/src/file_0.rs"
  done
  out="$work/run$run.jsonl"
  launch="$(now_ms)"
  (cd "$work/projects" && PI_CODING_AGENT_DIR="$work/agent" PISTACHIO_PERF="$out" PISTACHIO_BENCH=1 \
    "$bin" >"$work/run$run.log" 2>&1) &
  pid=$!
  ( sleep 180; kill "$pid" 2>/dev/null ) & watchdog=$!
  wait "$pid" || echo "run $run: the app exited with an error (see $work/run$run.log)" >&2
  exit_ms="$(now_ms)"
  kill "$watchdog" 2>/dev/null || true
  echo "{\"kind\":\"process\",\"launch_epoch_ms\":$launch,\"exit_epoch_ms\":$exit_ms}" >>"$out"
  echo "run $run/$runs done"
done

python3 "$root/script/perf/report.py" "$work/summary.json" "$baseline" "$work"/run*.jsonl
echo "Raw data: $work"
