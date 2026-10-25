# Building, packaging, and distributing Core and CLI

This guide covers the open-source Core and CLI. First-party code uses the [MIT License](../../../LICENSE), and this repository alone is sufficient to build it. Builds require no other product repository.

Run all commands from this repository's root. Development, building a local executable, packaging an archive, and publishing a release are separate operations: `cargo build` does not install the program, sign files, or upload a release.

## 1. Prepare the build environment

[rust-toolchain.toml](../../../rust-toolchain.toml) pins Rust **1.97.0** and declares `rustfmt` and `clippy`. Install rustup, then run:

```bash
rustup toolchain install
rustup show active-toolchain
rustc --version
cargo --version
```

Builds use the repository's `Cargo.lock`; `--locked` prevents automatic dependency updates. The first build downloads the Rust toolchain and Cargo dependencies. Once all inputs are cached, `--frozen` requires an unchanged lockfile and disables network access.

| Build platform | Required system tools | Notes |
| --- | --- | --- |
| Linux | C compiler, linker, libc development files | Usually `build-essential` on Debian/Ubuntu or `base-devel` on Arch. CLI does not require GTK, WebKit, Node.js, or Tauri. |
| Windows x64 | Visual Studio Build Tools with Desktop development with C++, Windows SDK, MSVC linker | Use the MSVC Rust toolchain; start Developer PowerShell if needed. Release target: `x86_64-pc-windows-msvc`. |
| macOS | Xcode Command Line Tools or full Xcode | Install with `xcode-select --install`; check the selected tools with `xcode-select -p`. |

Snapper, `pkexec`, and systemd are **runtime tools** for particular cleanup capabilities, not universal CLI compilation requirements. The recipe and platform determine whether they are needed; ordinary user cache scanning does not require Snapper setup.

## 2. Local debug and release builds

Debug builds suit development; optimized release builds suit actual use and distribution:

```bash
cargo build --locked -p spaceprune-cli -p spaceprune-readonly-helper
cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper
```

| Build | Linux / macOS output | Windows output |
| --- | --- | --- |
| Default debug | `target/debug/spaceprune` | `target/debug/spaceprune.exe` |
| Release | `target/release/spaceprune` | `target/release/spaceprune.exe` |
| Release with `--target <triple>` | `target/<triple>/release/spaceprune` | `target/<triple>/release/spaceprune.exe` |

`CARGO_TARGET_DIR` or `--target-dir` changes the `target` root. Do not confuse default and explicit-target paths: doing so can run an old executable left by a previous build.

For Linux source development, use the shared launcher:

```bash
./scripts/run-cli.sh
./scripts/run-cli.sh scan --no-progress
```

It builds CLI, the read-only component, cleanup component, and installer together, then launches CLI with the arguments unchanged. `cargo run -p spaceprune-cli --` remains supported, but Cargo rebuilds only CLI, not the separate privileged programs. After changing a native handler or authorization protocol, use the complete build command or launcher; do not simply copy old components beside a new CLI.

After a Linux/macOS build, first check entry points that do not delete files:

```bash
./target/release/spaceprune --version
./target/release/spaceprune --help
./target/release/spaceprune scan --no-progress
```

In Windows PowerShell:

```powershell
.\target\release\spaceprune.exe --version
.\target\release\spaceprune.exe --help
.\target\release\spaceprune.exe scan --no-progress
```

For interactive checks, run `spaceprune theme` or `spaceprune --select` in a real terminal. Esc cancels theme selection. The cleanup selector enters the real cleanup flow; test deletion only with a dedicated test directory and recipe. See [CLI reference](../usage/CLI.md) for JSON and redirected output.

## 3. Target platforms and architectures

The current [release workflow](../../../.github/workflows/release.yml) defines six CLI archives:

| Target | Workflow build environment | Archive |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Ubuntu 22.04 x64 | `.tar.gz` |
| `aarch64-unknown-linux-gnu` | Ubuntu 22.04 x64 + GNU ARM64 cross-linker | `.tar.gz` |
| `x86_64-unknown-linux-musl` | Ubuntu 22.04 x64 + musl toolchain | `.tar.gz` |
| `x86_64-apple-darwin` | macOS Intel runner | `.tar.gz` |
| `aarch64-apple-darwin` | macOS Apple Silicon runner | `.tar.gz` |
| `x86_64-pc-windows-msvc` | Windows x64 runner | `.zip` |

