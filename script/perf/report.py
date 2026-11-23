#!/usr/bin/env python3
"""Summarises PISTACHIO_PERF files from script/perf.sh.

Usage: report.py <summary.json to write> <baseline summary.json or ""> <run.jsonl>...
"""

import json
import statistics
import sys
from collections import defaultdict

out_path, baseline_path, files = sys.argv[1], sys.argv[2], sys.argv[3:]


def median(values):
    return statistics.median(values) if values else None


def p95(values):
    values = sorted(values)
    return values[round((len(values) - 1) * 0.95)] if values else None


startup = defaultdict(list)  # name -> [ms per run]
interactions = defaultdict(list)  # name -> [ms, ...] over all runs
renders = defaultdict(lambda: defaultdict(list))  # view -> stat -> [per run]
errors = []
for path in files:
    lines = [json.loads(line) for line in open(path) if line.strip()]
    process = next((l for l in lines if l["kind"] == "process"), None)
    launch = process["launch_epoch_ms"] if process else None
    quit_at = None
    last_renders = {}
    for line in lines:
        kind = line["kind"]
        if kind == "phase":
            name = line["name"]
            if name.startswith("bench_error"):
                errors.append(f"{path}: {name}")
            if name == "quit":
                quit_at = line["epoch_ms"]
            elif launch is not None:
                startup[f"launch → {name}"].append(line["epoch_ms"] - launch)
        elif kind == "interaction":
            if line["name"] == "startup: first frame" and launch is not None:
                startup["launch → first frame on screen"].append(line["epoch_ms"] - launch)
            else:
                interactions[line["name"]].append(line["ms"])
        elif kind == "renders":
            last_renders[line["view"]] = line
    for view, line in last_renders.items():
        for stat in ("count", "p50_ms", "p95_ms", "max_ms", "total_ms"):
            renders[view][stat].append(line[stat])
    if process and quit_at is not None:
        startup["quit → process exited"].append(process["exit_epoch_ms"] - quit_at)

summary = {
    "startup": {name: median(values) for name, values in startup.items()},
    "interactions": {
        name: {"median": median(values), "p95": p95(values), "max": max(values), "n": len(values)}
        for name, values in interactions.items()
    },
    "renders": {view: {stat: median(values) for stat, values in stats.items()} for view, stats in renders.items()},
}
with open(out_path, "w") as handle:
    json.dump(summary, handle, indent=2)

baseline = json.load(open(baseline_path)) if baseline_path else None


def delta(section, name, key, value):
    if not baseline or value is None:
        return ""
    old = baseline.get(section, {}).get(name)
    if isinstance(old, dict):
        old = old.get(key)
    if old is None:
        return ""
    return f"  (was {old:.1f}, {value - old:+.1f})"


print(f"\n== Startup and quit (median of {len(files)} runs, ms) ==")
for name, value in summary["startup"].items():
    print(f"  {name:<40} {value:8.1f}{delta('startup', name, None, value)}")
print("\n== Interactions: input → frame presented (ms) ==")
print(f"  {'':<34} {'median':>8} {'p95':>8} {'max':>8} {'n':>5}")
for name, stats in sorted(summary["interactions"].items()):
    print(f"  {name:<34} {stats['median']:8.1f} {stats['p95']:8.1f} {stats['max']:8.1f} {stats['n']:5}"
          f"{delta('interactions', name, 'median', stats['median'])}")
print("\n== Render time per view during the script (ms, element building only) ==")
print(f"  {'':<14} {'count':>6} {'p50':>7} {'p95':>7} {'max':>7} {'total':>8}")
for view, stats in sorted(summary["renders"].items()):
    print(f"  {view:<14} {stats['count']:6.0f} {stats['p50_ms']:7.2f} {stats['p95_ms']:7.2f} "
          f"{stats['max_ms']:7.2f} {stats['total_ms']:8.1f}{delta('renders', view, 'p95_ms', stats['p95_ms'])}")
for error in errors:
    print("ERROR:", error)
print(f"\nSummary: {out_path}")
