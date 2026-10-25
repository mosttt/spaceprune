"""Credential lifecycle checks with native commands replaced; no real keychain is touched."""

import base64
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from macos_package import provisioning


class ProvisioningTests(unittest.TestCase):
    def test_security_failure_never_logs_password_argv_or_native_output(self):
        result = subprocess.CompletedProcess([], 1, "private-key-output", "password-output")
        with patch.object(provisioning.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, r"^security import failed \(exit 1\)$"):
                provisioning.security("import", "fixture.p12", "-P", "private-password")

    def test_import_failure_removes_sensitive_files_and_temporary_keychain(self):
        with tempfile.TemporaryDirectory() as temporary:
            calls = []
            def security(*args):
                calls.append(args)
                if args[0] == "create-keychain":
                    Path(args[-1]).write_text("keychain fixture")
                if args[0] == "import":
                    self.assertEqual(Path(args[1]).read_bytes(), b"private fixture")
                    raise RuntimeError("import failed")
                return ""
            with patch.dict(os.environ, {
                "RUNNER_TEMP": temporary, "APPLE_CERTIFICATE": base64.b64encode(b"private fixture").decode(),
                "APPLE_CERTIFICATE_PASSWORD": "fixture-password",
            }), patch.object(provisioning, "security", side_effect=security):
                with self.assertRaisesRegex(RuntimeError, "import failed"):
                    provisioning.create_keychain(Path(temporary) / "metadata.json")
            self.assertEqual(list(Path(temporary).iterdir()), [])
            self.assertTrue(any(call[0] == "delete-keychain" for call in calls))

    def test_cleanup_preserves_keychains_added_by_another_job(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "spaceprune-signing-fixture"
            directory.mkdir()
            keychain = directory / "build.keychain-db"
            keychain.touch()
            metadata = Path(temporary) / "metadata.json"
            metadata.write_text(json.dumps({"generator": "spaceprune-signing-keychain", "path": str(keychain), "directory": str(directory)}))
            calls = []
            def security(*args):
                calls.append(args)
                return f'"{keychain}" "/another job/keychain" "/login.keychain-db"' if args == ("list-keychains", "-d", "user") else ""
            with patch.object(provisioning, "security", side_effect=security):
                provisioning.delete_keychain(metadata)
            self.assertIn(("list-keychains", "-d", "user", "-s", "/another job/keychain", "/login.keychain-db"), calls)
            self.assertFalse(directory.exists())
            self.assertFalse(metadata.exists())

    def test_rejected_notarization_cleans_key_and_does_not_print_raw_response(self):
        with tempfile.TemporaryDirectory() as temporary:
            def run(args, **kwargs):
                key = Path(args[args.index("--key") + 1])
                self.assertEqual(key.read_text(), "private fixture")
                self.assertEqual(key.stat().st_mode & 0o777, 0o600)
                return subprocess.CompletedProcess(args, 1, "private fixture", "private fixture")
            with patch.dict(os.environ, {"RUNNER_TEMP": temporary, "APPLE_API_KEY_ID": "fixture",
                "APPLE_API_ISSUER": "fixture", "APPLE_API_PRIVATE_KEY": "private fixture"}), patch.object(provisioning.subprocess, "run", side_effect=run):
                with self.assertRaisesRegex(RuntimeError, "^notarytool failed") as raised:
                    provisioning.notarize(Path(temporary) / "fixture.zip")
            self.assertNotIn("private fixture", str(raised.exception))
            self.assertEqual(list(Path(temporary).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
