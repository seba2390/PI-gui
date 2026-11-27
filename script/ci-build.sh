#!/usr/bin/env bash
# CI builds with a record of what happened, so a slow, stalled or starved build can be told apart
# afterwards (release.yml, release-cache.yml).
#
#   script/ci-build.sh --release --locked [--target …]   # cargo build --timings, while monitoring
#   script/ci-build.sh --report                          # summary; run it with `if: always()`
#
# The build prints the machine's memory (and swap, or Windows' commit charge), free disk, CPU and
# the largest processes every $CI_MONITOR_SECONDS (30) between cargo's lines, so they are in the
# job's log as it runs. Everything goes to target/ci-logs (build.log, resources.log, the cargo
# timings report), which the workflow uploads even when the build fails or times out. --report
# writes a summary to $GITHUB_STEP_SUMMARY: the result, how many crates were compiled (all of them
# means the cache was not used), peak memory, low disk and the end of the log after a failure.
set -uo pipefail

logs=target/ci-logs
mkdir -p "$logs"

os() {
  case "$(uname -s)" in
    Linux) echo linux ;;
    Darwin) echo macos ;;
    MINGW* | MSYS* | CYGWIN*) echo windows ;;
    *) echo other ;;
  esac
}

# One line: `mem_used=… mem_total=… swap_used=… disk_free=… cpu=… top=…` (sizes in MB).
sample() {
  local mem_used="" mem_total="" swap="" disk="" cpu="" top=""
  case "$(os)" in
    linux)
      read -r mem_total mem_used swap < <(awk '
        /^MemTotal:/ {t=$2} /^MemAvailable:/ {a=$2} /^SwapTotal:/ {st=$2} /^SwapFree:/ {sf=$2}
        END {printf "%d %d %d\n", t/1024, (t-a)/1024, (st-sf)/1024}' /proc/meminfo)
      cpu="load $(cut -d' ' -f1 /proc/loadavg)"
      top=$(ps -eo rss=,comm= --sort=-rss | head -3 | awk '{printf "%s%s:%dMB", (NR>1?",":""), $2, $1/1024}')
      ;;
    macos)
      mem_total=$(($(sysctl -n hw.memsize) / 1048576))
      mem_used=$(vm_stat | awk '
        /page size of/ {p=$8}
        /Pages active/ {a=$3} /Pages wired down/ {w=$4} /occupied by compressor/ {c=$5}
        END {printf "%d", (a+w+c)*p/1048576}')
      swap=$(sysctl -n vm.swapusage | awk '{for (i=1;i<=NF;i++) if ($i=="used") {v=$(i+2); sub(/M$/,"",v); printf "%d", v}}')
      cpu="load $(sysctl -n vm.loadavg | awk '{print $2}')"
      top=$(ps -axo rss=,comm= | sort -rn | head -3 | awk '{n=$2; sub(".*/","",n); printf "%s%s:%dMB", (NR>1?",":""), n, $1/1024}')
      ;;
    windows)
      # Windows fails allocations when the commit charge (RAM + page file) runs out, so that is
      # the "swap" column: committed MB out of the limit.
      # shellcheck disable=SC2016 # PowerShell's own variables
      read -r mem_total mem_used swap cpu top < <(powershell -NoProfile -NonInteractive -Command '
        $o = Get-CimInstance Win32_OperatingSystem
        $c = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average
        $t = (Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 3 |
              ForEach-Object { "{0}:{1}MB" -f $_.ProcessName, [int]($_.WorkingSet64 / 1MB) }) -join ","
        "{0} {1} {2}/{3} {4}% {5}" -f [int]($o.TotalVisibleMemorySize / 1KB),
          [int](($o.TotalVisibleMemorySize - $o.FreePhysicalMemory) / 1KB),
          [int](($o.TotalVirtualMemorySize - $o.FreeVirtualMemory) / 1KB),
          [int]($o.TotalVirtualMemorySize / 1KB), [int]$c, $t' 2>/dev/null | tr -d '\r')
      ;;
  esac
  disk=$(df -Pk . 2>/dev/null | awk 'NR==2 {printf "%d", $4/1024}')
  echo "mem_used=${mem_used:-?} mem_total=${mem_total:-?} swap_used=${swap:-?} disk_free=${disk:-?} cpu=${cpu:-?} top=${top:-?}"
}

