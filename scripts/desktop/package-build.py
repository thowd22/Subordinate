#!/usr/bin/env python3
"""Package only current-commit executables reported by Cargo, never baked binaries."""
import hashlib
import json
import os
from pathlib import Path
import shutil

out = Path("dist")
out.mkdir(exist_ok=True)
expected = {"subordinate", "subordinate-cli", "subordinate-mcp", "empty_timeline_drop",
            "empty_track_menu", "media_import_app", "stdio"}
found = {}
for line in Path("desktop-build.json").read_text(encoding="utf-8").splitlines():
    item = json.loads(line)
    name = item.get("target", {}).get("name")
    executable = item.get("executable")
    if name in expected and executable:
        source = Path(executable)
        # Cargo may also report the bin's test harness: only keep normal bins.
        if name.startswith("subordinate") and item.get("profile", {}).get("test"):
            continue
        dest = out / (name + source.suffix if source.suffix == ".exe" else name)
        shutil.copy2(source, dest)
        found[name] = {"file": dest.name, "sha256": hashlib.sha256(dest.read_bytes()).hexdigest()}
if expected - found.keys():
    raise SystemExit(f"missing build outputs: {sorted(expected - found.keys())}")
(out / "build-info.json").write_text(json.dumps({"sha": os.environ["GITHUB_SHA"],
    "executables": found}, indent=2), encoding="utf-8")
