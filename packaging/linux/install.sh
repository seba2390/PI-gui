#!/usr/bin/env sh
# Installs PI-gui for the current user (no root needed):
#   ~/.local/bin/pi-gui, its icon, and a launcher entry.
# Run from the unpacked release folder:  ./install.sh      Remove with:  ./install.sh --uninstall
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
icon="$data/icons/hicolor/512x512/apps/pi-gui.png"
desktop="$data/applications/pi-gui.desktop"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin/pi-gui" "$icon" "$desktop"
    echo "PI-gui removed."
    exit 0
fi

mkdir -p "$bin" "$(dirname "$icon")" "$(dirname "$desktop")"
install -m 755 "$here/pi-gui" "$bin/pi-gui"
install -m 644 "$here/pi-gui.png" "$icon"
sed "s|^Exec=.*|Exec=$bin/pi-gui|" "$here/pi-gui.desktop" > "$desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$data/applications" || true

echo "PI-gui installed to $bin/pi-gui (find \"PI\" in your app launcher)."
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Note: $bin is not on your PATH; add it to run pi-gui from a terminal." ;;
esac
command -v pi >/dev/null 2>&1 || echo "Note: pi was not found; install it with: npm i -g @earendil-works/pi-coding-agent"
