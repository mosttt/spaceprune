"""Stage public sources with resolved SPDX comments for cargo-about UTF-8 paths."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
destination = Path(sys.argv[1]).resolve()
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=root))
files = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root).decode("utf-8").split("\0")
for relative in set(files) - {""}:
    original = root / relative
    target = destination / relative
    if original.is_file():
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original, target)
for package in metadata["packages"]:
    if package["source"] is not None:
        continue
    target = destination / Path(package["manifest_path"]).relative_to(root)
    expression = package["license"]
    if not expression:
        raise RuntimeError(f"Missing SPDX expression for {package['name']}")
    manifest = target.read_text(encoding="utf-8")
    if expression not in manifest:
        target.write_text(f"# Effective audited license: {expression}\n" + manifest, encoding="utf-8", newline="\n")
