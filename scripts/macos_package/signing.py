"""Explicit inside-out signing; never use recursive signing to choose executable roles."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

OPTIONS = "runtime,hard,kill,library"


def sign(path: Path, identifier: str, requirement: str, identity: str, run=subprocess.run) -> None:
    if not identity or identity == "-":
        raise ValueError("Native authorization requires a team-backed Apple signing identity, not ad-hoc signing")
    keychain = os.environ.get("SPACEPRUNE_MACOS_KEYCHAIN")
    run(["codesign", "--force", "--timestamp", "--options", OPTIONS, "--identifier", identifier, "--sign", identity,
         *(["--keychain", keychain] if keychain else []), str(path)], check=True)
    run(["codesign", "--verify", "--strict", "--all-architectures", "-R", requirement, str(path)], check=True)


def sign_authorization(app: Path, layout: dict, identity: str, run=subprocess.run) -> None:
    for service in layout["services"]:
        sign(app / service["bundleProgram"], service["label"], service["requirement"], identity, run)
    sign(app, layout["authorizationIdentifier"], layout["authorizationRequirement"], identity, run)
    run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)


def sign_desktop(app: Path, layout: dict, identity: str, run=subprocess.run, notarize_app=None) -> None:
    if layout["distribution"] != "desktop":
        raise ValueError("Desktop signing requires its own native registration identity")
    authorization = app / "Contents/Library" / layout["authorizationApp"]
    sign_authorization(authorization, layout, identity, run)
    if notarize_app:
        notarize_app(authorization)
    sign(app / "Contents/MacOS/spaceprune-runner", layout["cliIdentifier"], layout["cliRequirement"], identity, run)
    sign(app, layout["desktopIdentifier"], layout["desktopRequirement"], identity, run)
    if notarize_app:
        notarize_app(app)
    run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
