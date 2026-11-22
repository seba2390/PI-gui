#!/usr/bin/env python3
"""Edits a recorded demo take (script/demo/record.sh) into the README media.

Reads the take's timeline (written by the app's demo mode) and OpenScreen project, then:
  1. covers macOS's "window is being captured" pill (drawn over the traffic lights) with the
     empty title-bar strip next to it,
  2. writes the edits into the OpenScreen project: fast-forward of every `wait_idle` span (with
     a badge), gentle zooms on the pickers, the context wheel and the theme menu, and a cut
     between the `cut_out` and `cut_in` markers,
  3. renders it with `openscreen export`,
  4. crossfades the cut and encodes docs/media/demo.mp4 (H.264, no audio, faststart, size-capped),
  5. with --gif, also builds a short looping docs/media/demo-hero.gif with gifski.

Usage: script/demo/post.py [--work DIR] [--out DIR] [--sync-ms N] [--gif]
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OPENSCREEN = os.environ.get("OPENSCREEN", "/Applications/Openscreen.app/Contents/MacOS/Openscreen")

FAST_SPEED = 16.0  # OpenScreen's maximum
MAX_FAST_OUTPUT_S = 14.0  # longer agent runs lose their middle
MIN_FAST_MS = int(os.environ.get("DEMO_MIN_FAST_MS", 10000))  # shorter waits play at real speed
ZOOM_DEPTH = 2  # 1.5x
ZOOM_LEAD_MS = 350  # zoom in this long before the cursor reaches the control
ZOOM_TAIL_MS = 700
ZOOM_MERGE_GAP_MS = 1500
CROSSFADE_S = 0.2
MP4_LIMIT = 10 * 1024 * 1024
GIF_LIMIT = 10 * 1024 * 1024
GIF_WIDTH = int(os.environ.get("DEMO_GIF_WIDTH", 1800))  # README shows it at 900 px: crisp on Retina
GIF_FPS = 20
BADGE = "⏩ 16×"


@dataclass
class StepRun:
    index: int
    kind: str
    name: str | None
    start: int
    end: int
    targets: list[dict] = field(default_factory=list)


@dataclass
class Timeline:
    width: float
    height: float
    scale: float
    start_epoch: int
    steps: list[StepRun]

    def markers(self, name: str) -> list[StepRun]:
        return [s for s in self.steps if s.kind == "marker" and s.name == name]

    def marker(self, name: str) -> int | None:
        found = self.markers(name)
        return found[0].start if found else None


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=True, **kwargs)


def load_timeline(path: Path) -> Timeline:
    events = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    ready = next(e for e in events if e["event"] == "ready")
    start = next(e for e in events if e["event"] == "start")
    if any(e["event"] == "error" for e in events):
        sys.exit(f"the take failed: {[e for e in events if e['event'] == 'error']}")
    starts = {e["index"]: e for e in events if e["event"] == "step" and e["phase"] == "start"}
    steps = []
    for e in events:
        if e["event"] == "step" and e["phase"] == "end":
            s = starts[e["index"]]
            steps.append(
                StepRun(e["index"], e["step"], e.get("name"), s["t_ms"], e["t_ms"], e.get("targets", []))
            )
    return Timeline(
        ready["window"]["width"], ready["window"]["height"], ready["scale"], start["epoch_ms"], steps
    )


# ---- regions (source time, ms) -------------------------------------------------------------


def union(rects: list[dict]) -> tuple[float, float, float, float]:
    x0 = min(r["x"] for r in rects)
    y0 = min(r["y"] for r in rects)
    x1 = max(r["x"] + r["w"] for r in rects)
    y1 = max(r["y"] + r["h"] for r in rects)
    return x0, y0, x1 - x0, y1 - y0


def zoom_spans(tl: Timeline) -> list[tuple[int, int, float, float]]:
    """(start, end, cx, cy) in timeline ms and window fractions."""
    spans: list[tuple[int, int, list[dict], float]] = []  # start, end, rects, extra lift (points)
    steps = tl.steps
    for i, step in enumerate(steps):
        if step.kind in ("pick_model", "thinking", "web", "theme") and step.targets:
            spans.append((step.start, step.end, step.targets, 0.0))
        elif step.kind == "open_context" and step.targets:
            close = next((s for s in steps[i + 1 :] if s.kind == "close_context"), None)
            end = close.end if close else step.end + 3000
            # The breakdown opens above and to the left of the wheel.
            ring = step.targets[0]
            popover = {"x": ring["x"] - 230, "y": ring["y"] - 300, "w": 250, "h": 300}
            spans.append((step.start, end, [ring, popover], 0.0))
    merged: list[list] = []
    for start, end, rects, _ in sorted(spans, key=lambda s: s[0]):
        if merged and start - merged[-1][1] < ZOOM_MERGE_GAP_MS:
            merged[-1][1] = max(merged[-1][1], end)
            merged[-1][2] = merged[-1][2] + rects
        else:
            merged.append([start, end, list(rects)])
    out = []
    for start, end, rects in merged:
        x, y, w, h = union(rects)
        out.append(
            (
                start - ZOOM_LEAD_MS,
                end + ZOOM_TAIL_MS,
                min(max((x + w / 2) / tl.width, 0.0), 1.0),
                min(max((y + h / 2) / tl.height, 0.0), 1.0),
            )
        )
    return out


def fast_spans(tl: Timeline) -> list[tuple[int, int]]:
    """`wait_idle` spans (minus a beat at the end, so the finish plays at real speed)."""
    return [
        (s.start, s.end - 1200) for s in tl.steps if s.kind == "wait_idle" and s.end - s.start - 1200 >= MIN_FAST_MS
    ]


@dataclass
class Edit:
    """Source-time edits, in video ms."""

    trims: list[tuple[int, int]]
    speeds: list[tuple[int, int, float]]
    zooms: list[tuple[int, int, float, float]]
    badges: list[tuple[int, int]]
    cut: int | None  # where the cut lands (source ms, the end of its trim)

    def output_s(self, source_ms: float) -> float:
        """Where a source time lands in the edited video."""
        t = 0.0
        position = 0.0
        events = sorted(
            [(a, b, None) for a, b in self.trims] + [(a, b, s) for a, b, s in self.speeds]
        )
        for a, b, speed in events:
            if source_ms <= a:
                break
            t += a - position
            inside = min(source_ms, b) - a
            if speed is not None:
                t += inside / speed
            position = min(source_ms, b)
            if source_ms <= b:
                return t / 1000
        t += source_ms - position
        return t / 1000


def build_edit(tl: Timeline, offset: int, duration_ms: int) -> Edit:
    v = lambda ms: int(ms + offset)  # noqa: E731
    trims: list[tuple[int, int]] = []
    speeds: list[tuple[int, int, float]] = []
    badges: list[tuple[int, int]] = []
    for start, end in fast_spans(tl):
        a, b = v(start), v(end)
        excess = (b - a) - MAX_FAST_OUTPUT_S * 1000 * FAST_SPEED
        if excess > 0:
            # Keep the beginning and the end of the run, drop its middle.
            keep = (b - a - excess) / 2
            middle = (int(a + keep), int(b - keep))
            speeds += [(a, middle[0], FAST_SPEED), (middle[1], b, FAST_SPEED)]
            trims.append(middle)
        else:
            speeds.append((a, b, FAST_SPEED))
        badges.append((a, b))
    cut = None
    cut_out, cut_in = tl.marker("cut_out"), tl.marker("cut_in")
    if cut_out is not None and cut_in is not None and cut_in > cut_out:
        trims.append((v(cut_out), v(cut_in)))
        cut = v(cut_in)
    # Nothing before the first step, and a short tail.
    first = v(0)
    if first > 50:
        trims.append((0, first))
    end_marker = tl.marker("end")
    if end_marker is not None and v(end_marker) + 300 < duration_ms:
        trims.append((v(end_marker) + 300, duration_ms))
    # A zoom ends before a fast-forward starts, so it eases out at real speed.
    zooms = []
    for a, b, cx, cy in zoom_spans(tl):
        a, b = v(a), v(b)
        for fast_start, _, _ in speeds:
            if a < fast_start < b:
                b = fast_start
        zooms.append((a, b, cx, cy))
    return Edit(sorted(trims), sorted(speeds), zooms, badges, cut)


def write_project(project: dict, edit: Edit, video: Path) -> dict:
    project = json.loads(json.dumps(project))
    project["media"] = {"screenVideoPath": str(video), "cursorCaptureMode": "system"}
    e = project["editor"]
    e.update(
        {
            "aspectRatio": "16:9",
            "autoZoomEnabled": False,
            "exportQuality": "good",
            "exportFormat": "mp4",
            "padding": float(os.environ.get("DEMO_PADDING", 6)),
            "zoomRegions": [
                {
                    "id": f"zoom-{i + 1}",
                    "startMs": a,
                    "endMs": b,
                    "depth": ZOOM_DEPTH,
                    "focus": {"cx": cx, "cy": cy},
                    "focusMode": "manual",
                    "source": "manual",
                }
                for i, (a, b, cx, cy) in enumerate(edit.zooms)
            ],
            "speedRegions": [
                {"id": f"speed-{i + 1}", "startMs": a, "endMs": b, "speed": s}
                for i, (a, b, s) in enumerate(edit.speeds)
            ],
            "trimRegions": [
                {"id": f"trim-{i + 1}", "startMs": a, "endMs": b} for i, (a, b) in enumerate(edit.trims)
            ],
            "annotationRegions": [
                {
                    "id": f"badge-{i + 1}",
                    "startMs": a,
                    "endMs": b,
                    "type": "text",
                    "content": BADGE,
                    "textContent": BADGE,
                    "space": "frame",
                    "position": {"x": 84.5, "y": 3.5},
                    "size": {"width": 12, "height": 7},
                    "style": {
                        "color": "#ffffff",
                        "backgroundColor": "rgba(20, 20, 20, 0.72)",
                        "fontSize": 30,
                        "fontFamily": "Inter",
                        "fontWeight": "bold",
                        "fontStyle": "normal",
                        "textDecoration": "none",
                        "textAlign": "center",
                        "textAnimation": "none",
                    },
                    "zIndex": 10,
                }
                for i, (a, b) in enumerate(edit.badges)
            ],
        }
    )
    return project


# ---- media ---------------------------------------------------------------------------------


def cover_capture_pill(raw: Path, out: Path, scale: float) -> None:
    """macOS draws a "being captured" pill over the traffic lights; cover it with the empty
    title-bar strip to its right (so it follows the theme)."""
    px = lambda points: int(round(points * scale))  # noqa: E731
    w, h = px(78), px(28)
    graph = f"[0:v]split[a][b];[b]crop={w}:{h}:{px(84)}:{px(4)}[patch];[a][patch]overlay={px(4)}:{px(4)}"
    run(
        ["ffmpeg", "-v", "error", "-y", "-i", str(raw), "-filter_complex", graph, "-an",
         "-c:v", "libx264", "-preset", "medium", "-crf", "12", "-pix_fmt", "yuv420p",
         "-fps_mode", "passthrough", str(out)]
    )


def export(project_path: Path, out: Path, quality: str = "good") -> None:
    result = subprocess.run(
        [OPENSCREEN, "export", str(project_path), "-o", str(out), "--quality", quality, "--fps", "60", "--json"],
        capture_output=True, text=True,
    )
    done = [json.loads(l) for l in result.stdout.splitlines() if l.startswith("{")]
    done = [e for e in done if e.get("event") in ("done", "error")]
    if result.returncode != 0 or not done or not done[-1].get("success"):
        sys.exit(f"openscreen export failed: {done[-1] if done else result.stderr[-2000:]}")


def duration_s(video: Path) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(video)],
        capture_output=True, text=True, check=True,
    )
    return float(out.stdout.strip())


def encode_final(edited: Path, out: Path, cut_s: float | None) -> None:
    if cut_s is not None:
        a = cut_s
        graph = (
            f"[0:v]trim=0:{a:.3f},setpts=PTS-STARTPTS[a];"
            f"[0:v]trim={a:.3f},setpts=PTS-STARTPTS[b];"
            f"[a][b]xfade=transition=fade:duration={CROSSFADE_S}:offset={a - CROSSFADE_S:.3f},format=yuv420p[v]"
        )
    else:
        graph = "[0:v]format=yuv420p[v]"
    for crf in (20, 22, 24, 26, 28, 30, 32):
        run(
            ["ffmpeg", "-v", "error", "-y", "-i", str(edited), "-filter_complex", graph, "-map", "[v]",
             "-an", "-c:v", "libx264", "-preset", "slow", "-crf", str(crf), "-profile:v", "high",
             "-r", "30", "-movflags", "+faststart", str(out)]
        )
        if out.stat().st_size <= MP4_LIMIT:
            return
    sys.exit(f"{out} is still over 10 MB at crf 32")


def gif_segments(tl: Timeline, edit: Edit, offset: int) -> list[tuple[float, float]]:
    """(start, end) pieces of the edited video (before the crossfade) for the hero GIF: the end
    of the prompt being typed and sent, the reply streaming in, the context wheel, and the
    switch to the light theme."""
    def out(ms: float) -> float:
        return edit.output_s(ms + offset)

    find = lambda kind: next((s for s in tl.steps if s.kind == kind), None)  # noqa: E731
    pieces = []
    typed, send = find("type"), find("send")
    if typed and send:
        pieces.append((out(max(typed.end - 1500, typed.start)), out(send.end) + 1.8))
    ctx, close = find("open_context"), find("close_context")
    if ctx and close:
        pieces.append((out(ctx.end) - 0.4, out(close.end) + 0.2))
    theme = next((s for s in tl.steps if s.kind == "theme"), None)
    if theme:
        pieces.append((out(theme.start) + 0.3, out(theme.end) + 1.2))
    return pieces


def make_gif(final: Path, pieces: list[tuple[float, float]], out: Path, work: Path) -> None:
    frames = work / "gif-frames"
    shutil.rmtree(frames, ignore_errors=True)
    frames.mkdir()
    # Short crossfades between the pieces, and back to the start for the loop.
    inputs, graph, labels = [], [], []
    for i, (a, b) in enumerate(pieces):
        inputs += ["-ss", f"{a:.3f}", "-t", f"{b - a:.3f}", "-i", str(final)]
        graph.append(f"[{i}:v]setpts=PTS-STARTPTS,fps={GIF_FPS},scale={GIF_WIDTH}:-2:flags=lanczos[p{i}]")
        labels.append((f"p{i}", b - a))
    current, length = labels[0]
    for i, (label, piece_len) in enumerate(labels[1:], start=1):
        graph.append(f"[{current}][{label}]xfade=transition=fade:duration=0.25:offset={length - 0.25:.3f}[x{i}]")
        current, length = f"x{i}", length + piece_len - 0.25
    run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", ";".join(graph), "-map", f"[{current}]",
         str(frames / "f%04d.png")])
    pngs = sorted(frames.glob("f*.png"))
    for quality in (90, 85, 80, 70, 60):
        run(["gifski", "--quiet", "--fps", str(GIF_FPS), "--quality", str(quality), "--width", str(GIF_WIDTH),
             "-o", str(out), *map(str, pngs)])
        if out.stat().st_size <= GIF_LIMIT:
            break
    else:
        sys.exit(f"{out} is still over 10 MB")
    print(f"gif: {len(pngs) / GIF_FPS:.1f} s, {out.stat().st_size / 1e6:.1f} MB")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work", type=Path, default=Path(os.environ.get("DEMO_WORK", "/tmp/pistachio-demo")))
    parser.add_argument("--out", type=Path, default=ROOT / "docs" / "media")
    parser.add_argument(
        "--sync-ms", type=int, default=int(os.environ.get("DEMO_SYNC_MS", 150)),
        help="added to the measured offset between the recording and the timeline",
    )
    parser.add_argument("--gif", action="store_true", help="also build the short hero GIF")
    args = parser.parse_args()
    work: Path = args.work

    tl = load_timeline(work / "timeline.jsonl")
    record = json.loads((work / "record.json").read_text())
    raw = Path(record["screenVideoPath"])
    offset = tl.start_epoch - record["recording_started_epoch_ms"] + args.sync_ms
    duration_ms = int(duration_s(raw) * 1000)
    print(f"take: {duration_ms / 1000:.1f} s, timeline offset {offset} ms")

    screen = work / "screen.mp4"
    cover_capture_pill(raw, screen, tl.scale)
    edit = build_edit(tl, offset, duration_ms)
    project = write_project(json.loads((work / "take.openscreen").read_text()), edit, screen)
    project_path = work / "edit.openscreen"
    project_path.write_text(json.dumps(project, indent=2))
    (work / "edit.json").write_text(json.dumps(edit.__dict__, indent=2))

    edited = work / "edited.mp4"
    export(project_path, edited)
    args.out.mkdir(parents=True, exist_ok=True)
    final = args.out / "demo.mp4"
    cut_s = edit.output_s(edit.cut) if edit.cut is not None else None
    encode_final(edited, final, cut_s)
    print(f"mp4: {duration_s(final):.1f} s, {final.stat().st_size / 1e6:.1f} MB")
    if args.gif:
        # From a full-resolution render of the same edit, so the GIF is sharp.
        sharp = work / "edited-source.mp4"
        export(project_path, sharp, quality="source")
        make_gif(sharp, gif_segments(tl, edit, offset), args.out / "demo-hero.gif", work)


if __name__ == "__main__":
    main()