Install the target standard library before building. A Rust target supplies Rust target components, not the system SDK, cross-linker, or emulator.

### Linux x64 GNU

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper --target x86_64-unknown-linux-gnu
```

The release workflow builds GNU artifacts on Ubuntu 22.04 to control the glibc baseline. Building the same target on a newer distribution does not automatically provide the same runtime compatibility. Validate on the destination system; `file` and `ldd` can also inspect architecture and shared-library requirements before release.

### Linux ARM64 GNU

Native ARM64 Linux builds use the normal local C toolchain. To cross-build on Debian/Ubuntu x64 as the workflow does, install `gcc-aarch64-linux-gnu`, then run:

```bash
rustup target add aarch64-unknown-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper --target aarch64-unknown-linux-gnu
```

A successful cross-build proves that an ARM64 binary was produced. Startup, scanning, and terminal interaction still need testing on ARM64 Linux.

### Linux x64 musl

Install `musl-tools` on Debian/Ubuntu as in the workflow, then run:

```bash
rustup target add x86_64-unknown-linux-musl
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper --target x86_64-unknown-linux-musl
```

This target produces a static musl build; inspect the actual artifact and its behavior on the destination machine. Include the build environment's musl notice in the archive, rather than copying only the GNU archive's file list.

### macOS Intel and Apple Silicon

A distribution supporting administrator file access also needs the authorization app and services signed by the same identity. See [macOS native packaging](MACOS-PACKAGING.md). Ordinary Cargo builds neither prepare nor install those components.

Run the appropriate group on macOS of the corresponding architecture:

```bash
rustup target add x86_64-apple-darwin
cargo build --release --locked -p spaceprune-cli --target x86_64-apple-darwin
```

```bash
rustup target add aarch64-apple-darwin
cargo build --release --locked -p spaceprune-cli --target aarch64-apple-darwin
```

The workflow produces two separate archives, not a universal binary. CLI remains a command-line program; the complete archive also includes `SpacePruneAuthorization.app`. The workflow signs CLI and the authorization app, then creates the final `.tar.gz` after notarization. It does not produce a DMG. The ordinary Cargo commands above do not perform these packaging steps; follow the macOS guide for a complete distribution.

### Windows x64 MSVC

In PowerShell with the MSVC build environment prepared:

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --locked -p spaceprune-cli --target x86_64-pc-windows-msvc
.\target\x86_64-pc-windows-msvc\release\spaceprune.exe --version
```

The release CLI ZIP contains signed `spaceprune.exe` and `SpacePruneNative.msi`. The workflow uses `windows-native-package.py build --with-cli` to pin the final MSI before compiling the host. Direct Cargo builds remain suitable for development but do not build or install system components. The Windows runner must provide a signing certificate; see [Windows native packaging](WINDOWS-PACKAGING.md).

When permission is needed and the bundled generation is not installed, CLI automatically prepares it through the system Installer, then resumes authorization. Windows GNU cross-compilation is a development check, not a substitute for MSVC artifacts or native installation/UAC acceptance.

### Core library and mobile targets

If you only need the Core API:

```bash
cargo build --locked -p spaceprune --lib
cargo doc --locked -p spaceprune --no-deps
```

Core is a Rust library; it does not produce a separate installable `core` program. Other Rust applications integrate through `spaceprune::api`; see [Development](DEVELOPMENT.md).

For Linux hosts using Go-module-cache cleanup, the recommended package includes a cleanup helper built from the same source:

```bash
cargo build --release --locked -p spaceprune-readonly-helper --bin spaceprune-cleanup-helper
```

Place `target/release/spaceprune-cleanup-helper` beside the final host executable, retaining executable permissions and using a regular file rather than a symlink. With `--target`, use the matching architecture's output directory. Core does not search arbitrary PATH or build directories; copying an `.rlib` does not distribute the worker. App already uses its bundled helper; CLI includes the early worker entrypoint. Other Rust hosts may also implement that early function for ordinary-user Go cleanup, as shown in [Development](DEVELOPMENT.md#distributing-the-linux-go-cache-worker).

