#!/usr/bin/env python3
"""Copy public stable release images into the Pages build, without a manifest."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

repo = os.environ["GITHUB_REPOSITORY"]
raw = subprocess.check_output(["gh", "api", f"repos/{repo}/releases?per_page=5&page=1"])
releases = json.loads(raw)[:5]
root = Path("dist/client/firmware")
for release in releases:
    tag = release.get("tag_name", "")
    if release.get("draft") or release.get("prerelease") or not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        continue
    for asset in release.get("assets", []):
        name, size = asset.get("name", ""), asset.get("size", 0)
        if not re.fullmatch(r"[a-z][a-z0-9_]*\.bin", name) or asset.get("state") != "uploaded" or not isinstance(size, int) or not 0 < size <= 16 * 1024 * 1024:
            continue
        folder = root / tag
        folder.mkdir(parents=True, exist_ok=True)
        subprocess.run(["gh", "release", "download", tag, "--repo", repo, "--pattern", name, "--dir", str(folder), "--clobber"], check=True)
        data = (folder / name).read_bytes()
        digest = asset.get("digest")
        if len(data) != size or not data.startswith(b"\xe9") or (digest and digest != "sha256:" + hashlib.sha256(data).hexdigest()):
            raise SystemExit(f"Invalid release image: {tag}/{name}")
