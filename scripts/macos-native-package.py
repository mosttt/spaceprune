#!/usr/bin/env python3
"""Prepare, build or sign SpacePrune's fixed macOS artifacts without installing them."""

import argparse
import json
import os
import subprocess
from pathlib import Path

from macos_package.build import build
from macos_package.layout import build_version, load_layout, prepare_metadata
from macos_package.signing import sign, sign_desktop
from macos_package.provisioning import notarize_app


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "build", "sign-cli", "sign-desktop"])
    parser.add_argument("--team", default=os.environ.get("SPACEPRUNE_MACOS_TEAM_ID"), required="SPACEPRUNE_MACOS_TEAM_ID" not in os.environ)
    parser.add_argument("--target", choices=["x86_64-apple-darwin", "aarch64-apple-darwin"])
    parser.add_argument("--distribution", choices=["cli", "desktop"], default="cli")
    parser.add_argument("--profile", choices=["debug", "release"], default="release")
    parser.add_argument("--build-version")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--notarize", action="store_true", help="Notarize and staple each signed app during sign-desktop")
    signing = parser.add_mutually_exclusive_group()
    signing.add_argument("--identity", default=os.environ.get("SPACEPRUNE_MACOS_SIGNING_IDENTITY"))
    signing.add_argument("--unsigned", action="store_true")
    args = parser.parse_args()
    if args.notarize and args.action != "sign-desktop":
        parser.error("--notarize is supported only by sign-desktop")
    core = Path(__file__).resolve().parents[1]
    layout = load_layout(core, args.team, args.distribution)
    output = args.output.resolve()
    identity = None if args.unsigned else args.identity
    if args.action in {"prepare", "build"}:
        if args.target is None:
            parser.error("--target is required for package preparation and builds")
        version = build_version(core, args.build_version)
        if args.action == "prepare":
            revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=core, text=True).strip()
            result = prepare_metadata(layout, args.target, version, output, revision)
        else:
            if identity is None and not args.unsigned:
                parser.error("Choose --identity for distributable components or explicitly --unsigned for development")
            result = build(core, layout, args.target, args.profile, version, output, identity)
    else:
        if identity is None:
            parser.error("Signing requires --identity or SPACEPRUNE_MACOS_SIGNING_IDENTITY")
        if args.action == "sign-cli":
            sign(output, layout["cliIdentifier"], layout["cliRequirement"], identity)
        else:
            sign_desktop(output, layout, identity, notarize_app=notarize_app if args.notarize else None)
        result = {"signed": str(output)}
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
