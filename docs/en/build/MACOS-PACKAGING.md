# Packaging macOS native authorization components

Ordinary `cargo build` produces development programs only. A distribution supporting administrator file capabilities requires a host, authorization app, metadata service, and cleanup service from the same Apple Team. The build script prepares and signs artifacts; it does not install system services. When a foreground operation needs permission, the bundled authorization app uses ServiceManagement to register or update the fixed components automatically.

macOS ancestor-movement protection is integrated into ordinary execution and the privileged cleanup helper. After capturing a selection and before deleting it, the guard checks parent-directory generations and retained invalidation events. An unsupported or zero generation returns an explicit error and stops deletion. Linux regression tests and checks for both Darwin architectures passed; native APFS/HFS acceptance remains open. At this stage, use only isolated generated data for cleanup tests on Mac. Follow the [native implementation checks](../reference/MACOS-ANCESTRY.md) for movement, recovery, exchange and permission behavior; the [ancestor-move assessment](../reference/MACOS-ANCESTRY-EVIDENCE.md) retains the APFS synchronization and event-coverage questions.

## CLI distribution layout

Keep the complete `SpacePruneAuthorization.app` beside the CLI executable; do not copy only `spaceprune`. The authorization app contains the fixed `Contents/MacOS/spaceprune-helper-installer`, two services under `Contents/Library/LaunchServices/`, and declarations under `Contents/Library/LaunchDaemons/`. Info/launchd plists needed by older systems are also embedded in each service Mach-O's `__TEXT` segment.

Run from a complete Core Git checkout on macOS with the SDK, Python 3.9+, and a valid Developer ID Application identity. This example uses Apple Silicon; for Intel, use `x86_64-apple-darwin` and minimum OS `10.13`:

```bash
export SPACEPRUNE_MACOS_TEAM_ID='YOURTEAMID'
export SPACEPRUNE_MACOS_SIGNING_IDENTITY='Developer ID Application: Your Organization (YOURTEAMID)'
export SPACEPRUNE_MACOS_DISTRIBUTION=cli
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo build --locked --release -p spaceprune-cli --target aarch64-apple-darwin
python3 scripts/macos-native-package.py build --distribution cli \
  --target aarch64-apple-darwin \
  --output target/aarch64-apple-darwin/release/SpacePruneAuthorization.app
python3 scripts/macos-native-package.py sign-cli --distribution cli \
  --output target/aarch64-apple-darwin/release/spaceprune
```

The Team ID must be the actual publisher's 10-character identifier. Signed builds require tracked Core files to match HEAD. `--unsigned` is only for checking development artifact layouts and cannot obtain native authorization. The `prepare` subcommand can generate plists on other platforms; it needs no Apple credentials and does not establish that Mach-O linking or system installation works.

The CLI authorization app ID is `com.spaceprune.authorization.cli`; its services are `com.spaceprune.metadata-v6-cli` and `com.spaceprune.cleanup-v4-cli`. The separate desktop host uses the `desktop` distribution identity and does not replace CLI services. Identities, protocols, paths, and signing requirements come from the `spaceprune-access` packaging descriptor rather than duplicate definitions in scripts.

## Signing, notarization, and versioning

Sign explicitly in this order: services → authorization app → host. Each program uses its role-specific identifier and `runtime,hard,kill,library` policy. `--deep` is used only for final verification, not recursive signing. Native minimum versions remain Intel 10.13 and Apple Silicon 11.0.

`CFBundleVersion` defaults to the commit count in the full Core history. Shallow clones must fetch the missing history or explicitly provide a monotonically increasing `--build-version major.minor.patch`; releases should advance along one version line. Code identity checks compare actual CodeDirectory hashes, not version-number equality.

