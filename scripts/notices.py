#!/usr/bin/env python3
"""Collect dependency license texts without embedding local manifest paths."""
import json
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked"], cwd=root))
output = root / "target/notices"
output.mkdir(parents=True, exist_ok=True)
summary = []
for package in metadata["packages"]:
    if package["id"] in metadata["workspace_members"]:
        continue
    summary.append({key: package.get(key) for key in ("name", "version", "license", "repository")})
    folder = Path(package["manifest_path"]).parent
    selected = []
    for pattern in ("LICENSE*", "COPYING*", "COPYRIGHT*", "OFL*", "fonts/**/LICENSE*", "fonts/**/OFL*"):
        selected.extend(p for p in folder.glob(pattern) if p.is_file())
    target = output / (package["name"] + "-" + package["version"])
    for source in set(selected):
        target.mkdir(exist_ok=True)
        shutil.copy2(source, target / source.name)
(output / "dependencies.json").write_text(json.dumps(summary, indent=2) + "\n")
print(f"Collected notices for {len(summary)} third-party packages")
