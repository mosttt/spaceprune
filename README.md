# SpacePrune

[简体中文](README.zh-CN.md)

SpacePrune is an open-source, recipe-driven cleanup CLI with reusable Rust Core libraries and plugins. Define what may be cleaned in TOML, inspect a read-only scan, then choose and confirm what to remove. Reuse the same recipe as new files appear.

Local scanning and cleanup work offline, without an account or an AI service. First-party code in this repository is licensed under MIT.

## Quick start

Requires Rust 1.97. Build the CLI and its three authorization components from the repository root:

```bash
cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper
```

Start with a scan; scanning does not delete files:

```bash
./target/release/spaceprune scan
```

Open the interactive list to choose what to clean:

```bash
./target/release/spaceprune --select
```

Or clean the recommended items after confirming the prompt:

```bash
./target/release/spaceprune clean
```

Without a personal recipe, the built-in basic recipe checks the user cache directory on Linux/macOS and recommends direct children not modified for 30 days. On Windows, it checks the user temporary directory and recommends direct children not modified for 7 days. A directory's modification age is based on its most recently modified content. Other cleanup capabilities must be explicitly configured in a recipe.

For repeated Linux source development, `./scripts/run-cli.sh` builds the CLI and matching components before launch. Windows/macOS native authorization also requires packaging and signing; see [Building and packaging](docs/en/build/BUILDING.md) for prerequisites, targets and distribution commands.

## Features

- **Inspect before cleanup:** browse modules, folders and files; review sizes, diagnostics and recommendation reasons; select groups or individual items.
- **Reusable recipes:** configure scope, exclusions and age/size conditions in TOML, with includes, platform conditions and variable mappings.
- **Terminal and scripts:** interactive selection, stable `scan`/`clean` commands, JSON output, cancellation, English/Chinese language preferences and four terminal themes.
- **Built-in plugins:** generic filesystem rules, Snapper snapshots, Linux system resources, and developer-tool caches/Docker. Platform support varies by handler; external process plugins can extend the catalog.
- **Scheduled cleanup:** run configured recommendation rules through Linux systemd user services, macOS LaunchAgent or Windows Task Scheduler.
- **Embeddable Core:** integrate through `spaceprune::api`, or consume the pure data contracts without a terminal interface.

Saving a recipe does not delete files. Manual cleanup requires confirmation; enabled schedules execute the configured recommended rules without a new prompt each time. Logical sizes and actual reclaimed space are reported separately where shared storage prevents a direct comparison.

## Source layout

- `crates/`: the CLI, Core libraries, data contracts and native authorization components.
- `plugins/`: built-in plugins grouped under `cross-platform/`, `linux/`, `windows/` and `android/`.
- `recipes/` and `mappings/`: bundled recipes and platform variable mappings.
- `scripts/` and `.github/workflows/`: build, packaging and release tooling.
- `docs/en/` and `docs/zh/`: usage, architecture, build and contribution guides.
- `licenses/`: third-party notices and distribution materials.

## Documentation

Start with the [documentation index](docs/en/README.md), or go directly to:

- [CLI commands and terminal controls](docs/en/usage/CLI.md)
- [Recipe authoring](docs/en/usage/RECIPE-GUIDE.md) and [configuration reference](docs/en/usage/CONFIGURATION.md)
- [Architecture](docs/en/reference/ARCHITECTURE.md) and [plugin protocol](docs/en/reference/PLUGIN-PROTOCOL.md)
- [Building and packaging](docs/en/build/BUILDING.md), [installation and automation](docs/en/build/DEPLOYMENT.md), and [CI/releases](docs/en/build/CI.md)
- [Development](docs/en/build/DEVELOPMENT.md), [feature/platform scope](docs/en/reference/FEATURE-MATRIX.md), and [validation status](docs/en/development/VALIDATION.md)

The [contribution guide](docs/zh/community/CONTRIBUTING.md) and [code of conduct](docs/zh/community/CODE_OF_CONDUCT.md) are currently available in Simplified Chinese.

## License

First-party source uses the [MIT License](LICENSE). Third-party dependencies retain their own licenses; see [license materials](licenses/README.md) and [distribution requirements](docs/en/build/DEPLOYMENT.md).
