# Core and CLI documentation

[简体中文](../zh/README.md) · [Project home](../../README.md)

For an existing binary, start with [CLI usage](usage/CLI.md). To build on Windows, macOS or Linux, use [Building and packaging](build/BUILDING.md). Scan first and confirm the intended scope before cleanup.

## Usage and recipes

- [CLI commands, selection, language and themes](usage/CLI.md)
- [Write reusable recipes and validate them read-only](usage/RECIPE-GUIDE.md)
- [Configuration locations, TOML fields and handler options](usage/CONFIGURATION.md)

## Build, install and maintain

- [Toolchains, platform targets and archives](build/BUILDING.md)
- [Installation, upgrades, personal configuration and automation](build/DEPLOYMENT.md)
- [Local development and public interfaces](build/DEVELOPMENT.md)
- [Windows native component packaging](build/WINDOWS-PACKAGING.md)
- [macOS signing, notarization and authorization components](build/MACOS-PACKAGING.md)
- [GitHub CI and official releases](build/CI.md)

## Interfaces and boundaries

- [Architecture and data flow](reference/ARCHITECTURE.md)
- [Lifecycle events](reference/EVENTS.md)
- [Native permissions and authorization reuse](reference/NATIVE-AUTHORIZATION.md)
- [External plugin protocol](reference/PLUGIN-PROTOCOL.md)
- [Windows system paths](reference/WINDOWS-SYSTEM-PATHS.md)
- [Feature and verification scope](reference/FEATURE-MATRIX.md)

## Contribute

The [contribution guide](../zh/community/CONTRIBUTING.md) and [code of conduct](../zh/community/CODE_OF_CONDUCT.md) are currently available in Simplified Chinese. [Current work](development/CURRENT-WORK.md) lists outstanding tasks. [Validation](development/VALIDATION.md) retains current acceptance boundaries instead of per-fix diaries.
