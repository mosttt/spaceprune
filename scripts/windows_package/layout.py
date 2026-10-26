"""Build fixed WiX package metadata from the public Rust payload descriptor."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import uuid
import xml.etree.ElementTree as ET

NAMESPACE = "http://wixtoolset.org/schemas/v4/wxs"
IDENTITY_NAMESPACE = uuid.UUID("9e9f45ca-48f8-4eaa-8814-247e19bd982b")
CORE = Path(__file__).resolve().parents[2]
NOTICES = ["LICENSE", "THIRD-PARTY-LICENSES-RUST.txt", "THIRD-PARTY-LICENSES-RUST-STDLIB.html"]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def notice_paths(directory: Path, *, source_tree: bool = False) -> dict[str, Path]:
    return {name: directory / ("licenses/rust" if source_tree and name != "LICENSE" else "") / name
            for name in NOTICES}


def notice_digest(directory: Path, *, source_tree: bool = False) -> str:
    paths = notice_paths(directory, source_tree=source_tree)
    digest = hashlib.sha256()
    for name in NOTICES:
        digest.update(f"{name}\0{sha256(paths[name])}\n".encode())
    return digest.hexdigest()


def load_layout(core: Path, target: str, binaries: Path, notices: Path | None = None) -> dict:
    digests = [sha256(binaries / name) for name in ["spaceprune-readonly-helper.exe", "spaceprune-cleanup-helper.exe"]]
    host = next(line[6:] for line in subprocess.check_output(["rustc", "-vV"], encoding="utf-8").splitlines() if line.startswith("host: "))
    environment = dict(os.environ)
    environment.pop("SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY", None)
    result = subprocess.check_output(["cargo", "run", "--locked", "--quiet", "--target", host,
        "--package", "spaceprune-access", "--example", "windows_package_layout", "--", target, *digests,
        notice_digest(notices or core, source_tree=notices is None)], cwd=core, env=environment, encoding="utf-8")
    value = json.loads(result)
    if value["schemaVersion"] != 1 or len(value["components"]) != 2:
        raise ValueError("Unsupported Windows package layout")
    return value


def identifier(name: str) -> str:
    return str(uuid.uuid5(IDENTITY_NAMESPACE, name)).upper()


def prepare(layout: dict, binaries: Path, destination: Path, revision: str, notices: Path | None = None) -> Path:
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("An immutable Core source revision is required")
    version = layout["version"].split("-", 1)[0]
    fields = version.split(".")
    if len(fields) != 3 or any(not part.isdigit() for part in fields) or any(int(part) > limit for part, limit in zip(fields, [255, 255, 65535])):
        raise ValueError("The Core version does not fit Windows Installer's version fields")
    generation = layout["generationDirectory"]
    if notice_digest(notices or CORE, source_tree=notices is None) != layout["noticesSha256"]:
        raise ValueError("License materials changed after their identity was captured")
    ET.register_namespace("", NAMESPACE)
    def element(parent, name, **attributes):
        return ET.SubElement(parent, f"{{{NAMESPACE}}}{name}", attributes)
    wix = ET.Element(f"{{{NAMESPACE}}}Wix")
    package = element(wix, "Package", Name="SpacePrune Native Components", Manufacturer="SpacePrune",
        Version=version, Language="1033", InstallerVersion="500", Scope="perMachine",
        ProductCode=identifier(generation), UpgradeCode=identifier("spaceprune-native-components"))
    element(package, "MediaTemplate", EmbedCab="yes")
    element(package, "Property", Id="SPACEPRUNE_NATIVE_GENERATION", Value=generation)
    element(package, "Property", Id="SPACEPRUNE_CORE_REVISION", Value=revision)
    directory = element(package, "StandardDirectory", Id="ProgramFiles64Folder")
    directory = element(directory, "Directory", Id="NativeRoot", Name=layout["installRoot"])
    directory = element(directory, "Directory", Id="NativeGeneration", Name=generation)
    feature = element(package, "Feature", Id="NativeComponents", Title="SpacePrune native access components", Level="1")
    for index, entry in enumerate(layout["components"]):
        name = entry["filename"]
        source = (binaries / name).resolve()
        if sha256(source) != entry["sha256"]:
            raise ValueError("A native component changed after its identity was captured")
        component_id = f"NativeComponent{index}"
        component = element(directory, "Component", Id=component_id, Guid=identifier(f"{generation}/{name}"), Bitness="always64")
        element(component, "File", Id=f"NativeFile{index}", Source=f"$(var.PayloadDir)/{name}", Name=name, KeyPath="yes")
        element(feature, "ComponentRef", Id=component_id)
    license_directory = element(directory, "Directory", Id="NativeLicenses", Name="licenses")
    for index, name in enumerate(NOTICES):
        component_id = f"NativeLicense{index}"
        component = element(license_directory, "Component", Id=component_id,
            Guid=identifier(f"{generation}/licenses/{name}"), Bitness="always64")
        element(component, "File", Id=f"NativeLicenseFile{index}", Source=f"$(var.LicenseDir)/{name}", Name=name, KeyPath="yes")
        element(feature, "ComponentRef", Id=component_id)
    # No custom action, service or major-upgrade removal is emitted. Old running generations remain intact.
    destination.mkdir(parents=True, exist_ok=True)
    path = destination / "native-components.wxs"
    ET.indent(wix, space="  ")
    ET.ElementTree(wix).write(path, encoding="utf-8", xml_declaration=True)
    (destination / "native-package.json").write_text(json.dumps({
        "generator": "spaceprune-windows-native-package", "coreRevision": revision, "layout": layout,
    }, indent=2) + "\n")
    return path


def write_pin(layout: dict, package: Path, revision: str, destination: Path) -> dict:
    by_name = {entry["filename"]: entry["sha256"] for entry in layout["components"]}
    identity = {"schema_version": 1, "core_revision": revision, "msi_sha256": sha256(package),
        "payload": {"architecture": layout["architecture"],
            "metadata_sha256": by_name["spaceprune-readonly-helper.exe"],
            "cleanup_sha256": by_name["spaceprune-cleanup-helper.exe"],
            "notices_sha256": layout["noticesSha256"]}}
    destination.write_text(json.dumps(identity, indent=2) + "\n")
    return identity
