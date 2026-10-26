"""Create a signed MSI and its exact package pin; never invoke installation."""
from __future__ import annotations

import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile

from .layout import notice_paths, load_layout, prepare, write_pin


def sign(path: Path, certificate: str, timestamp: str, run=subprocess.run) -> None:
    if not re.fullmatch(r"[A-Fa-f0-9]{40}", certificate):
        raise ValueError("Provide a certificate SHA-1 selector from the Windows certificate store")
    tool = os.environ.get("SPACEPRUNE_WINDOWS_SIGNTOOL", "signtool")
    run([tool, "sign", "/sha1", certificate, "/fd", "SHA256", "/tr", timestamp, "/td", "SHA256", str(path)], check=True)
    run([tool, "verify", "/pa", "/all", str(path)], check=True)


def build(core: Path, target: str, profile: str, output: Path, certificate: str | None,
          timestamp: str, wix: str, include_cli: bool = False) -> dict:
    if platform.system() != "Windows":
        raise ValueError("MSI builds require Windows, its SDK and WiX 4 or newer")
    if certificate:
        subprocess.run(["git", "diff", "--exit-code", "HEAD", "--", "."], cwd=core, check=True, stdout=subprocess.DEVNULL)
    if output.exists():
        marker = output / "native-package.json"
        if output.is_symlink() or not marker.is_file() or json.loads(marker.read_text(encoding="utf-8")).get("generator") != "spaceprune-windows-native-package":
            raise ValueError("Refusing to replace unrelated files")
    environment = dict(os.environ)
    # Helpers are built before the final package pin exists; a host pin would create a hash cycle.
    environment.pop("SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY", None)
    if target.endswith("-msvc"):
        if "CARGO_ENCODED_RUSTFLAGS" in environment:
            existing = environment["CARGO_ENCODED_RUSTFLAGS"]
            environment["CARGO_ENCODED_RUSTFLAGS"] = (
                (existing + "\x1f" if existing else "") + "-C\x1ftarget-feature=+crt-static"
            )
        else:
            environment["RUSTFLAGS"] = (environment.get("RUSTFLAGS", "") + " -C target-feature=+crt-static").strip()
    subprocess.run(["cargo", "build", "--locked", "--package", "spaceprune-readonly-helper",
        "--bin", "spaceprune-readonly-helper", "--bin", "spaceprune-cleanup-helper", "--target", target,
        *(["--release"] if profile == "release" else [])], cwd=core, env=environment, check=True)
    cargo = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=core, encoding="utf-8"))
    binaries = Path(cargo["target_directory"]) / target / profile
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=core, encoding="utf-8").strip()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="spaceprune-windows-stage-", dir=output.parent) as temporary:
        staged = Path(temporary) / "package"
        payload = staged / "payload"
        payload.mkdir(parents=True)
        notices = staged / "licenses"
        notices.mkdir()
        for name, source in notice_paths(core, source_tree=True).items():
            shutil.copy2(source, notices / name)
        for name in ["spaceprune-readonly-helper.exe", "spaceprune-cleanup-helper.exe"]:
            shutil.copy2(binaries / name, payload / name)
            if certificate:
                sign(payload / name, certificate, timestamp)
        layout = load_layout(core, target, payload, notices)
        native = platform.machine().lower() in ({"amd64", "x86_64"} if layout["architecture"] == "x64" else {"arm64", "aarch64"})
        if native:
            for entry in layout["components"]:
                subprocess.run([str(payload / entry["filename"]), entry["probe"]], check=True)
        wxs = prepare(layout, payload, staged, revision, notices)
        package = staged / layout["packageName"]
        subprocess.run([wix, "build", "-arch", layout["architecture"], "-d", f"PayloadDir={payload}",
            "-d", f"LicenseDir={notices}", "-o", str(package), str(wxs)], check=True)
        if certificate:
            sign(package, certificate, timestamp)
        write_pin(layout, package, revision, staged / "package-identity.json")
        if include_cli:
            host_environment = {**environment, "SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY": str(staged / "package-identity.json")}
            subprocess.run(["cargo", "build", "--locked", "--package", "spaceprune-cli", "--target", target,
                *(["--release"] if profile == "release" else [])], cwd=core, env=host_environment, check=True)
            executable = staged / "spaceprune.exe"
            shutil.copy2(binaries / "spaceprune.exe", executable)
            if certificate:
                sign(executable, certificate, timestamp)
        if output.exists():
            previous = Path(temporary) / "previous"
            output.rename(previous)
            try:
                staged.rename(output)
            except BaseException:
                previous.rename(output)
                raise
        else:
            staged.rename(output)
    return {"package": str(output / layout["packageName"]), "identity": str(output / "package-identity.json"),
        "cli": str(output / "spaceprune.exe") if include_cli else None,
        "signed": certificate is not None, "nativeProbesExecuted": native, "installed": False}
