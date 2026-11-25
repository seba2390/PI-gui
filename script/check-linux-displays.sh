#!/usr/bin/env bash
# Checks that the app opens its window on X11 and on Wayland, on virtual displays:
#   script/check-linux-displays.sh /usr/bin/pistachio <folder for screenshots and logs>
# Needs (Ubuntu 24.04 or newer): xvfb openbox x11-utils x11-apps imagemagick sway grim
# mesa-vulkan-drivers dbus fonts-dejavu-core. Not as root (sway refuses).
set -euo pipefail

app=$1
out=$2
mkdir -p "$out"
out="$(cd "$out" && pwd)"
pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  pids=()
}
trap cleanup EXIT

die() {
  echo "check-linux-displays.sh: $*" >&2
  tail -n 40 "$out"/*.log >&2 || true
  exit 1
}

# Runs `$@` until it succeeds, for up to 60 seconds.
until_ok() {
  for _ in $(seq 60); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  return 1
}

# The app, with its own state, in the environment `$2…` as its only display.
start_app() {
  local name=$1
  shift
  local state
  state="$(mktemp -d)"
  env -u DISPLAY -u WAYLAND_DISPLAY PI_CODING_AGENT_DIR="$state" "$@" "$app" \
    >"$out/$name.log" 2>&1 &
  app_pid=$!
  pids+=("$app_pid")
}

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR_FOR_CHECK:-$(mktemp -d)}"
chmod 700 "$XDG_RUNTIME_DIR"

echo "==> X11"
Xvfb :97 -screen 0 1440x900x24 >"$out/xvfb.log" 2>&1 &
pids+=($!)
until_ok xdpyinfo -display :97 || die "Xvfb did not start"
DISPLAY=:97 openbox >"$out/openbox.log" 2>&1 &
pids+=($!)
start_app x11 DISPLAY=:97
x11_window() { xwininfo -display :97 -root -tree | grep -q '"Pistachio": ("pistachio" "pistachio")'; }
until_ok x11_window || die "no window on X11"
sleep 5
kill -0 "$app_pid" 2>/dev/null || die "the app ended on X11"
xwd -silent -display :97 -root | convert xwd:- "$out/x11.png"
cleanup

echo "==> Wayland"
echo 'output HEADLESS-1 resolution 1440x900' >"$out/sway.config"
echo 'xwayland disable' >>"$out/sway.config"
env -u DISPLAY -u WAYLAND_DISPLAY WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
  WLR_RENDERER=pixman SWAYSOCK="$XDG_RUNTIME_DIR/sway.sock" \
  sway --unsupported-gpu -c "$out/sway.config" >"$out/sway.log" 2>&1 &
pids+=($!)
export SWAYSOCK="$XDG_RUNTIME_DIR/sway.sock"
until_ok swaymsg -t get_version || die "sway did not start"
wayland="$(basename "$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' | head -n 1)")"
start_app wayland WAYLAND_DISPLAY="$wayland"
wayland_window() { swaymsg -t get_tree | grep -q '"app_id": "pistachio"'; }
until_ok wayland_window || die "no window on Wayland"
sleep 5
kill -0 "$app_pid" 2>/dev/null || die "the app ended on Wayland"
WAYLAND_DISPLAY="$wayland" grim "$out/wayland.png"
cleanup

# A window that stays empty is one color inside its frame.
for name in x11 wayland; do
  colors="$(convert "$out/$name.png" -gravity center -crop 1000x700+0+0 +repage -format '%k' info:)"
  echo "$name.png has $colors colors"
  [[ "$colors" -gt 50 ]] || die "nothing is drawn on $name"
done
echo "Opens on X11 and Wayland."
