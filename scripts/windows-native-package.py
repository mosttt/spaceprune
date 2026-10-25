#!/usr/bin/env python3
"""Prepare or build Windows native component packages without installing them."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import shutil

from windows_package.build import build
from windows_package.layout import load_layout, notice_paths, prepare


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "build"])
    parser.add_argument("--target", required=True, choices=["x86_64-pc-windows-msvc", "x86_64-pc-windows-gnu", "aarch64-pc-windows-msvc"])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--binaries", type=Path, help="Existing payloads, required only by prepare")
    parser.add_argument("--profile", choices=["debug", "release"], default="release")
    parser.add_argument("--with-cli", action="store_true", help="Build and sign the portable CLI after its final MSI pin exists")
    parser.add_argument("--wix", default=os.environ.get("SPACEPRUNE_WINDOWS_WIX", "wix"))
    parser.add_argument("--timestamp-url", default="https://timestamp.digicert.com")
    signing = parser.add_mutually_exclusive_group()
    signing.add_argument("--certificate-sha1", default=os.environ.get("SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1"))
    signing.add_argument("--unsigned", action="store_true")
    args = parser.parse_args()
    core = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    if args.action == "prepare":
        if args.with_cli:
            parser.error("--with-cli requires build")
        if args.binaries is None:
            parser.error("prepare requires --binaries")
        notices = output / "licenses"
        notices.mkdir(parents=True, exist_ok=True)
        for name, source in notice_paths(core, source_tree=True).items():
            shutil.copy2(source, notices / name)
        layout = load_layout(core, args.target, args.binaries.resolve(), notices)
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=core, text=True).strip()
        result = {"wxs": str(prepare(layout, args.binaries.resolve(), output, revision, notices)),
            "defines": {"PayloadDir": str(args.binaries.resolve()), "LicenseDir": str(notices)}, "installed": False}
    else:
        if not args.unsigned and not args.certificate_sha1:
            parser.error("Choose a signing certificate or explicitly --unsigned for development")
        result = build(core, args.target, args.profile, output,
            None if args.unsigned else args.certificate_sha1, args.timestamp_url, args.wix, args.with_cli)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
