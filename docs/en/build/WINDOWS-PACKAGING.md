# Packaging Windows native components

`scripts/windows-native-package.py` builds a separate `SpacePruneNative.msi` containing public metadata/cleanup helpers and license materials. The tool only generates, signs, and verifies artifacts; it neither installs components nor grants read or deletion permission.

Current status: fixed package identity, installed-generation detection, on-demand installation, and portable CLI release wiring are implemented. Native Windows signing, Installer/UAC, cancellation/rollback, and cross-account installation still need validation. Linux cross-checks are not Windows installation acceptance.

## Build

Build from a complete Core Git checkout on Windows with the pinned Rust 1.97.0 MSVC toolchain, Python 3, C++ Build Tools/Windows SDK `signtool`, and a WiX v4-compatible `wix build`. The workflow uses WiX 4.0.6; installing the global tool requires a suitable .NET SDK. The tool supports x64 and ARM64, and MSVC helpers use the static CRT.

Prepare tools and select an existing code-signing identity in the build account:

```powershell
rustup target add x86_64-pc-windows-msvc
dotnet tool install --global wix --version 4.0.6
Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
    Select-Object Subject, Thumbprint, NotAfter, HasPrivateKey
$thumbprint = Read-Host 'Paste the Thumbprint from the certificate list above'
$cert = Get-Item "Cert:\CurrentUser\My\$($thumbprint.Trim())" -ErrorAction Stop
if (-not $cert.HasPrivateKey -or $cert.NotAfter -le (Get-Date)) {
    throw 'Select a valid certificate with HasPrivateKey=True'
}
$env:SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1 = $cert.Thumbprint
[Environment]::SetEnvironmentVariable('SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1', $cert.Thumbprint, 'User')
$signTool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName | Select-Object -Last 1
if (-not $signTool) { throw 'Windows SDK signtool was not found' }
$env:SPACEPRUNE_WINDOWS_SIGNTOOL = $signTool.FullName
python scripts/windows-native-package.py build --target x86_64-pc-windows-msvc --output target/windows-native-package --with-cli
if ($LASTEXITCODE -ne 0) { throw 'Windows package build failed' }
```

Skip WiX installation if 4.0.6 is already installed. The certificate must be valid, have an accessible private key, and have a chain trusted by both build and destination machines. Its fingerprint is an identity selector, not a private key. Without a trusted code-signing identity, ordinary `cargo build` can still produce a development CLI, but cannot establish installation/authorization acceptance for protected directories.

