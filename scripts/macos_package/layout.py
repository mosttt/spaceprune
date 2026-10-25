"""Generate plist inputs from the runtime's fixed package descriptor."""

from __future__ import annotations

import json
import os
import plistlib
import re
import subprocess
from pathlib import Path


def load_layout(core: Path, team: str, distribution: str = "cli") -> dict:
    if not re.fullmatch(r"[A-Z0-9]{10}", team):
        raise ValueError("A ten-character Apple team identifier is required")
    host = next(line[6:] for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    if distribution not in {"cli", "desktop"}:
        raise ValueError("Unsupported native distribution")
    environment = dict(os.environ)
    environment["SPACEPRUNE_MACOS_DISTRIBUTION"] = distribution
    if host.endswith("apple-darwin"):
        environment["MACOSX_DEPLOYMENT_TARGET"] = minimum_version(host)
    value = subprocess.check_output([
        "cargo", "run", "--locked", "--quiet", "--target", host, "--package", "spaceprune-access",
        "--example", "macos_package_layout", "--", team,
    ], cwd=core, env=environment, text=True)
    layout = json.loads(value)
    if layout.get("distribution") != distribution or layout["schemaVersion"] != 1 or {service["kind"] for service in layout["services"]} != {"metadata", "cleanup"}:
        raise ValueError("Unsupported native package descriptor")
    return layout


def minimum_version(target: str) -> str:
    return {"x86_64-apple-darwin": "10.13", "aarch64-apple-darwin": "11.0"}[target]


def build_version(core: Path, explicit: str | None = None) -> str:
    if explicit is None:
        shallow = subprocess.check_output(["git", "rev-parse", "--is-shallow-repository"], cwd=core, text=True).strip()
        if shallow != "false":
            raise ValueError("A full Core history or an explicit monotonic build version is required")
        count = int(subprocess.check_output(["git", "rev-list", "--count", "HEAD"], cwd=core, text=True))
        explicit = f"{1 + count // 10000}.{count // 100 % 100}.{count % 100}"
    if not re.fullmatch(r"[1-9][0-9]{0,3}\.(?:0|[1-9][0-9]?)\.(?:0|[1-9][0-9]?)", explicit):
        raise ValueError("CFBundleVersion must be a monotonically increasing major.minor.patch (4/2/2 digits)")
    return explicit


def write_plist(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(plistlib.dumps(value, fmt=plistlib.FMT_XML, sort_keys=True))


def prepare_metadata(layout: dict, target: str, version: str, destination: Path, revision: str) -> dict:
    minimum = minimum_version(target)
    short_version = layout["version"].split("-", 1)[0]
    destination.mkdir(parents=True, exist_ok=True)
    app_info = {
        "CFBundleIdentifier": layout["authorizationIdentifier"],
        "CFBundleExecutable": layout["authorizationExecutable"],
        "CFBundleName": "SpacePruneAuthorization",
        "CFBundleDisplayName": "SpacePrune Authorization",
        "CFBundlePackageType": "APPL",
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleVersion": version,
        "CFBundleShortVersionString": short_version,
        "LSMinimumSystemVersion": minimum,
        "LSUIElement": True,
        "NSSystemAdministrationUsageDescription": "SpacePrune prepares its signed file access components when you request protected operations.",
        "SMPrivilegedExecutables": {service["label"]: service["requirement"] for service in layout["services"]},
    }
    write_plist(destination / "authorization-info.plist", app_info)
    environment = {}
    for service in layout["services"]:
        info = {
            "CFBundleIdentifier": service["label"],
            "CFBundleName": service["label"],
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleVersion": version,
            "CFBundleShortVersionString": short_version,
            "SMAuthorizedClients": [layout["authorizationRequirement"]],
        }
        job = {"Label": service["label"], "MachServices": {service["label"]: True}, "ProcessType": "Interactive"}
        info_path = destination / f"{service['kind']}-info.plist"
        job_path = destination / f"{service['kind']}-launchd.plist"
        write_plist(info_path, info)
        write_plist(job_path, job)
        # Both native mechanisms use the fixed executable's no-argument launchd entry.
        write_plist(destination / service["plist"], {
            **job, "BundleProgram": service["bundleProgram"], "ProgramArguments": [service["bundleProgram"]],
        })
        environment[f"SPACEPRUNE_MACOS_{service['kind'].upper()}_INFO_PLIST"] = str(info_path.resolve())
        environment[f"SPACEPRUNE_MACOS_{service['kind'].upper()}_JOB_PLIST"] = str(job_path.resolve())
    (destination / "native-package.json").write_text(json.dumps({
        "generator": "spaceprune-native-package", "schemaVersion": 1,
        "coreRevision": revision, "target": target, "buildVersion": version, "team": layout["team"],
        "layout": layout,
    }, indent=2) + "\n")
    return environment