The release workflow integrates signing, notarization, and credential cleanup for both macOS architectures. Repository variables are `SPACEPRUNE_MACOS_TEAM_ID` and `SPACEPRUNE_MACOS_SIGNING_IDENTITY`; secrets are `APPLE_CERTIFICATE` (base64 P12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, and `APPLE_API_PRIVATE_KEY` (P8 contents). Do not commit secrets. CI creates an isolated temporary keychain and cleans it up on success or failure. Import failures do not print password-bearing commands or raw output.

Submit the complete temporary CLI ZIP for notarization. After Accepted, staple the authorization app inside it, then create the final tar.gz. The CLI executable itself does not support stapling; Gatekeeper retrieves its notarization ticket using the submitted code identity. Missing credentials, rejected notarization, or mismatched signatures fail the macOS release build rather than producing an unsigned release.

### Create an archive locally

After the signed Apple Silicon build above, follow the workflow's packaging order. Replace `release_target` for Intel. Supply `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, and `APPLE_API_PRIVATE_KEY` to the current build process through your existing credential manager; do not put P8 contents in shell history or the repository. An existing local signing identity does not need the CI-specific P12 import step.

```bash
set -euo pipefail
release_target=aarch64-apple-darwin
package_id="$(cargo pkgid --locked -p spaceprune-cli)"
package_name="spaceprune-v${package_id##*#}-${release_target}"
package_dir="dist/${package_name}"
test ! -e "$package_dir"
mkdir -p "$package_dir/docs"
cp "target/${release_target}/release/spaceprune" "$package_dir/spaceprune"
ditto "target/${release_target}/release/SpacePruneAuthorization.app" "$package_dir/SpacePruneAuthorization.app"
cp README.md LICENSE \
  licenses/rust/THIRD-PARTY-LICENSES-RUST.txt \
  licenses/rust/THIRD-PARTY-LICENSES-RUST-STDLIB.html \
  "$package_dir/"
cp -R docs/. "$package_dir/docs/"
ditto -c -k --keepParent "$package_dir" "dist/${package_name}.notary.zip"
python3 scripts/macos-ci.py notarize "dist/${package_name}.notary.zip"
xcrun stapler staple "$package_dir/SpacePruneAuthorization.app"
xcrun stapler validate "$package_dir/SpacePruneAuthorization.app"
tar -C dist -czf "dist/${package_name}.tar.gz" "$package_name"
(cd dist && shasum -a 256 "${package_name}.tar.gz" > "${package_name}.tar.gz.sha256")
```

The script retains staging and the notarization ZIP for inspection; it does not install components. If using `CARGO_TARGET_DIR`, adjust both input paths. Distribute the final `.tar.gz` and checksum file, keeping CLI adjacent to the complete authorization app.

## On-demand updates and acceptance limits

Before connecting, verify the bundled components and actual services for publisher signature, role, and code identity. When a trusted registered file or actual service is outdated, a foreground operation refreshes it once and fully verifies the new connection. Older systems can identify an update from installed files even when the daemon cannot start. macOS 13+ uses SMAppService unregister/register; older systems use natively authorized SMJobRemove/SMJobBless. The latter receives a single-element argument list from the OS, so services use a fixed-filename, no-argument entry point. Cancellation, refusal, or pending system approval stops file operations; installation is not retried in a loop.

Administrator authorization and Login Items approval remain controlled by macOS. Applying updates automatically does not bypass them, and read-only permission does not mean deletion was confirmed. If an update interrupts work, preserve receipts and unconfirmed state rather than automatically replaying cleanup.

`spaceprune authorization status/install` currently supports Linux only. macOS has neither equivalent manual commands nor a complete user-callable native-service uninstall entry point. Deleting CLI or the authorization app is not ServiceManagement unregistration and must not be recorded as successful component removal. Test unregistration/removal in a recoverable environment and retain actual registration states and results.

Current evidence includes portable state-machine, subprocess, and packaging tests, plus checks for both Darwin architectures. It does not include real Apple-signed installation evidence. Signing, notarization, old-version upgrades, TCC/revocation/cancellation, cross-account behavior, and APFS require tests on the target system. CLI Terminal mode integrates system `security` text authentication, credential revalidation, and terminal-control handoff (authorization agent v6). GUI interaction, portable PTYs, and cross-checks do not replace actual macOS text-terminal acceptance; see [implementation and validation records](../../zh/build/MACOS-PACKAGING.md).
