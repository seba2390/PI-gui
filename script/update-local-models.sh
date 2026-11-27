#!/usr/bin/env bash
# Copies the catalog of local models and engines (crates/local-models/catalog.json) from a
# local-llm-radar checkout, where `scripts/resolve.py` pinned every download and
# `scripts/export_catalog.py` wrote it. Review the diff like code: it decides what Pistachio
# downloads and runs. Then look over the suggestions and run a live test:
#
#   cargo test -p local-models typical_computers -- --ignored --nocapture
#   LOCAL_MODELS_LIVE=lfm3-1.7b/gguf-q4_k_m/llama-cpp cargo test -p local-models --test live -- --ignored --nocapture
set -euo pipefail
cd "$(dirname "$0")/.."
radar=${1:-../local-llm-radar}
source="$radar/catalog/catalog.json"
[[ -f $source ]] || { echo "no $source: pass the local-llm-radar checkout" >&2; exit 1; }
python=python3; [[ -x $radar/.venv/bin/python ]] && python=$radar/.venv/bin/python
(cd "$radar" && "$python" scripts/export_catalog.py --check)
cp "$source" crates/local-models/catalog.json
cargo test -q -p local-models catalog
git diff --stat -- crates/local-models/catalog.json
