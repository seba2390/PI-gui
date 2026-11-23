#!/usr/bin/env python3
"""Writes the license notices of every crate linked into pistachio (normal dependencies only,
for one target platform), as required when distributing the binary.

Usage: script/third-party-licenses.py [target-triple] > THIRD-PARTY-LICENSES.txt
"""
import json
import os
import subprocess
import sys

NAMES = ("license", "licence", "copying", "notice", "unlicense", "copyright", "authors")


def main() -> None:
    # License texts contain names like "Michał"; Windows' console encoding can't write them.
    sys.stdout.reconfigure(encoding="utf-8")
    target = sys.argv[1] if len(sys.argv) > 1 else None
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if target:
        cmd += ["--filter-platform", target]
    meta = json.loads(subprocess.check_output(cmd))
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == "pistachio" and p["source"] is None)

    # Walk normal (linked) dependencies only: build and dev dependencies are not shipped.
    seen, stack = set(), [root]
    while stack:
        node = nodes[stack.pop()]
        for dep in node["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                stack.append(dep["pkg"])

    groups: dict[str, list[str]] = {}
    for pkg_id in sorted(seen, key=lambda i: (packages[i]["name"], packages[i]["version"])):
        pkg = packages[pkg_id]
        if pkg_id in meta["workspace_members"]:
            continue  # this project's own crates (vendored third-party crates are listed)
        directory = os.path.dirname(pkg["manifest_path"])
        files = sorted(
            f for f in os.listdir(directory)
            if f.lower().startswith(NAMES) and os.path.isfile(os.path.join(directory, f))
        )
        texts = []
        for name in files:
            with open(os.path.join(directory, name), encoding="utf-8", errors="replace") as handle:
                texts.append(f"--- {name} ---\n{handle.read().strip()}")
        text = "\n\n".join(texts) or f"(no license file shipped; license: {pkg.get('license')})"
        label = f"{pkg['name']} {pkg['version']} ({pkg.get('license') or 'see text'})"
        if pkg.get("repository"):
            label += f" {pkg['repository']}"
        groups.setdefault(text, []).append(label)

    out = sys.stdout
    out.write("Pistachio includes the following third-party software.\n")
    out.write("Each block lists the packages it applies to, followed by their license texts.\n\n")
    for text, labels in groups.items():
        out.write("=" * 78 + "\n")
        out.write("\n".join(labels) + "\n\n")
        out.write(text + "\n\n")

    # Native libraries linked statically through crates (not crates themselves).
    out.write("=" * 78 + "\n")
    out.write("ONNX Runtime (MIT) https://github.com/microsoft/onnxruntime\n")
    out.write("linked into sherpa-onnx's prebuilt libraries (speech to text)\n\n")
    out.write(
        "Copyright (c) Microsoft Corporation\n\n"
        "Permission is hereby granted, free of charge, to any person obtaining a copy of this "
        "software and associated documentation files (the \"Software\"), to deal in the Software "
        "without restriction, including without limitation the rights to use, copy, modify, "
        "merge, publish, distribute, sublicense, and/or sell copies of the Software, and to "
        "permit persons to whom the Software is furnished to do so, subject to the following "
        "conditions:\n\nThe above copyright notice and this permission notice shall be included "
        "in all copies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED "
        "\"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED "
        "TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND "
        "NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY "
        "CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, "
        "ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN "
        "THE SOFTWARE.\n\n"
    )

    # Models built into the binary.
    root_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    with open(os.path.join(root_dir, "crates/app/assets/silero-vad/LICENSE"), encoding="utf-8") as handle:
        silero = handle.read()
    out.write("=" * 78 + "\n")
    out.write("Silero VAD v5 (MIT) https://github.com/snakers4/silero-vad\n")
    out.write("voice activity detector built into the app (speech to text)\n\n")
    out.write(silero.strip() + "\n\n")

    # Packages that ship no license file are covered by these standard texts (each of them
    # is MIT, Apache-2.0, or offers one of the two).
    with open(os.path.join(root_dir, "LICENSE-MIT"), encoding="utf-8") as handle:
        mit = handle.read().split("\n", 2)[2]  # without this project's copyright line
    with open(os.path.join(root_dir, "LICENSE-APACHE"), encoding="utf-8") as handle:
        apache = handle.read()
    out.write("=" * 78 + "\n")
    out.write("Standard texts for the packages above marked \"no license file shipped\".\n\n")
    out.write("--- MIT License ---\n" + mit.strip() + "\n\n")
    out.write("--- Apache License 2.0 ---\n" + apache.strip() + "\n")


if __name__ == "__main__":
    main()
