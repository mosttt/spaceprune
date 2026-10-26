"""Locate Windows packaging tools without changing PATH or system trust."""
from __future__ import annotations

import os
from pathlib import Path
import platform
import re
import shutil


class BuildToolError(ValueError):
    """A build prerequisite is absent or explicitly points to a missing file."""


def resolve_signtool() -> str:
    configured = os.environ.get("SPACEPRUNE_WINDOWS_SIGNTOOL")
    if configured:
        found = shutil.which(configured)
        if found:
            return found
        raise BuildToolError(
            f"SPACEPRUNE_WINDOWS_SIGNTOOL does not name an executable: {configured}. "
            "Correct the path, or unset it to search the installed Windows SDK."
        )
    found = shutil.which("signtool.exe")
    if found:
        return found

    roots = []
    if os.environ.get("WindowsSdkDir"):
        roots.append(Path(os.environ["WindowsSdkDir"]))
    for variable in ["ProgramFiles(x86)", "ProgramFiles"]:
        if os.environ.get(variable):
            roots.append(Path(os.environ[variable]) / "Windows Kits" / "10")
    # SignTool runs on the build host, regardless of the payload's target architecture.
    host = platform.machine().lower()
    architecture = {"amd64": "x64", "x86_64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(host, "x86")
    architectures = [architecture] if architecture == "x86" else [architecture, "x86"]
    for root in roots:
        versions = sorted(
            (folder for folder in (root / "bin").glob("*")
             if re.fullmatch(r"\d+(?:\.\d+)*", folder.name)),
            key=lambda folder: tuple(map(int, folder.name.split("."))), reverse=True,
        )
        for architecture in architectures:
            for folder in [*versions, root / "bin"]:
                candidate = folder / architecture / "signtool.exe"
                if candidate.is_file():
                    return str(candidate)
    raise BuildToolError(
        "Windows SDK signing tool signtool.exe was not found. "
        "Install the Windows SDK Signing Tools through the Windows SDK installer "
        "or Visual Studio Installer, then retry. For a custom location, set "
        "SPACEPRUNE_WINDOWS_SIGNTOOL to the full signtool.exe path. "
        "Keep your existing signing certificate; installing a certificate does not install SignTool."
    )


def resolve_wix(command: str) -> str:
    found = shutil.which(command)
    if found:
        return found
    raise BuildToolError(
        f"WiX executable was not found: {command}. Install the .NET SDK and run "
        "'dotnet tool install --global wix --version 4.0.6', then reopen the terminal. "
        "For an existing custom installation, set SPACEPRUNE_WINDOWS_WIX to its executable path."
    )
