"""Portable Windows package checks; no Windows Installer or elevation runs in these fixtures."""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from windows_package import build as builder
from windows_package.layout import NAMESPACE, NOTICES, load_layout, notice_paths, prepare, sha256, write_pin

CORE = Path(__file__).resolve().parents[2]
TARGET = "x86_64-pc-windows-msvc"
REVISION = "a" * 40
XML = {"w": NAMESPACE}


class WindowsPackageTests(unittest.TestCase):
    def payload(self, root):
        binaries = root / "payload with spaces"
        binaries.mkdir(parents=True)
        for name, content in [("spaceprune-readonly-helper.exe", b"metadata fixture"),
                              ("spaceprune-cleanup-helper.exe", b"cleanup fixture")]:
            (binaries / name).write_bytes(content)
        return binaries

    def test_package_contains_only_fixed_files_in_program_files_without_custom_actions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = self.payload(root)
            layout = load_layout(CORE, TARGET, binaries)
            self.assertTrue(layout["generationDirectory"].startswith("native-7-5-"))
            self.assertEqual({entry["filename"]: entry["probe"] for entry in layout["components"]}, {
                "spaceprune-readonly-helper.exe": "--supports-windows-metadata-v7",
                "spaceprune-cleanup-helper.exe": "--supports-cleanup-v5",
            })
            wxs = prepare(layout, binaries, root / "metadata", REVISION)
            document = ET.parse(wxs)
            package = document.find("w:Package", XML)
            self.assertEqual(package.get("Scope"), "perMachine")
            self.assertEqual(document.find(".//w:StandardDirectory", XML).get("Id"), "ProgramFiles64Folder")
            self.assertEqual(document.find(".//w:Directory[@Id='NativeGeneration']", XML).get("Name"), layout["generationDirectory"])
            files = document.findall(".//w:File", XML)
            self.assertEqual({file.get("Name") for file in files}, {entry["filename"] for entry in layout["components"]} | set(NOTICES))
            self.assertEqual(len([file for file in files if file.get("Name").endswith(".exe")]), 2)
            self.assertTrue(all(file.get("Source").startswith(("$(var.PayloadDir)/", "$(var.LicenseDir)/")) for file in files))
            for forbidden in ["CustomAction", "ServiceInstall", "ServiceControl", "MajorUpgrade", "RemoveFile", "RegistryValue"]:
                self.assertEqual(document.findall(f".//w:{forbidden}", XML), [])
            self.assertEqual(document.find(".//w:MediaTemplate", XML).get("EmbedCab"), "yes")

    def test_prepare_cli_exports_a_complete_flat_license_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = self.payload(root)
            output = root / "prepared metadata"
            result = json.loads(subprocess.check_output([
                sys.executable, str(CORE / "scripts/windows-native-package.py"), "prepare",
                "--target", TARGET, "--binaries", str(binaries), "--output", str(output),
            ], cwd=CORE, text=True))
            notices = Path(result["defines"]["LicenseDir"])
            self.assertEqual(notices, output / "licenses")
            self.assertEqual({path.name for path in notices.iterdir()}, set(NOTICES))
            for name, source in notice_paths(CORE, source_tree=True).items():
                self.assertEqual((notices / name).read_bytes(), source.read_bytes())
            self.assertFalse(result["installed"])

    def test_payload_updates_keep_old_generation_paths_and_change_product_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = self.payload(root)
            previous = load_layout(CORE, TARGET, binaries)
            old = prepare(previous, binaries, root / "old", REVISION)
            (binaries / "spaceprune-readonly-helper.exe").write_bytes(b"updated metadata fixture")
            with self.assertRaisesRegex(ValueError, "changed after"):
                prepare(previous, binaries, root / "mismatched", REVISION)
            current = load_layout(CORE, TARGET, binaries)
            new = prepare(current, binaries, root / "new", REVISION)
            self.assertNotEqual(previous["generationDirectory"], current["generationDirectory"])
            old_product = ET.parse(old).find("w:Package", XML)
            new_product = ET.parse(new).find("w:Package", XML)
            self.assertNotEqual(old_product.get("ProductCode"), new_product.get("ProductCode"))
            self.assertEqual(old_product.get("UpgradeCode"), new_product.get("UpgradeCode"))

    def test_final_msi_pin_is_accepted_by_the_rust_identity_contract(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = self.payload(root)
            layout = load_layout(CORE, TARGET, binaries)
            package = root / layout["packageName"]
            package.write_bytes(b"fixture bytes after signing")
            pin = root / "package-identity.json"
            identity = write_pin(layout, package, REVISION, pin)
            self.assertEqual(identity["msi_sha256"], sha256(package))
            host = next(line[6:] for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
            actual = subprocess.check_output(["cargo", "run", "--locked", "--quiet", "--target", host, "-p", "spaceprune-access",
                "--example", "windows_package_layout", "--", "--check-pin", str(pin)], cwd=CORE, text=True)
            self.assertEqual(json.loads(actual), identity)
            identity["destination"] = "C:\\user-selected"
            pin.write_text(json.dumps(identity))
            rejected = subprocess.run(["cargo", "run", "--locked", "--quiet", "--target", host, "-p", "spaceprune-access",
                "--example", "windows_package_layout", "--", "--check-pin", str(pin)], cwd=CORE, capture_output=True)
            self.assertNotEqual(rejected.returncode, 0)

    def test_license_content_changes_require_a_new_generation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = self.payload(root)
            notices = root / "licenses"
            notices.mkdir()
            for name, source in notice_paths(CORE, source_tree=True).items():
                shutil.copy2(source, notices / name)
            previous = load_layout(CORE, TARGET, binaries, notices)
            material = notices / "THIRD-PARTY-LICENSES-RUST.txt"
            material.write_bytes(material.read_bytes() + b"\nFixture license update\n")
            current = load_layout(CORE, TARGET, binaries, notices)
            self.assertNotEqual(previous["generationDirectory"], current["generationDirectory"])
            with self.assertRaisesRegex(ValueError, "changed after"):
                prepare(previous, binaries, root / "stale", REVISION, notices)

    def test_signing_uses_the_certificate_store_and_sha256_then_verifies(self):
        calls = []
        def run(arguments, **options):
            self.assertTrue(options["check"])
            calls.append(arguments)
        package = Path("fixture with spaces") / "SpacePruneNative.msi"
        builder.sign(package, "a" * 40, "https://timestamp.example.test", run, tool="SDK with spaces/signtool.exe")
        self.assertEqual([call[1] for call in calls], ["sign", "verify"])
        self.assertTrue(all(call[0] == "SDK with spaces/signtool.exe" for call in calls))
        self.assertIn("SHA256", calls[0])
        self.assertNotIn("/p", calls[0])
        self.assertEqual(calls[0][-1], str(package))

    def test_failed_package_signing_keeps_the_previous_generated_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = self.payload(root)
            layout = load_layout(CORE, TARGET, binaries)
            target_directory = root / "target"
            built = target_directory / TARGET / "release"
            built.mkdir(parents=True)
            for source in binaries.iterdir():
                (built / source.name).write_bytes(source.read_bytes())
            output = root / "existing"
            output.mkdir()
            (output / "native-package.json").write_text(json.dumps({"generator": "spaceprune-windows-native-package"}))
            previous = output / "SpacePruneNative.msi"
            previous.write_bytes(b"previous accepted artifact")
            calls = []
            def run(arguments, **options):
                calls.append(arguments)
                if arguments[:2] == ["cargo", "build"]:
                    self.assertEqual(options["env"]["CARGO_ENCODED_RUSTFLAGS"], "-C\x1ftarget-feature=+crt-static")
                    self.assertNotIn("SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY", options["env"])
                if arguments[0] == "wix":
                    Path(arguments[arguments.index("-o") + 1]).write_bytes(b"new MSI fixture")
            def check_output(arguments, **options):
                return json.dumps({"target_directory": str(target_directory)}) if arguments[0] == "cargo" else REVISION
            def sign(path, certificate, timestamp, *, tool):
                self.assertEqual(tool, "fixture-signtool.exe")
                if path.suffix == ".msi": raise RuntimeError("MSI signature rejected")
            with patch.object(builder.platform, "system", return_value="Windows"), patch.object(builder, "load_layout", return_value=layout), \
                 patch.object(builder.subprocess, "run", side_effect=run), patch.object(builder.subprocess, "check_output", side_effect=check_output), \
                 patch.object(builder, "resolve_signtool", return_value="fixture-signtool.exe"), \
                 patch.object(builder, "resolve_wix", return_value="wix"), patch.object(builder, "sign", side_effect=sign), \
                 patch.dict(os.environ, {"CARGO_ENCODED_RUSTFLAGS": "", "SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY": "old-host-pin.json"}):
                with self.assertRaisesRegex(RuntimeError, "signature rejected"):
                    builder.build(CORE, TARGET, "release", output, "a" * 40, "https://timestamp.example.test", "wix")
            self.assertEqual(previous.read_bytes(), b"previous accepted artifact")
            self.assertTrue(any(call[0] == "wix" and "build" in call for call in calls))
            self.assertFalse(any("/i" in call or "msiexec" in call for call in calls))

    def test_portable_cli_is_built_after_the_final_signed_msi_with_its_exact_pin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = self.payload(root)
            layout = load_layout(CORE, TARGET, binaries)
            target_directory = root / "构建€target"
            built = target_directory / TARGET / "release"
            built.mkdir(parents=True)
            for source in binaries.iterdir():
                shutil.copy2(source, built / source.name)
            output = root / "distribution"
            signed = []
            def sign(path, certificate, timestamp, *, tool):
                self.assertEqual(tool, "fixture-signtool.exe")
                signed.append(path.name)
                if path.suffix == ".msi":
                    path.write_bytes(path.read_bytes() + b"final signature fixture")
            def run(arguments, **options):
                if arguments[0] == "wix":
                    Path(arguments[arguments.index("-o") + 1]).write_bytes(b"MSI fixture")
                elif arguments[:2] == ["cargo", "build"]:
                    environment = options["env"]
                    if "spaceprune-cli" in arguments:
                        pin = Path(environment["SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY"])
                        identity = json.loads(pin.read_bytes())
                        self.assertEqual(identity["msi_sha256"], sha256(pin.parent / "SpacePruneNative.msi"))
                        package = Path(environment["SPACEPRUNE_WINDOWS_EMBED_PACKAGE"])
                        self.assertEqual(package, pin.parent / "SpacePruneNative.msi")
                        self.assertIn("SpacePruneNative.msi", signed)
                        (built / "spaceprune.exe").write_bytes(b"Host fixture embeds:" + pin.read_bytes() + package.read_bytes())
                    else:
                        self.assertNotIn("SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY", environment)
                        self.assertNotIn("SPACEPRUNE_WINDOWS_EMBED_PACKAGE", environment)
            def check_output(arguments, **options):
                # Cargo emits UTF-8 even when Windows/Python's locale defaults to GBK.
                raw = (json.dumps({"target_directory": str(target_directory)}, ensure_ascii=False)
                       if arguments[0] == "cargo" else REVISION).encode("utf-8")
                return raw.decode(options.get("encoding") or "gbk")
            with patch.object(builder.platform, "system", return_value="Windows"), patch.object(builder, "load_layout", return_value=layout), \
                 patch.object(builder.subprocess, "run", side_effect=run), patch.object(builder.subprocess, "check_output", side_effect=check_output), \
                 patch.object(builder, "resolve_signtool", return_value="fixture-signtool.exe"), \
                 patch.object(builder, "resolve_wix", return_value="wix"), patch.object(builder, "sign", side_effect=sign), \
                 patch.dict(os.environ, {"SPACEPRUNE_WINDOWS_EMBED_PACKAGE": "old-host.msi"}):
                result = builder.build(CORE, TARGET, "release", output, "a" * 40, "https://timestamp.example.test", "wix", True)
            self.assertEqual(Path(result["cli"]).read_bytes(), b"Host fixture embeds:" + (output / "package-identity.json").read_bytes() + (output / "SpacePruneNative.msi").read_bytes())
            self.assertEqual(signed[-2:], ["SpacePruneNative.msi", "spaceprune.exe"])
            self.assertFalse(result["installed"])


if __name__ == "__main__":
    unittest.main()
