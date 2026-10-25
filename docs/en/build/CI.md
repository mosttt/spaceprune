# SpacePrune CI and releases

This guide describes the open-source Core/CLI workflows in `.github/workflows`, release targets, and maintenance. This repository’s release workflow builds and publishes Core/CLI only. See [Building, packaging, and distribution](BUILDING.md) for local platform builds and manual archives.

These workflows target GitHub.com and GitHub-hosted runners. They run only when the repository is hosted on GitHub, Actions is enabled, and the tag is pushed to that GitHub repository.

## CI workflow

File: [ci.yml](../../../.github/workflows/ci.yml)

Triggers:

- Push to `main`.
- Pull request.
- Manual `workflow_dispatch`.
- `workflow_call` from the release workflow.
- Weekly dependency security audit, Monday at 03:23 UTC.

Internal feature branches run checks through pull requests, without duplicating the entire workflow on branch pushes.

### Quality

On Ubuntu 24.04, using the project's Rust 1.97.0 toolchain:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --workspace --release --locked
```

The job also uses a pinned actionlint version to check GitHub Actions syntax and expressions, and runs `cargo check` for these mobile library targets:

```text
aarch64-linux-android
aarch64-apple-ios
```

Mobile checks include only `spaceprune-plugin-api`, `spaceprune-config`, `spaceprune-engine`, the filesystem handler, and the platform facade. They do not build Android or iOS executables.

Quality installs `cargo-about 0.9.2` and runs `scripts/rust-licenses.sh check` and `scripts/rust-stdlib-licenses.sh check` to verify the Cargo dependency and Rust standard-library materials required in archives. `python3 -m unittest discover -s scripts/tests` validates portable native packaging/signing lifecycles, not installation on the destination OS.

### Dependency security

A separate security audit job runs `cargo audit --deny warnings`. An earlier formatting, Clippy, test, or build failure does not short-circuit it. The weekly run independently refreshes the RustSec database.

### Native macOS and Windows checks

Non-scheduled runs also execute Clippy and tests on actual GitHub-hosted operating systems. The job named `desktop-platforms` checks the Core/CLI workspace on Linux, Windows and macOS.

| Runner | Purpose |
| --- | --- |
| `macos-15` | Native macOS ARM64 tests |
| `windows-2025` | Native Windows x64 tests |

Weekly runs include only Ubuntu quality and security, avoiding the full desktop matrix solely to refresh RustSec data.

## Release workflow

File: [release.yml](../../../.github/workflows/release.yml)

A pushed `v*` tag triggers this sequence:

1. Call the complete CI workflow.
2. Require Ubuntu quality, the independent security audit, mobile target checks, and native macOS/Windows checks to pass.
3. Require the tag to begin with `v`.
4. Require the remainder to exactly match the `spaceprune-cli` Cargo package version.
5. Build the release matrix.
6. After all builds succeed, create a draft release, upload all archives and checksums, then publish it. Existing releases are not overwritten.

If CI fails, tag validation, `build`, and `publish` are skipped. Release calls the same reusable CI workflow on the tagged commit rather than relying on a historical CI run, keeping release and daily checks in one definition.

For Cargo version `0.1.3`, the correct tag is:

```bash
git tag -a v0.1.3 -m "SpacePrune v0.1.3"
git push origin v0.1.3
```

### Release binaries

| System | Rust target | Format |
| --- | --- | --- |
| Linux x86-64, glibc | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux ARM64, glibc | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| Linux x86-64, static musl | `x86_64-unknown-linux-musl` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` |
| Windows x86-64 | `x86_64-pc-windows-msvc` | `.zip` |

Each archive includes:

```text
spaceprune / spaceprune.exe
README.md
LICENSE
THIRD-PARTY-LICENSES-RUST.txt
THIRD-PARTY-LICENSES-RUST-STDLIB.html
docs/README.md
docs/zh/
docs/en/
```

Platform-specific runtime materials are also required:

