#!/usr/bin/env python3
"""Writes the license notices of every crate linked into pi-gui (normal dependencies only,
for one target platform), as required when distributing the binary.

Usage: script/third-party-licenses.py [target-triple] > THIRD-PARTY-LICENSES.txt
"""
import json
import os
import subprocess
import sys

NAMES = ("license", "licence", "copying", "notice", "unlicense", "copyright", "authors")


def main() -> None:
    target = sys.argv[1] if len(sys.argv) > 1 else None
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if target:
        cmd += ["--filter-platform", target]
    meta = json.loads(subprocess.check_output(cmd))
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == "pi-gui" and p["source"] is None)

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
        if pkg["source"] is None:
            continue  # this project's own crates
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
    out.write("PI-gui includes the following third-party software.\n")
    out.write("Each block lists the packages it applies to, followed by their license texts.\n\n")
    for text, labels in groups.items():
        out.write("=" * 78 + "\n")
        out.write("\n".join(labels) + "\n\n")
        out.write(text + "\n\n")

    # Packages that ship no license file are covered by these standard texts (each of them
    # is MIT, Apache-2.0, or offers one of the two).
    root_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
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