The ordinary worker runs as the current user and does not require installation under `/usr/libexec`. For administrator capabilities, use this guide's combined build command and distribute matching `spaceprune-readonly-helper`, `spaceprune-cleanup-helper` and `spaceprune-helper-installer` executables. When permissions are actually needed, the foreground authorization flow checks the bundle and applies missing or old system copies; background operations do not open installation or authorization prompts. Missing or old bundled workers require an updated host distribution, not elevated fallback to the old Go traversal.

CI also runs `cargo check` for several lower-level Android/iOS libraries. There are no corresponding mobile CLI, APK, or IPA releases; see [CI and releases](CI.md) for the exact scope.

## 4. Install CLI locally

Build from the checked-out source and install into Cargo's user installation directory:

```bash
cargo install --path crates/spaceprune-cli --locked --bin spaceprune
spaceprune --version
```

On Linux, to access or clean protected directories, also install the same-version read-only, cleanup, and installer components beside CLI:

```bash
cargo install --path crates/spaceprune-privileged-helper --locked --bins
```

If using a custom `--root`, use the same installation root for CLI and all three components. Foreground operations that need permission automatically apply missing, outdated, or different bundled system components, then resume the operation. Compilation and `cargo install` themselves do not change system copies. To prepare them in advance, run `spaceprune authorization install`; `spaceprune authorization status` only checks and does not request authorization. Background tasks do not open installation prompts.

These two management commands currently support Linux only. Prepare complete Windows/macOS authorization distributions using their platform guides; ordinary `cargo install` does not replace a pinned MSI or signed authorization app.

`cargo install` uses the release profile by default. The usual path is `~/.cargo/bin/spaceprune`, or `%USERPROFILE%\.cargo\bin\spaceprune.exe` on Windows. A custom `CARGO_HOME` or Cargo installation-root setting changes it. Ensure the actual installation directory is in `PATH`.

For an independent installation root:

```bash
cargo install --path crates/spaceprune-cli --locked --bin spaceprune --root ./local-install
./local-install/bin/spaceprune --version
```

On Windows, the relative executable path is `.\local-install\bin\spaceprune.exe`. Add `--force` to `cargo install` only when you intend to replace an existing installation; it replaces the program, not user recipes. These commands use local source and do not imply that the crate has been published to crates.io.

Archive users can run from the extracted directory, or place the complete runtime directory in a location they manage and add it to `PATH`. Keep the three adjacent Linux components, the pinned Windows MSI, or the complete macOS authorization app; do not move just CLI. Keep binaries separate from personal configuration; see [Deployment](DEPLOYMENT.md) for upgrades and automation.

## 5. Create archives manually

First complete the target's release build. Manual archives suit internal delivery or validation; use the repository workflow for official GitHub releases so versions, targets, and manifests do not drift. Archive names follow `spaceprune-v<version>-<target>`.

### Linux `.tar.gz`

This Bash example packages Linux x64 GNU. For other Linux targets, change `release_target` and ensure CLI and all three components have been built. Do not use this script for macOS, whose signed authorization app and notarization steps differ. It reads the Cargo package version and stops if the staging directory exists, avoiding stale files in a new archive.

```bash
set -e
release_target="x86_64-unknown-linux-gnu"
package_id="$(cargo pkgid --locked -p spaceprune-cli)"
release_version="${package_id##*#}"
package_name="spaceprune-v${release_version}-${release_target}"
package_dir="dist/${package_name}"
test ! -e "$package_dir"
mkdir -p "$package_dir/docs"
cp "target/${release_target}/release/spaceprune" "$package_dir/spaceprune"
chmod 755 "$package_dir/spaceprune"
for helper in spaceprune-readonly-helper spaceprune-cleanup-helper spaceprune-helper-installer; do
  cp "target/${release_target}/release/$helper" "$package_dir/"
done
cp README.md LICENSE \
  licenses/rust/THIRD-PARTY-LICENSES-RUST.txt \
  licenses/rust/THIRD-PARTY-LICENSES-RUST-STDLIB.html \
  "$package_dir/"
cp -R docs/. "$package_dir/docs/"
if [[ "$release_target" == *-musl ]]; then
  cp -L /usr/share/doc/musl/copyright "$package_dir/THIRD-PARTY-LICENSES-MUSL.txt"
fi
tar -C dist -czf "dist/${package_name}.tar.gz" "$package_name"
```