`--with-cli` builds and signs `spaceprune.exe` after the final MSI is signed and the pin is written, then places the EXE, MSI, and pin in the output directory. Omit it when only components are needed. CLI archives must include that EXE and the fixed-name MSI; unpinned adjacent helpers are no longer the official distribution path. Archive these final files using the [build guide](BUILDING.md#5-create-archives-manually), not an intermediate EXE from the Cargo cache.

`SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1` selects the certificate; Windows Certificate Store manages its private key. The tool does not accept PFX passwords. `SPACEPRUNE_WINDOWS_SIGNTOOL` and `SPACEPRUNE_WINDOWS_WIX` can specify tool locations; `--timestamp-url` selects the RFC3161 timestamp service.

Build order: build both helpers → apply and verify SHA-256 Authenticode signatures → compute signed payload identity → generate WiX/MSI → sign and verify MSI → write final package identity. `package-identity.json` binds final MSI bytes, both signed EXEs, license materials, and Core revision. Official signed builds require tracked source to match HEAD. Cross-architecture builds do not execute target programs; `nativeProbesExecuted` reports this accurately.

Development builds can explicitly use `--unsigned`, but those artifacts cannot pass official runtime signature-trust checks. Generation failures preserve completed prior output. Only output directories marked by this tool may be replaced.

## Free local development certificate

Local testing does not require purchasing a commercial certificate. This is Windows Authenticode signing, not the AI Prompt Ed25519 identity. Plain `cargo run -p spaceprune-cli --` does not build or bundle permission components. To test UAC/protected paths, run the signed package output below, not the ordinary Cargo-cache EXE.

After installing the SDK, WiX and .NET above, use Windows PowerShell in an isolated test account/machine to create a local code-signing certificate once and explicitly trust its public certificate in that account. Trust only the development certificate you just created, never an unknown certificate:

```powershell
$cert = New-SelfSignedCertificate -Type CodeSigningCert `
    -Subject 'CN=SpacePrune Local Development' `
    -CertStoreLocation Cert:\CurrentUser\My `
    -KeyAlgorithm RSA -KeyLength 3072 -HashAlgorithm SHA256 `
    -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddMonths(6)
$publicCert = Join-Path $env:TEMP "spaceprune-dev-$($cert.Thumbprint).cer"
Export-Certificate -Cert $cert -FilePath $publicCert | Out-Null
Import-Certificate -FilePath $publicCert -CertStoreLocation Cert:\CurrentUser\Root | Out-Null
Import-Certificate -FilePath $publicCert -CertStoreLocation Cert:\CurrentUser\TrustedPublisher | Out-Null
$env:SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1 = $cert.Thumbprint
[Environment]::SetEnvironmentVariable('SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1', $cert.Thumbprint, 'User')
```

The private key stays in the account's certificate store, not in source, the `.cer` or the installer. The command saves the thumbprint for the current user, so new terminals can reuse it. Do not recreate the identity for updates; use the selector above only when changing certificates. CLI and App use the same thumbprint; the App's normal build entry bundles its native components automatically.

This covers the same test account. A different UAC administrator or destination machine must also explicitly trust the identity; the build never changes other accounts or machine-wide trust automatically. Choose public-release signing separately rather than asking ordinary users to trust a development certificate.

After setting `SPACEPRUNE_WINDOWS_SIGNTOOL` as above, build and run the complete test CLI from Core:

```powershell
python scripts/windows-native-package.py build --target x86_64-pc-windows-msvc --profile debug --output target/windows-native-package --with-cli
if ($LASTEXITCODE -ne 0) { throw 'Windows package build failed' }
.\target\windows-native-package\spaceprune.exe
```

Keep `spaceprune.exe` and `SpacePruneNative.msi` together. Protected operations verify and install components on demand, then request UAC; do not copy helpers into system folders or elevate the entire GUI. This resolves the reported `HelperMissing`, not every refusal: files held by another process or protected by system policy can still fail and must retain their real diagnostics without terminating processes or changing ACLs.

Ordinary accessible-file tests need no certificate: use the regular Cargo CLI or unsigned App `--debug --no-bundle` path. Development packages belong on isolated test machines. After testing and uninstalling the native components, remove only this exact certificate's Root/TrustedPublisher trust using Certificate Manager if no longer needed; do not bulk-delete certificates.


## Generate packaging metadata only

```bash
python3 scripts/windows-native-package.py prepare \
  --target x86_64-pc-windows-msvc --binaries '<helper-directory>' --output '<metadata-directory>'
```

This can run across platforms without executing helpers, WiX, or Windows Installer. Output includes `.wxs` and layout records; the command returns the required `PayloadDir`/`LicenseDir` build definitions. Helpers must use fixed filenames.

## Installation location and trust

The fixed location is system `ProgramFiles64Folder/SpacePruneNative/native-7-5-<payload-digest>/`. The generation digest includes architecture, capability roles, protocols, signed EXEs, and license materials. External inputs cannot choose the installation directory. Generations can coexist without overwriting or terminating an active older generation. The MSI contains no custom actions, service registration, or old-generation removal actions.

Windows metadata v7 / cleanup v5 are login-bound bootstrap and IPC protocols. The read payload remains v6 and the confirmed cleanup grant remains v4; Linux/macOS capability probes are unchanged. Older Windows v6/v4 components fail the new probe, so the existing operation prepares the bundled version and revalidates it without reusing old authorization sessions.

The license directory retains this repository's LICENSE and Rust dependency/standard-library notices. Before packaging, run the license scripts to compare the generated inputs and complete text. Preserve original bytes so Windows line-ending conversion does not change signed package identity. Separate license `.sha256` files are not required.

`SignedFile` represents verified data held through a read-only handle, not permission to execute elevated. Elevatable helpers must still meet Program Files, administrator owner/DACL, and valid signature requirements. Installation additionally binds to the host's compile-time final package digest; an arbitrary validly signed MSI is not accepted.

## Pin package identity in the host

When building Windows CLI or an application host, set `SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY` to the absolute path of the generated `package-identity.json`. `spaceprune-access/build.rs` embeds the bounded file contents in the compiled artifact. Runtime does not read that environment variable or an adjacent manifest. Non-Windows builds ignore it; development builds without it retain the existing protected-adjacent-helper lookup.

Read and cleanup entry points share `windows_native::pinned_helper`, looking only for the exact generation under fixed Program Files. They verify administrator protection, signature, complete payload digest, and protocol in sequence. Malformed pins and architecture mismatches fail explicitly rather than falling back to another adjacent program. Handles remain held until the actual UAC session exits. `pinned_package` likewise verifies the complete post-signing digest of the fixed adjacent MSI, but does not itself install or execute it.

The required order is helper → signed MSI → final pin → host. Helper and layout tools clear inherited host pins so a previous package identity is not embedded in a payload whose digest is being calculated, avoiding a cycle. The public release workflow orchestrates this through `--with-cli`. The Windows runner must already have a signing identity in its certificate store; repository variable `SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1` selects it. A missing identity fails explicitly instead of silently publishing an unsigned package that cannot pass runtime trust checks.

## On-demand installation and cancellation

Successful ordinary access does not start installation. When administrator capabilities are needed and the exact generation is missing/outdated, `windows_native::ensure_helper` prepares it within the original operation. Background tasks return `NeedsAuthorization` without hidden dialogs. Concurrent callers serialize and recheck state; installation failure is remembered within the operation to prevent repeated directory-by-directory prompts.

An unprivileged host child process installs the fixed MSI through Windows Installer, which displays UAC when required. Inherited pipes bind requests and receipts. The child revalidates the parent's creation identity, account SID, AuthenticationId LUID, TokenSessionId, login SID, original deadline, and compile-time package identity. Installation properties are fixed; arbitrary target directories or installer parameters cannot be supplied. The GUI as a whole is not elevated.

Cancellation or parent disconnection closes the control pipe. The Installer progress callback requests cancellation, and the host waits for the system call and worker to truly exit. Killing msiexec is not grounds to claim rollback completed. Windows controls the UAC secure desktop; the application cannot close it for the user and does not report the operation stopped while waiting. Installation uses at most 120 seconds of the original operation's remaining deadline. Cancellation/rollback completion is not permission to resume business work.

The original operation resumes only after installation finishes and components pass signature/digest/ACL/protocol checks again. Installation approval is separate from read or deletion approval; reading and confirmed cleanup still establish separate UAC sessions. This flow needs no manual upgrade guidance and does not terminate cleanup using an older generation.

CLI `authorization status/install` currently supports Linux only; it is not a Windows manual-preparation entry point. Windows uses on-demand installation of the bundled MSI. To remove a test native generation, first ensure no CLI or other host still uses it, then uninstall `SpacePrune Native Components` through its MSI or Windows Installed apps. Do not manually delete generation directories from Program Files.

## Login isolation and cancellation

Temporary mailboxes, request files, and cleanup pipes allow access only to the original login SID, SYSTEM, and Administrators. File ownership remains with the original account SID for configuration and user-directory scope. A standard user entering another administrator's UAC credentials does not redirect the request to the administrator's home. An `OWNER RIGHTS` ACE removes other logins of the same account's implicit ability to rewrite DACLs through ownership. Ordinary persistent configuration retains the original account-private ACL and is not limited to a login's lifecycle.

The helper holds the original host process handle and re-queries its primary token's account, login, AuthenticationId, and session ID during liveness checks. Closing a read-only connection removes the session lease before the temporary directory. Unrelated leftover files preventing directory cleanup cannot allow the old request to continue. If neither lease nor directory can be revoked, close fails and still waits for the worker thread to finish. While a Windows UAC prompt remains active, cancellation/timeout can only prevent subsequent work and wait for native return; it cannot report stopped early.

## Validation scope

Portable tests cover payload changes, retention of older generations, fixed installation layout, prohibited custom actions, final-pin round trips, license mismatches, and preservation of prior output on signing failures. Windows-specific tests cover write/directory-replacement denial while handles are held; these must run natively on Windows.

Actual WiX builds, certificate chains, UAC, Windows Installer cancellation/rollback, NTFS, and cross-account behavior still require Windows acceptance and are not marked passed.
