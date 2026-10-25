"""Short-lived CI signing credentials. Passwords and private keys never enter logs or metadata."""

from __future__ import annotations

import base64
import json
import os
import secrets
import shlex
import shutil
import subprocess
import tempfile
from pathlib import Path


def security(*arguments: str) -> str:
    result = subprocess.run(["security", *arguments], capture_output=True, text=True)
    if result.returncode:
        # CalledProcessError would include password-bearing argv in its default message.
        raise RuntimeError(f"security {arguments[0]} failed (exit {result.returncode})")
    return result.stdout


def create_keychain(metadata: Path) -> Path:
    if metadata.exists():
        raise ValueError("Signing metadata already exists; clean up the previous keychain first")
    certificate = os.environ.get("APPLE_CERTIFICATE")
    certificate_password = os.environ.get("APPLE_CERTIFICATE_PASSWORD")
    if not certificate or certificate_password is None:
        raise ValueError("APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD are required for signing CI")
    directory = Path(tempfile.mkdtemp(prefix="spaceprune-signing-", dir=os.environ.get("RUNNER_TEMP")))
    keychain = directory / "build.keychain-db"
    password = secrets.token_urlsafe(32)
    try:
        metadata.parent.mkdir(parents=True, exist_ok=True)
        metadata.write_text(json.dumps({"generator": "spaceprune-signing-keychain", "path": str(keychain), "directory": str(directory)}) + "\n")
        metadata.chmod(0o600)
        # Record ownership before native calls so CI's always-cleanup can recover a killed import.
        security("create-keychain", "-p", password, str(keychain))
        security("set-keychain-settings", "-lut", "3600", str(keychain))
        security("unlock-keychain", "-p", password, str(keychain))
        certificate_file = directory / "certificate.p12"
        certificate_file.write_bytes(base64.b64decode(certificate, validate=True))
        certificate_file.chmod(0o600)
        try:
            security("import", str(certificate_file), "-k", str(keychain), "-P", certificate_password, "-T", "/usr/bin/codesign")
        finally:
            certificate_file.unlink(missing_ok=True)
        security("set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, str(keychain))
        return keychain
    except BaseException:
        if metadata.exists():
            delete_keychain(metadata)
        else:
            shutil.rmtree(directory)
        raise


def delete_keychain(metadata: Path) -> None:
    if not metadata.exists():
        return
    value = json.loads(metadata.read_text())
    directory = Path(value["directory"])
    keychain = Path(value["path"])
    if value.get("generator") != "spaceprune-signing-keychain" or not directory.name.startswith("spaceprune-signing-") or keychain != directory / "build.keychain-db":
        raise ValueError("Refusing to remove an unrelated keychain")
    if keychain.exists():
        security("delete-keychain", str(keychain))
    # Preserve any keychains another concurrent job added after this one was created.
    current = shlex.split(security("list-keychains", "-d", "user"))
    remaining = [path for path in current if Path(path).resolve() != keychain.resolve()]
    if current != remaining:
        security("list-keychains", "-d", "user", "-s", *remaining)
    shutil.rmtree(directory, ignore_errors=True)
    metadata.unlink()


def notarize(archive: Path) -> None:
    key_id = os.environ.get("APPLE_API_KEY_ID")
    issuer = os.environ.get("APPLE_API_ISSUER")
    private_key = os.environ.get("APPLE_API_PRIVATE_KEY")
    if not key_id or not issuer or not private_key:
        raise ValueError("Notarization requires APPLE_API_KEY_ID, APPLE_API_ISSUER and APPLE_API_PRIVATE_KEY")
    with tempfile.TemporaryDirectory(prefix="spaceprune-notary-", dir=os.environ.get("RUNNER_TEMP")) as temporary:
        key = Path(temporary) / "AuthKey.p8"
        key.write_text(private_key)
        key.chmod(0o600)
        result = subprocess.run(["xcrun", "notarytool", "submit", str(archive), "--key", str(key),
            "--key-id", key_id, "--issuer", issuer, "--wait", "--output-format", "json"], capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError(f"notarytool failed (exit {result.returncode}); credentials and raw output are not logged")
        response = json.loads(result.stdout)
        if response.get("status") != "Accepted":
            raise RuntimeError(f"Notarization was not accepted; inspect submission {response.get('id', 'unknown')}")
        print(f"Notarization accepted: {response['id']}")


def notarize_app(app: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="spaceprune-app-notary-", dir=os.environ.get("RUNNER_TEMP")) as temporary:
        archive = Path(temporary) / "application.zip"
        subprocess.run(["ditto", "-c", "-k", "--keepParent", str(app), str(archive)], check=True)
        notarize(archive)
        subprocess.run(["xcrun", "stapler", "staple", str(app)], check=True)
        subprocess.run(["xcrun", "stapler", "validate", str(app)], check=True)