The musl notice path matches the Ubuntu release workflow. On another distribution, locate the license for the musl actually used; do not silently omit it. This example retains staging for inspection. Use `tar -tzf dist/<archive>.tar.gz` to inspect the archive structure.

On Linux, create and immediately verify a separate checksum file:

```bash
cd dist
sha256sum "${package_name}.tar.gz" > "${package_name}.tar.gz.sha256"
sha256sum --check "${package_name}.tar.gz.sha256"
```

This changes the working directory to `dist`; return to the repository root before running other repository commands.

### macOS `.tar.gz`

Follow [macOS native packaging](MACOS-PACKAGING.md) to produce CLI and the complete authorization app under the same release identity, then sign, notarize, and archive them. CLI, `SpacePruneAuthorization.app`, and license materials must come from the same build. Merely replacing the target name in the Linux example would omit the authorization app. Generate checksums from the final archive, not from files before signing or stapling.

### Windows `.zip`

First use the `--with-cli` example in [Windows native packaging](WINDOWS-PACKAGING.md) to generate the final signed CLI, MSI, and pin in `target/windows-native-package`, then run from the repository root:

```powershell
$ErrorActionPreference = "Stop"
$releaseTarget = "x86_64-pc-windows-msvc"
$packageId = cargo pkgid --locked -p spaceprune-cli
if ($LASTEXITCODE -ne 0) { throw "Could not read the CLI version" }
$releaseVersion = ($packageId -split '#')[-1]
$packageName = "spaceprune-v$releaseVersion-$releaseTarget"
$packageDir = "dist/$packageName"
if (Test-Path $packageDir) { throw "The staging directory already exists" }
New-Item -ItemType Directory -Force "$packageDir/docs" | Out-Null
Copy-Item target/windows-native-package/spaceprune.exe, target/windows-native-package/SpacePruneNative.msi, target/windows-native-package/package-identity.json "$packageDir/"
Copy-Item README.md, LICENSE, licenses/rust/THIRD-PARTY-LICENSES-RUST.txt, licenses/rust/THIRD-PARTY-LICENSES-RUST-STDLIB.html "$packageDir/"
Copy-Item docs/* -Recurse "$packageDir/docs/"
Compress-Archive -Path $packageDir -DestinationPath "dist/$packageName.zip"
Get-FileHash -Algorithm SHA256 "dist/$packageName.zip"
```

Compare the recipient's SHA-256 with the publisher's value. Checksums verify file integrity; they do not replace publisher signatures. CLI runs directly from the ZIP. The adjacent MSI installs separate native authorization components, not a graphical CLI installer; the host installs that pinned MSI on demand.

## 6. Licenses, signatures, and official releases

CLI archives include the MIT License for first-party Core/CLI code, Cargo dependency notices, Rust standard-library notices. musl builds also include a musl notice. Recheck these materials after dependency or toolchain changes. Existing Linux release checks are:

```bash
scripts/rust-licenses.sh check
scripts/rust-stdlib-licenses.sh check
```

The first requires `cargo-about 0.9.2`; the second requires standard-library copyright files in the current Rust sysroot. `generate` updates repository materials; review changes before committing. See [CI and releases](CI.md) for tool installation and complete CI commands.

The release workflow integrates macOS signing/notarization and Windows Authenticode. Missing identities fail the build rather than falling back to unsigned output. Signing environments, repository variables, and secrets are documented in [CI and releases](CI.md). Final archives also have `SHA256SUMS` for integrity checks, not an independent artifact signature. Compute checksums only after all signing/notarization steps.

Pushing a release tag starts remote builds and publication; it is not an ordinary local validation step. See [CI and releases](CI.md) for tag validation, the six-target matrix, permissions, and immutable assets. These commands have been checked against current source and workflows. A documented command is not evidence that its build, signing, or installation test has run on the target platform.