monitor() {
  local interval=${CI_MONITOR_SECONDS:-30}
  while :; do
    echo "[monitor $(date -u +%H:%M:%S)] $(sample)" | tee -a "$logs/resources.log"
    sleep "$interval"
  done
}

build() {
  date -u +%s > "$logs/started"
  echo "cargo build --timings $*" > "$logs/command"
  : > "$logs/resources.log"
  monitor &
  local watcher=$!
  cargo build --timings "$@" 2>&1 | tee "$logs/build.log"
  local status=${PIPESTATUS[0]}
  kill "$watcher" 2>/dev/null
  wait "$watcher" 2>/dev/null
  echo "[monitor $(date -u +%H:%M:%S)] $(sample)" >> "$logs/resources.log"
  echo "$status" > "$logs/status"
  date -u +%s > "$logs/finished"
  return "$status"
}

minutes() { printf '%dm %02ds' $(($1 / 60)) $(($1 % 60)); }

# The build log without color codes (cargo's output is colored in CI).
plain_log() { sed $'s/\x1b\\[[0-9;]*[A-Za-z]//g' "$logs/build.log" 2>/dev/null; }

report() {
  cp target/cargo-timings/cargo-timing.html "$logs/" 2>/dev/null
  local summary=${GITHUB_STEP_SUMMARY:-/dev/stdout}
  local status result started now duration compiled total cache peak low_disk last
  status=$(cat "$logs/status" 2>/dev/null)
  case "$status" in
    0) result="✅ built" ;;
    "") result="⏱️ stopped before it finished (time limit, cancelled, or the runner gave out)" ;;
    *) result="❌ failed (exit $status)" ;;
  esac
  started=$(cat "$logs/started" 2>/dev/null || date -u +%s)
  now=$(cat "$logs/finished" 2>/dev/null || date -u +%s)
  duration=$((now - started))
  compiled=$(plain_log | grep -c '^ *Compiling ')
  total=$(grep -c '^name = ' Cargo.lock 2>/dev/null)
  case "${CACHE_HIT:-}" in
    true) cache="restored for this exact Cargo.lock" ;;
    false) cache="no exact match: an older one may have been restored (the rust-cache step says which key)" ;;
    *) cache="see the rust-cache step" ;;
  esac
  peak=$(awk '{for (i=1;i<=NF;i++) if ($i ~ /^mem_used=[0-9]/) {split($i,a,"="); if (a[2]>m) m=a[2]} if (!t) for (i=1;i<=NF;i++) if ($i ~ /^mem_total=[0-9]/) {split($i,b,"="); t=b[2]}} END {if (m) printf "%d of %d MB", m, t}' "$logs/resources.log" 2>/dev/null)
  low_disk=$(awk '{for (i=1;i<=NF;i++) if ($i ~ /^disk_free=[0-9]/) {split($i,a,"="); if (min=="" || a[2]<min) min=a[2]}} END {if (min!="") printf "%d MB", min}' "$logs/resources.log" 2>/dev/null)
  last=$(tail -n 1 "$logs/resources.log" 2>/dev/null)
  {
    echo "### Build on $(os): $result"
    echo
    echo "| | |"
    echo "|---|---|"
    echo "| Command | \`$(cat "$logs/command" 2>/dev/null)\` |"
    echo "| Time | $(minutes "$duration") |"
    echo "| Crates compiled | ${compiled:-0} (Cargo.lock has ${total:-?} packages; each platform builds some of them) |"
    echo "| Cache | $cache |"
    echo "| Peak memory in use | ${peak:-?} |"
    echo "| Least free disk | ${low_disk:-?} |"
    echo "| Last sample | \`${last:-none}\` |"
    echo
    if ((${compiled:-0} > 200)); then
      echo "> [!WARNING]"
      echo "> Most dependencies were compiled: the cache had none of them (see the rust-cache step)."
      echo
    fi
    if [[ "$status" != 0 ]]; then
      echo "<details><summary>End of the build log</summary>"
      echo
      echo '```'
      plain_log | tail -n 40
      echo '```'
      echo "</details>"
      echo
    fi
    echo "The full log, the resource samples and cargo's timings report (cargo-timing.html: how long"
    echo "each crate took, and what ran in parallel) are in this job's build-log artifact."
  } >> "$summary"
}

if [[ "${1:-}" == --report ]]; then
  report
else
  build "$@"
fi
