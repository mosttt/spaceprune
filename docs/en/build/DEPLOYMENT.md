# Deploying Core and CLI

Core and CLI are open source, with first-party code under the [MIT License](../../../LICENSE). This page explains installing and upgrading a built CLI and using personal configuration. See [Building, packaging, and distribution](BUILDING.md) for toolchains, debug/release builds, six targets, and archive creation; see [CI and releases](CI.md) for GitHub publication.

Offline CLI use needs no account, model service or database.

## 1. Install a built CLI

Choose an archive matching the destination operating system and CPU architecture, verify the publisher's SHA-256, then extract it. The executable is `spaceprune` on Linux/macOS and `spaceprune.exe` on Windows. Run it from the extracted directory or add the complete runtime directory to the target user's `PATH`. Do not copy only CLI: Linux needs its three adjacent helpers, Windows needs `SpacePruneNative.msi`, and macOS needs the complete `SpacePruneAuthorization.app`. For source installation, see [Install CLI locally](BUILDING.md#4-install-cli-locally).

Run `spaceprune --version` and `spaceprune --help` to confirm the installed version, then perform a read-only `spaceprune scan --no-progress`. Results reflect the target user's configuration and permissions; an administrator account's results do not replace standard-user acceptance.

The official workflow applies Authenticode to Windows payloads and signs/notarizes macOS payloads before creating archives and checksums. CLI has no graphical installer; the Windows MSI installs only native authorization components. Signing success is established by the release record and actual files. Validate read-only scanning and terminal interaction on each target platform; deletion tests must use dedicated test data.

## 2. Keep binaries separate from personal configuration

Release archives exclude personal settings and recipes. CLI discovers application `config.toml` for the platform, then loads `recipes/<ID>/<ID>.toml`. See [Default configuration discovery](../usage/CONFIGURATION.md#2-default-configuration-discovery) for locations, XDG priority, Windows/macOS paths, and complete fields.

A typical configuration directory contains:

```text
spaceprune/
├── config.toml
└── recipes/
    └── personal/
        ├── personal.toml
        └── variables.toml
```

`config.toml` stores the active recipe, the `language` preference shared by Desktop and CLI, and the independent CLI theme. Recipes, variable mappings, and includes live in each recipe's directory; Desktop stores its appearance separately. Updating the CLI binary does not automatically rewrite these personal files.

Use `--config <PATH>` for independent test configuration rather than overwriting real user settings. After copying a recipe from another device, check path mappings and run a read-only scan on the destination machine. The same configuration need not match the same content on another computer.

## 3. Runtime tools and permissions

Ordinary file cleanup uses the current user's permissions. Snapper and Linux system handlers require their corresponding tools on the destination system. See [Built-in handlers and external plugins](../usage/CLI.md#built-in-handlers-and-external-plugins) for optional tools, handlers, and parameters. If a tool is missing or permission is denied, inspect the returned result; unread content must not be treated as successfully checked or cleaned.

Snapper deletion acts on explicitly selected snapshots through system authorization. Compiling CLI does not grant permission in advance. Linux authorization components are built by Core and share a system installation location between CLI and Desktop. Keep `spaceprune-readonly-helper`, `spaceprune-cleanup-helper`, and `spaceprune-helper-installer` adjacent to, and the same version as, `spaceprune`. Move them together.

Use `spaceprune authorization status` to inspect readiness, or `spaceprune authorization install` to explicitly install/update the system copy. Interactive operations prepare missing, outdated, or different bundled components when permission is actually needed; background tasks do not open installation prompts. See [Linux system authorization components](../usage/CLI.md#linux-system-authorization-components).

The `authorization status/install` management commands support Linux only. Windows foreground operations install and verify the pinned bundled MSI; the signed macOS authorization app registers/updates fixed services. Installation approval does not grant read or deletion approval. See [Windows packaging](WINDOWS-PACKAGING.md) and [macOS packaging](MACOS-PACKAGING.md).

macOS now checks parent-directory generations and retained vnode events, stopping deletion and using recovery when ancestor movement or observation failure is detected. Native APFS/HFS acceptance and APFS in-flight rename synchronization evidence remain outstanding. Use isolated generated data for cleanup tests; successful signing/installation does not establish deletion-boundary correctness. See [implementation and native acceptance](../reference/MACOS-ANCESTRY.md). macOS also lacks a complete user-callable native-service uninstall entry point. Deleting CLI or the authorization app does not unregister system services.

External plugins are separate runtime extensions, not automatically included by compiling CLI. See [Plugin protocol](../reference/PLUGIN-PROTOCOL.md) for installation, permissions, and protocol requirements.

## 4. Automation and upgrades

Automation uses Linux systemd user services, macOS LaunchAgent, or Windows current-user scheduled tasks. Before installation, complete a manual read-only scan as the target user with the actual application `config.toml`, then review selected modules, schedule, and enabled state. See [Automation](../usage/CLI.md#automation).

Installed tasks record the CLI runner and application configuration paths. Reinstall affected automation after moving or renaming the binary; do not leave schedules pointing at temporary build directories or deleted versions. Automatic cleanup executes recommended rules on the installed schedule without asking for manual confirmation each time.

Before upgrading, retain the complete active release directory and a backup of personal configuration. Wait for current scans or cleanup to finish before replacing CLI and its companion components as a set. Recheck the version, load existing configuration, and run a read-only scan afterward. Rolling back the program does not undo completed deletions, nor guarantee that an older version understands every rule saved by a newer version.

## 5. Distribution materials

When distributing Core/CLI binaries, retain this repository's MIT License, supplied Rust dependency and standard-library notices. musl archives also need the notice for the musl actually linked. See [Licenses, signatures, and official releases](BUILDING.md#6-licenses-signatures-and-official-releases) for generation and archive manifests.

Applications embedding Core must maintain license materials for their own dependencies and artifacts.
