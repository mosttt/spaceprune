"""Portable artifact checks; signatures and launchd still require native macOS validation."""

import json
import plistlib
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from macos_package.build import assemble
from macos_package.layout import build_version, load_layout, minimum_version, prepare_metadata
from macos_package.signing import sign_authorization, sign_desktop

CORE = Path(__file__).resolve().parents[2]


class NativePackageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.layout = load_layout(CORE, "A1B2C3D4E5", "cli")
        cls.desktop = load_layout(CORE, "A1B2C3D4E5", "desktop")

    def metadata(self, root, target="x86_64-apple-darwin"):
        metadata = root / "metadata with spaces"
        environment = prepare_metadata(self.layout, target, "1.2.34", metadata, "a" * 40)
        return metadata, environment

    def binaries(self, root):
        binaries = root / "binaries"
        binaries.mkdir()
        for name in [self.layout["authorizationExecutable"], *(service["binary"] for service in self.layout["services"])]:
            path = binaries / name
            path.write_bytes(b"unsigned test fixture: " + name.encode())
            path.chmod(0o755)
        return binaries

    def test_bless_metadata_uses_embedded_clients_and_no_custom_launch_arguments(self):
        with tempfile.TemporaryDirectory() as temporary:
            metadata, environment = self.metadata(Path(temporary))
            app = plistlib.loads((metadata / "authorization-info.plist").read_bytes())
            self.assertEqual(app["CFBundleIdentifier"], "com.spaceprune.authorization.cli")
            self.assertEqual(app["LSMinimumSystemVersion"], "10.13")
            self.assertEqual(app["CFBundleVersion"], "1.2.34")
            self.assertEqual(len(app["SMPrivilegedExecutables"]), 2)
            for service in self.layout["services"]:
                info = plistlib.loads(Path(environment[f"SPACEPRUNE_MACOS_{service['kind'].upper()}_INFO_PLIST"]).read_bytes())
                job = plistlib.loads(Path(environment[f"SPACEPRUNE_MACOS_{service['kind'].upper()}_JOB_PLIST"]).read_bytes())
                self.assertEqual(info["CFBundleIdentifier"], job["Label"])
                self.assertEqual(info["SMAuthorizedClients"], [self.layout["authorizationRequirement"]])
                self.assertNotIn("ProgramArguments", job)
                self.assertNotIn("Program", job)
                self.assertEqual(job["MachServices"], {service["label"]: True})
                self.assertIn('certificate leaf[subject.OU] = "A1B2C3D4E5"', app["SMPrivilegedExecutables"][service["label"]])
                modern = plistlib.loads((metadata / service["plist"]).read_bytes())
                self.assertEqual(modern["BundleProgram"], service["bundleProgram"])
                self.assertEqual(modern["ProgramArguments"], [service["bundleProgram"]])

    def test_app_layout_matches_both_cli_and_desktop_runtime_discovery(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, _ = self.metadata(root, "aarch64-apple-darwin")
            binaries = self.binaries(root)
            for directory in [root / "cli", root / "SpacePrune.app/Contents/Library"]:
                app = directory / self.layout["authorizationApp"]
                assemble(CORE, self.layout, metadata, binaries, app)
                self.assertTrue((app / "Contents/MacOS" / self.layout["authorizationExecutable"]).is_file())
                self.assertEqual(plistlib.loads((app / "Contents/Info.plist").read_bytes())["LSMinimumSystemVersion"], "11.0")
                for service in self.layout["services"]:
                    self.assertEqual((app / service["bundleProgram"]).read_bytes(), (binaries / service["binary"]).read_bytes())
                    self.assertTrue((app / "Contents/Library/LaunchDaemons" / service["plist"]).is_file())
                assemble(CORE, self.layout, metadata, binaries, app)
                self.assertEqual(json.loads((app / "Contents/Resources/native-package.json").read_text())["coreRevision"], "a" * 40)
            foreign = root / "foreign" / self.layout["authorizationApp"]
            foreign.mkdir(parents=True)
            (foreign / "keep").write_text("user data")
            with self.assertRaises(ValueError):
                assemble(CORE, self.layout, metadata, binaries, foreign)
            self.assertEqual((foreign / "keep").read_text(), "user data")

    def test_role_signing_is_inside_out_and_never_uses_recursive_signing(self):
        calls = []
        def run(args, **kwargs):
            self.assertTrue(kwargs["check"])
            calls.append(args)
        app = Path("/temporary build/SpacePrune.app")
        sign_desktop(app, self.desktop, "Developer ID Application: Fixture", run)
        signed = [call for call in calls if "--sign" in call]
        self.assertEqual([call[call.index("--identifier") + 1] for call in signed], [
            "com.spaceprune.metadata-v6-desktop", "com.spaceprune.cleanup-v4-desktop", "com.spaceprune.authorization.desktop", "com.spaceprune.cli", "com.spaceprune.desktop",
        ])
        self.assertTrue(all("--deep" not in call for call in signed))
        self.assertTrue(all(call[call.index("--options") + 1] == "runtime,hard,kill,library" for call in signed))
        self.assertTrue(any("-R" in call and self.layout["desktopRequirement"] in call for call in calls))
        with self.assertRaises(ValueError):
            sign_authorization(app, self.layout, "-", run)

    def test_signing_failure_keeps_previous_generated_bundle(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, _ = self.metadata(root)
            binaries = self.binaries(root)
            app = root / self.layout["authorizationApp"]
            assemble(CORE, self.layout, metadata, binaries, app)
            executable = app / "Contents/MacOS" / self.layout["authorizationExecutable"]
            previous = executable.read_bytes()
            (binaries / self.layout["authorizationExecutable"]).write_bytes(b"new executable")
            def reject(staged):
                self.assertEqual((staged / "Contents/MacOS" / self.layout["authorizationExecutable"]).read_bytes(), b"new executable")
                raise RuntimeError("signing rejected")
            with self.assertRaisesRegex(RuntimeError, "signing rejected"):
                assemble(CORE, self.layout, metadata, binaries, app, reject)
            self.assertEqual(executable.read_bytes(), previous)

    def test_inner_notarization_finishes_before_outer_signature(self):
        events = []
        app = Path("/fixture/SpacePrune.app")
        def run(args, **kwargs):
            if "--sign" in args:
                events.append(("sign", args[-1]))
        def notarize(path):
            events.append(("notarize", str(path)))
        sign_desktop(app, self.desktop, "Fixture identity", run, notarize)
        inner = str(app / "Contents/Library/SpacePruneAuthorization.app")
        self.assertLess(events.index(("sign", inner)), events.index(("notarize", inner)))
        self.assertLess(events.index(("notarize", inner)), events.index(("sign", str(app))))
        self.assertEqual(events[-1], ("notarize", str(app)))

    def test_build_version_respects_apples_format_and_the_existing_intel_floor(self):
        self.assertEqual(build_version(CORE, "1.2.34"), "1.2.34")
        for invalid in ["0.1.0", "1.100.0", "10000.0.0", "1.2.3.4", "latest"]:
            with self.assertRaises(ValueError):
                build_version(CORE, invalid)
        self.assertNotEqual(self.layout["authorizationIdentifier"], self.desktop["authorizationIdentifier"])
        self.assertTrue({entry["label"] for entry in self.layout["services"]}.isdisjoint({entry["label"] for entry in self.desktop["services"]}))
        self.assertEqual(minimum_version("x86_64-apple-darwin"), "10.13")
        self.assertEqual(minimum_version("aarch64-apple-darwin"), "11.0")
        with self.assertRaises(ValueError):
            load_layout(CORE, 'A1B2C3D4E5" or true')


if __name__ == "__main__":
    unittest.main()
