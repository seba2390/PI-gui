#!/usr/bin/env bash
# Prepares a clean, isolated environment for recording the README demo:
#   - a fresh pi agent dir (no history, default settings) with your models.json,
#   - the demo project /tmp/demo/snake (a git repo with one commit),
#   - the Spark model the demo uses (inkling-2-mini; never DeepSeek),
#   - a fresh debug build bundled as an app.
# Usage: script/demo/setup.sh            (env: DEMO_WORK, DEMO_PROJECT, DEMO_MODEL)
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
work="${DEMO_WORK:-/tmp/pistachio-demo}"
project="${DEMO_PROJECT:-/tmp/demo/snake}"
model="${DEMO_MODEL:-inkling-2-mini}"
agent="$work/agent"

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

say "Isolated agent dir: $agent"
rm -rf "$work"
mkdir -p "$agent"
cp "$HOME/.pi/agent/models.json" "$agent/models.json"
# pi starts on the demo model, so nothing is ever sent to another one.
provider="$(python3 - "$agent/models.json" "$model" <<'PY'
import json, sys
models, wanted = json.load(open(sys.argv[1])), sys.argv[2]
for name, provider in models.get("providers", {}).items():
    if any(m.get("id") == wanted for m in provider.get("models", [])):
        print(name)
        break
PY
)"
[[ -n "$provider" ]] || die "$model is not in ~/.pi/agent/models.json"
cat > "$agent/settings.json" <<JSON
{
  "defaultProvider": "$provider",
  "defaultModel": "$model",
  "defaultThinkingLevel": "medium"
}
JSON

# The app starts in Chat mode (so the switch to Code shows) and in the dark theme.
mkdir -p "$agent/pistachio"
cat > "$agent/pistachio/settings.json" <<'JSON'
{
  "theme": "dark",
  "start_mode": "chat",
  "check_for_updates": false
}
JSON

say "Demo project: $project"
rm -rf "$project"
mkdir -p "$project"
git_demo() { git -C "$project" -c user.name=Demo -c user.email=demo@example.com "$@"; }
git_demo init -q -b main
git_demo commit -q --allow-empty -m "Initial commit"
echo "# Snake" > "$project/README.md"
echo "/target" > "$project/.gitignore"
git_demo add README.md .gitignore
git_demo commit -q -m "Add README"

say "Spark model: $model"
status="$(zsh -ic sparkstatus 2>/dev/null)" || die "sparkstatus failed (is it defined in ~/.zshrc?)"
running() { grep -A1 -i "^$1" <<<"$status" | grep -q "✔"; }
if running "DeepSeek"; then die "DeepSeek is running; stop it first (the demo never uses it)"; fi
if running "Qwen"; then die "Qwen is running; only one Spark model may run. Stop it first"; fi
if ! running "Inkling"; then
    say "Starting inkling (sparkstart inkling)…"
    zsh -ic "sparkstart inkling" || die "sparkstart inkling failed"
fi
base_url="$(python3 - "$agent/models.json" "$model" <<'PY'
import json, sys
models, wanted = json.load(open(sys.argv[1])), sys.argv[2]
for provider in models["providers"].values():
    for m in provider.get("models", []):
        if m.get("id") == wanted:
            print(m.get("baseUrl") or provider.get("baseUrl"))
PY
)"
answers() { curl -fsS -m 3 "$base_url/models" 2>/dev/null | grep -q "$model"; }
for _ in $(seq 1 120); do
    answers && break
    sleep 5
done
answers || die "$model does not answer at $base_url"

say "Building"
cd "$root"
cargo build
script/bundle-macos.sh debug >/dev/null
say "Ready: $root/target/debug/Pistachio.app"
