# SPDX-License-Identifier: Apache-2.0

"""Collect published notices for the locked Linux release dependency graph."""

import json
from pathlib import Path
import re
import shutil
import subprocess
import sys


def collect(destination: Path) -> None:
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--filter-platform", "x86_64-unknown-linux-gnu",
    ]))
    workspace = set(metadata["workspace_members"])
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    pending = list(workspace)
    visited: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        pending.extend(
            dep["pkg"] for dep in nodes[package_id]["deps"]
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"])
        )

    dependencies = visited - workspace
    index = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] not in dependencies:
            continue
        name = f'{package["name"]}-{package["version"]}'
        source = Path(package["manifest_path"]).parent
        notices = {
            file for file in source.iterdir()
            if file.is_file() and re.match(r"^(licen[cs]e|copying|notice|copyright|unlicense)", file.name, re.I)
        }
        if package["license_file"]:
            notices.add(source / package["license_file"])
        if name in {"uuid-simd-0.8.0", "vsimd-0.8.0"} and not notices:
            # These crate archives omit the license from their upstream tree.
            # Source: github.com/Nugine/simd at d74c030d9dc4f3cae02146d1f497ff62726ef09a.
            notices.add(Path(__file__).with_name("licenses") / "Nugine-simd-MIT.txt")
        if not notices:
            raise SystemExit(f"No license or notice file found for {name}")
        target = destination / name
        target.mkdir(parents=True)
        for file in sorted(notices):
            shutil.copyfile(file, target / file.name)
        index.append(f'{name}\t{package["license"] or package["license_file"]}')

    (destination / "INDEX.txt").write_text("\n".join(index) + "\n", encoding="utf-8")


if __name__ == "__main__":
    collect(Path(sys.argv[1]))
