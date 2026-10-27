#!/usr/bin/env python3
"""Check native dependency boundaries without installing or executing target binaries."""
import argparse
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest-path", type=Path, default=Path(__file__).resolve().parents[1] / "Cargo.toml")
    parser.add_argument("--package", default="spaceprune-cli")
    args = parser.parse_args()
    linux = {"spaceprune-plugin-snapper", "spaceprune-plugin-system-linux", "spaceprune-platform-linux"}
    windows = {"spaceprune-windows-elevation"}
    shared = {"spaceprune-contracts", "spaceprune-plugin-filesystem", "spaceprune-plugin-developer-tools"}
    for target, required, excluded in [
        ("x86_64-pc-windows-msvc", shared | windows, linux),
        ("x86_64-unknown-linux-gnu", shared | linux, windows),
        ("x86_64-apple-darwin", shared, linux | windows),
    ]:
        output = subprocess.check_output([
            "cargo", "tree", "--locked", "--manifest-path", str(args.manifest_path.resolve()),
            "--package", args.package, "--target", target, "--edges", "normal",
            "--prefix", "none", "--format", "{p}",
        ], encoding="utf-8")
        packages = {line.split()[0] for line in output.splitlines() if line.strip()}
        missing, unexpected = required - packages, excluded & packages
        if missing or unexpected:
            raise SystemExit(f"{target}: missing={sorted(missing)}, unexpected={sorted(unexpected)}")
        print(f"{args.package}: {target}: native dependency boundaries OK")


if __name__ == "__main__":
    main()
