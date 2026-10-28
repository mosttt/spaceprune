# Developing Core and CLI

Windows cleanup streams confirmed plans in bounded named-pipe chunks, including plans larger than the transport buffer. Runtime `spaceprune-readonly-*` and `spaceprune-cleanup-*` directories are reserved: scanning skips them and recursive cleanup preserves them, including directories created after confirmation.

This independent Rust workspace provides MIT-licensed Core libraries, CLI and built-in plugins. Development requires only this repository and public dependencies.

## 1. Source and toolchain

```text
spaceprune/
├── crates/
│   ├── spaceprune/                 Core application API
│   ├── spaceprune-cli/             CLI and terminal UI
│   ├── spaceprune-config/          Configuration loading and storage
│   ├── spaceprune-engine/          Scan, planning and execution
│   ├── spaceprune-plugin-api/      Handler contract
│   ├── spaceprune-plugin-host/     External plugin host
│   └── spaceprune-platform*/      Platform services
├── plugins/                       Built-in filesystem and Snapper handlers
├── recipes/                       Embedded recipe resources
└── mappings/                      Platform variable mappings
```

[rust-toolchain.toml](../../../rust-toolchain.toml) declares the toolchain, currently Rust 1.97.0; `Cargo.lock` fixes dependency versions. See [Building, packaging, and distribution](BUILDING.md) for platform prerequisites, debug/release builds, target directories, and installation.

## 2. Core API and CLI responsibilities

Other Rust applications should depend on the `crates/spaceprune` library and use `SpacePrune`, `ScanRequest`, `ConfigStore`, and structured reports through `spaceprune::api`. The CLI crate owns Clap, terminal I/O, and TUI; it is not the reusable API for other Rust hosts.

Preserve the complete flow: read configuration → scan → build a plan → execute after confirmation. Read-only scans do not delete files; cancelled or incomplete scans cannot serve directly as complete cleanup evidence. Recipes define scope and recommendation rules. Core validates and executes them; it does not plan AI exploration. See [Architecture](../reference/ARCHITECTURE.md), [Events](../reference/EVENTS.md), and [Plugin protocol](../reference/PLUGIN-PROTOCOL.md).

### Distributing the Linux Go-cache worker

Library hosts continue to use `spaceprune::api`, not the CLI. For `builtin.developer-tools/go-modcache` cleanup (or the original `builtin.system-linux/go-modcache` alias), the recommended distribution includes `spaceprune-cleanup-helper`, built from the same Core source, beside the host executable. Preserve that basename and executable permissions; do not use a symlink. Core discovers and probes it automatically, then starts it as the current user. Ordinary read-only Go modules do not require system-component installation first. See [Building](BUILDING.md#core-library-and-mobile-targets) for build commands and the complete authorization component set. CLI already implements the early entry below, so standalone `cargo run` can also handle read-only modules owned by its UID/GID.

A Rust host that controls `main` may instead implement the existing narrow worker entrypoint, avoiding an extra helper for ordinary-user Go cleanup. Call it before creating an async runtime, threads or GUI, or parsing application arguments:

```rust
fn main() {
    if let Some(code) = spaceprune::run_go_cache_worker_if_requested() {
        std::process::exit(code);
    }
    // Initialize the host and use spaceprune::api normally.
}
```

This entry handles only the user's Go-cache worker, never an administrator cleanup session. Without a bundled helper, an integrated early entrypoint or a trusted current installed helper, read-only Go-module cleanup explicitly reports an unavailable component. It does not request administrator access to compensate for a missing executable or fall back to `go clean`. Real capability gaps, such as unavailable user namespaces or unmapped directory GIDs, follow the Recipe's `elevation` policy; `never` requests no authorization. Distributions supporting administrator operations still include matching metadata, cleanup and installer components, installed or updated on demand by the existing authorization flow.

## 3. Validate according to the change

For a local change, start with the affected package or specific tests, for example:

```bash
cargo test --locked -p spaceprune-config
cargo test --locked -p spaceprune-cli
```

After changing shared configuration, Engine, or cross-crate APIs, expand to the complete workspace:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

See [CI and releases](CI.md) for full pre-release checks, dependency audits, and the platform matrix. Passing tests do not replace real terminal interaction or target-platform execution. Do not test irreversible deletion on real user data; create isolated files and a separate recipe.

Android/iOS currently check only several lower-level library targets; no mobile application build is provided. Passing `cargo check` does not establish acceptance of a platform executable, installer, or system authorization feature.

## 4. License materials

Third-party notices and `about.hbs` live in `licenses/rust/`. The tool configuration `about.toml` stays at the root. Run the same checks from the repository root. Contribution guidelines and the code of conduct live in `docs/zh/community/`.

First-party Core/CLI source retains the [MIT License](../../../LICENSE). Cargo dependency notices are maintained with `cargo-about 0.9.2` and `scripts/rust-licenses.sh`; Rust 1.97.0 standard-library materials are checked separately with `scripts/rust-stdlib-licenses.sh`. Review changes after dependency, toolchain, or target changes. See [Distribution materials](BUILDING.md#6-licenses-signatures-and-official-releases) for archive contents and checks.

Maintain distribution obligations against this repository’s actual dependencies and artifacts.

## 5. Embedding in other applications

Integrate through public `spaceprune::api`. A host's interface, services and release process are outside this workspace. Validate affected caller contracts when changing public APIs. Keep dependency paths portable and distribute only this repository's applicable materials, without development-machine absolute paths.

## 6. Documentation and progress

Put user, configuration, architecture, and build documentation under `docs/zh/` or `docs/en/`, grouped into `usage/`, `build/` and `reference/`; plans, decisions, investigations, and validation records belong in `docs/{zh,en}/development/`. Update the relevant guide when commands, configuration schemas, execution boundaries, or packaging change. Code identifiers and new comments use English. Chinese documentation is maintained first during development, with English versions completed afterward.

## Platform compilation boundaries

Native implementations and dependencies are selected by the target OS. Snapper, Linux system cleanup and systemd integration compile only for Linux; Windows elevation only for Windows. Filesystem rules, Docker, configuration, protocols and package descriptors remain shared where applicable. Snapper's native query API is Linux-only. An explicit workspace-wide check may check an empty platform crate on other targets without compiling its implementation or dependencies.

Run `python scripts/check-platform-boundaries.py` to verify the CLI dependency graph for Windows, Linux and macOS. Build scripts select resources with `CARGO_CFG_TARGET_OS`, not the build host. The portable resource compiler remains a host build tool to support cross-compilation; non-Windows artifacts do not embed an MSI.
