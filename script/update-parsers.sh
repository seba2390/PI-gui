#!/usr/bin/env bash
# Moves the pins of the document parsers that Pistachio downloads on request (crates/documents):
# the packages of each to the newest versions, locked for every platform that has wheels of all of
# them, and the models they need to the newest commits. Needs uv and curl. Then run the live tests
# (they download the parsers, and read a PDF with each):
#
#   cargo test -p documents --test live -- --ignored --test-threads 1
#
# uv and Python themselves are pinned in crates/python-env (script/update-searxng.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
python=$(sed -n 's/^pub const PYTHON_VERSION: &str = "\(.*\)";/\1/p' crates/python-env/src/catalog.rs)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cat >"$work/update.py" <<'PY'
import hashlib, json, os, subprocess, sys, tomllib

WORK, PYTHON = sys.argv[1], sys.argv[2].rsplit(".", 1)[0]
OUT = "crates/documents/src/locks"
TARGETS = {
    "aarch64-apple-darwin": "aarch64-apple-darwin",
    "x86_64-apple-darwin": "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu": "x86_64-manylinux_2_28",
    "aarch64-unknown-linux-gnu": "aarch64-manylinux_2_28",
}
# The oldest macOS the wheels must run on: `catalog::OLDEST_MACOS`.
MACOS = "14.0"

# `drop`: packages it asks for and does not need for what it does here (its web page and server,
# other model backends, Chinese full-text search).
PARSERS = {
    "pymupdf4llm": {
        "package": "pymupdf",
        "requirement": "pymupdf>=2",
        # The last version with wheels for Intel Macs.
        "older": {"x86_64-apple-darwin": "pymupdf4llm<2"},
        "drop": [],
    },
    "docling": {
        "package": "docling-slim",
        "requirement": "docling-slim[format-pdf,format-office,convert-core,models-local,feat-ocr-rapidocr-onnx]",
        "drop": [],
    },
    "mineru": {
        "package": "mineru",
        "requirement": "mineru>=4.2",
        "drop": ["accelerate", "aiosqlite", "fastapi-offline", "gradio", "jieba", "mineru-llama-cpp",
                 "modelscope", "onnxruntime-ep-webgpu", "safetensors", "torch", "torchvision",
                 "transformers", "watchfiles"],
    },
}
# repository, revision, the files it needs (a path or a folder), and where they go.
MODELS = {
    "docling": [
        ("docling-project/docling-layout-heron", "main",
         ["config.json", "preprocessor_config.json", "model.safetensors"],
         "docling-project--docling-layout-heron"),
        # As `TableStructureModel.download_models` of Docling asks for.
        ("docling-project/docling-models", "v2.3.0", ["model_artifacts/tableformer/accurate"],
         "docling-project--docling-models"),
    ],
    "mineru": [
        # As `MINERU_4_MODELS_ONNX` in MinerU's `model/registry.py` asks for.
        ("opendatalab/MinerU-4_models_onnx", "main", ["Layout", "MFR", "OCR", "Table"],
         "MinerU-4_models_onnx"),
    ],
}


def curl(url):
    return subprocess.check_output(["curl", "-fsSL", "--retry", "3", url])


def lock(name, parser, target, platform):
    source, override = f"{WORK}/{name}.in", f"{WORK}/{name}.override"
    with open(source, "w") as f:
        f.write(parser.get("older", {}).get(target, parser["requirement"]) + "\n")
    with open(override, "w") as f:
        f.writelines(f"{package} ; python_version < '3'\n" for package in parser["drop"])
    out = f"{WORK}/pylock.toml"
    result = subprocess.run(
        ["uv", "pip", "compile", source, "--override", override, "--no-config", "--python-platform",
         platform, "--python-version", PYTHON, "--only-binary", ":all:", "--no-header", "--quiet",
         "--format", "pylock.toml", "-o", out],
        env=dict(os.environ, MACOSX_DEPLOYMENT_TARGET=MACOS), capture_output=True, text=True)
    if result.returncode:
        reason = result.stderr.strip().splitlines()
        print(f"{name}: not for {target}: {reason[1].strip() if len(reason) > 1 else reason}")
        return None
    with open(out, "rb") as f:
        packages = tomllib.load(f)["packages"]
    os.remove(out)
    size, lines, version = 0, [], None
    for package in packages:
        if package["name"] == parser["package"]:
            version = package["version"]
        wheels = package["wheels"]
        size += max(wheel["size"] for wheel in wheels)
        hashes = " ".join("--hash=sha256:" + wheel["hashes"]["sha256"] for wheel in wheels)
        lines.append(f"{package['name']}=={package['version']} {hashes}\n")
    return version, f"# version {version}\n# download {size}\n" + "".join(lines)


def models(name):
    lines = []
    for repo, revision, wanted, folder in MODELS[name]:
        info = json.loads(curl(f"https://huggingface.co/api/models/{repo}/revision/{revision}"))
        commit = info["sha"]
        tree = json.loads(curl(f"https://huggingface.co/api/models/{repo}/tree/{commit}?recursive=true"))
        files = [f for f in tree if f["type"] == "file"
                 and any(f["path"] == path or f["path"].startswith(path + "/") for path in wanted)]
        assert files, repo
        for file in sorted(files, key=lambda f: f["path"]):
            url = f"https://huggingface.co/{repo}/resolve/{commit}/{file['path']}"
            sha256 = file["lfs"]["oid"] if "lfs" in file else hashlib.sha256(curl(url)).hexdigest()
            lines.append(f"{sha256} {file['size']} {url} {folder}/{file['path']}\n")
    with open(f"{OUT}/{name}-models.txt", "w") as f:
        f.writelines(lines)


os.makedirs(OUT, exist_ok=True)
for name, parser in PARSERS.items():
    versions = set()
    for target, platform in TARGETS.items():
        path = f"{OUT}/{name}-{target}.txt"
        locked = lock(name, parser, target, platform)
        if locked is None:
            if os.path.exists(path):
                os.remove(path)
            continue
        version, text = locked
        if target not in parser.get("older", {}):
            versions.add(version)
        with open(path, "w") as f:
            f.write(text)
    # An older version may be all that resolves for one of them.
    assert len(versions) == 1, (name, versions)
    print(f"{name} {versions.pop()}")
    if name in MODELS:
        models(name)
PY
uv run --no-project --no-config --quiet --python "${python%.*}" python "$work/update.py" "$work" "$python"
git --no-pager diff --stat -- crates/documents
