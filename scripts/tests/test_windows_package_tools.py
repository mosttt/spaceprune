"""SDK discovery fixtures do not execute Windows tools or modify certificate stores."""
import os
import contextlib
import io
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from windows_package import build as builder, tools


class WindowsPackageToolTests(unittest.TestCase):
    def sdk_tool(self, root, version, architecture="x64"):
        tool = root / "Windows Kits" / "10" / "bin" / version / architecture / "signtool.exe"
        tool.parent.mkdir(parents=True, exist_ok=True)
        tool.write_bytes(b"fixture only")
        return tool

    def test_sdk_not_on_path_uses_latest_numeric_version_for_host_not_payload(self):
        with tempfile.TemporaryDirectory(prefix="SDK with spaces ") as temporary:
            root = Path(temporary)
            self.sdk_tool(root, "10.0.9.0")
            expected = self.sdk_tool(root, "10.0.10.0")
            self.sdk_tool(root, "10.0.11.0", "arm64")
            with patch.dict(os.environ, {"ProgramFiles(x86)": str(root)}, clear=True), \
                 patch.object(tools.shutil, "which", return_value=None), \
                 patch.object(tools.platform, "machine", return_value="AMD64"):
                self.assertEqual(tools.resolve_signtool(), str(expected))

    def test_developer_sdk_directory_and_x86_host_tool_are_supported(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            expected = self.sdk_tool(root, "10.0.10.0", "x86")
            with patch.dict(os.environ, {"WindowsSdkDir": str(root / "Windows Kits" / "10")}, clear=True), \
                 patch.object(tools.shutil, "which", return_value=None), \
                 patch.object(tools.platform, "machine", return_value="ARM64"):
                self.assertEqual(tools.resolve_signtool(), str(expected))

    def test_explicit_tool_or_path_tool_takes_precedence_over_sdk_search(self):
        for environment, command in [
            ({"SPACEPRUNE_WINDOWS_SIGNTOOL": "custom/signtool.exe"}, "custom/signtool.exe"),
            ({}, "signtool.exe"),
        ]:
            with patch.dict(os.environ, environment, clear=True), \
                 patch.object(tools.shutil, "which", return_value="resolved/signtool.exe") as which:
                self.assertEqual(tools.resolve_signtool(), "resolved/signtool.exe")
                which.assert_called_once_with(command)

    def test_bad_explicit_tool_is_not_silently_replaced_by_another_sdk(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.sdk_tool(root, "10.0.10.0")
            with patch.dict(os.environ, {"ProgramFiles(x86)": str(root), "SPACEPRUNE_WINDOWS_SIGNTOOL": "missing.exe"}, clear=True), \
                 patch.object(tools.shutil, "which", return_value=None):
                with self.assertRaisesRegex(tools.BuildToolError, "SPACEPRUNE_WINDOWS_SIGNTOOL.*missing.exe"):
                    tools.resolve_signtool()

    def test_missing_tools_stop_before_compilation_and_preserve_existing_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "previous"
            output.mkdir()
            previous = output / "SpacePruneNative.msi"
            previous.write_bytes(b"previous accepted MSI")
            for certificate, expected in [("a" * 40, "Windows SDK signing tool"), (None, "WiX executable")]:
                with patch.dict(os.environ, {}, clear=True), \
                     patch.object(tools.shutil, "which", return_value=None), \
                     patch.object(builder.platform, "system", return_value="Windows"), \
                     patch.object(builder.subprocess, "run") as run:
                    with self.assertRaisesRegex(tools.BuildToolError, expected):
                        builder.build(Path(temporary), "x86_64-pc-windows-msvc", "debug", output,
                                      certificate, "https://timestamp.example.test", "wix")
                    run.assert_not_called()
                    self.assertEqual(previous.read_bytes(), b"previous accepted MSI")

    def test_build_command_reports_missing_sdk_without_a_traceback(self):
        script = Path(__file__).resolve().parents[1] / "windows-native-package.py"
        with tempfile.TemporaryDirectory() as temporary, \
             patch.dict(os.environ, {}, clear=True), \
             patch.object(tools.shutil, "which", return_value=None), \
             patch.object(builder.platform, "system", return_value="Windows"), \
             patch.object(builder.subprocess, "run") as run, \
             patch.object(sys, "argv", [str(script), "build", "--target", "x86_64-pc-windows-msvc",
                                       "--output", temporary, "--certificate-sha1", "a" * 40]):
            error = io.StringIO()
            with contextlib.redirect_stderr(error), self.assertRaises(SystemExit) as stopped:
                runpy.run_path(str(script), run_name="__main__")
            self.assertEqual(stopped.exception.code, 2)
            self.assertIn("Windows SDK signing tool signtool.exe was not found", error.getvalue())
            self.assertNotIn("Traceback", error.getvalue())
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