- Linux: `spaceprune-readonly-helper`, `spaceprune-cleanup-helper`, and `spaceprune-helper-installer`; musl archives also include `THIRD-PARTY-LICENSES-MUSL.txt` from the actual Ubuntu build environment.
- macOS: the complete signed `SpacePruneAuthorization.app`, stapled after notarization is Accepted and before creating the final tar.gz.
- Windows: signed `SpacePruneNative.msi` and `package-identity.json`; the archived CLI is built and signed after the final MSI is signed and pinned.

The workflow excludes user `config.toml`, personal recipes, credentials, Cargo build caches, and unrelated source.

Release also publishes `SHA256SUMS`. Linux ARM64 uses the Ubuntu 22.04 GNU cross-toolchain; both macOS architectures use their corresponding official runners; Windows uses MSVC. Linux glibc builds use Ubuntu 22.04 to avoid unintentionally raising the runtime glibc baseline.

### Prepare signing before release

Both macOS architectures integrate code signing, notarization, and temporary keychain cleanup. Repository variables are `SPACEPRUNE_MACOS_TEAM_ID` and `SPACEPRUNE_MACOS_SIGNING_IDENTITY`; secrets are `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, and `APPLE_API_PRIVATE_KEY`. See [macOS native packaging](MACOS-PACKAGING.md) for formats, signing order, and local commands.

Windows integrates Authenticode: the workflow installs WiX 4.0.6, locates the Windows SDK's `signtool`, then uses `windows-native-package.py build --with-cli` to build the pinned MSI and CLI. The repository variable `SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1` selects an existing code-signing identity in the runner account's certificate store. The workflow currently has no Windows certificate import step, and the default GitHub-hosted `windows-2025` image does not contain the project's private key. Before enabling official releases, maintainers must provide a usable signing environment and credential integration; setting a fingerprint alone is insufficient. See [Windows native packaging](WINDOWS-PACKAGING.md) for local commands when a certificate is available.

A missing identity, failed signature verification, or notarization result other than Accepted fails that build. It does not fall back to an unsigned release. `SHA256SUMS` covers final archives for integrity verification; no separate archive signature is currently added. Check the actual run logs and artifacts to establish whether a tag has completed signing, notarization, and target-system acceptance.

## Rerunning a release

The first successful run creates a GitHub release with automatically generated release notes. It first creates a draft and uploads assets individually, publishing only after every upload succeeds. An existing release for the tag causes an explicit failure; `--clobber` is not used to replace assets.

If publication fails partway through, cleanup is attempted only when the release created by this run is confirmed to still be a draft. Published releases are not deleted. Correct a published version with a new version and tag, not by rerunning to change its contents. The `publish` job is skipped in forks.

## Permissions and supply chain

- Default workflow permission is `contents: read`.
- Only the final publish job receives `contents: write`.
- Checkout does not retain Git credentials.
- Official GitHub Actions are pinned to full commit SHAs, with comments naming the corresponding major version.
- Release uses the runner's preinstalled GitHub CLI, not a third-party release action.
- actionlint uses a pinned Go module version; downloads are verified through the Go checksum database.
- Cargo builds consistently use `--locked`.
- `rust-toolchain.toml` is the single default Rust toolchain declaration for CI, Release, and local builds; cache keys include its hash.
- cargo-audit is pinned; the RustSec database is refreshed at runtime.
- Artifacts are already compressed, so Actions upload compression is disabled.

When upgrading an action, check its current major version and Node runtime requirements in the official GitHub repository, then update both the full commit SHA and its version comment. Do not change only the comment.

## Local pre-release checks

Install the same helper-tool versions used by the workflow. These commands install checking tools; they do not publish SpacePrune:

```bash
cargo install cargo-about --version 0.9.2 --locked --features cli
cargo install cargo-audit --version 0.22.2 --locked
```

Then run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --workspace --release --locked
cargo audit --deny warnings
scripts/rust-licenses.sh check
scripts/rust-stdlib-licenses.sh check
```

Local checks establish only the targets actually exercised. Keep cross-platform builds, target-machine execution, signatures, and publication records separate. A workflow file or passing local checks do not establish that an official downloadable release exists.
