#!/usr/bin/env python3
"""Provision a temporary signing keychain or notarize an explicitly selected build artifact."""

import argparse
import os
import sys
from pathlib import Path
from macos_package.provisioning import create_keychain, delete_keychain, notarize


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["keychain-create", "keychain-delete", "notarize"])
    parser.add_argument("path", type=Path)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("These actions require macOS native tools")
    if args.action == "keychain-create":
        keychain = create_keychain(args.path)
        if os.environ.get("GITHUB_ENV"):
            with open(os.environ["GITHUB_ENV"], "a") as environment:
                environment.write(f"SPACEPRUNE_MACOS_KEYCHAIN={keychain}\n")
        print("Temporary signing keychain prepared")
    elif args.action == "keychain-delete":
        delete_keychain(args.path)
    else:
        notarize(args.path)


if __name__ == "__main__":
    main()
