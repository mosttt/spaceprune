# Current state and remaining work

Scheduled invocations pin their recipe ID with `--recipe`; Windows Task Scheduler, macOS launchd and Linux systemd use the same argument contract. A per-invocation CLI override leaves shared defaults unchanged. Confirmed sharing violations complete automation normally while retained items stay separate from successful deletion counts.

Docker image scanning uses native `UniqueSize` for an exclusive-storage estimate. Build cache uses its size only when explicitly marked unshared; stopped containers use writable-layer size. Missing measurements remain unknown and known zero remains zero. Referenced content and release estimates remain separate. Partial receipts expose processed and unprocessed estimates, with file/subfolder wording. Seventeen Docker and three receipt projection tests passed on Windows.

Core/CLI builds independently under MIT. Start with [development](../build/DEVELOPMENT.md) and [validation](VALIDATION.md) for current checks and acceptance gates.

## Integrated capabilities

Filesystem handles generic file rules. Snapper owns snapshot configuration rules, protection and measurement; the Linux system plugin owns package/systemd lifecycles; developer tools own Go/Cargo/npm and Docker. Plugins provide domain discovery, measurement and consequences instead of hosts guessing from directory names. Go/Cargo/npm caches currently support Linux only. Docker supports Linux/Windows/macOS desktop, not mobile. Probe native interfaces first: skip missing tools and report installed-but-unavailable tools. Docker never deletes volumes or running containers; execution binds the endpoint, Engine and exact object IDs. An object becoming ineligible is not evidence of deletion. See [Docker](../usage/DOCKER.md).

Core hosts can reuse recipe settings, language preferences and native permissions. Start scans as the current user, prepare matching bundled components when needed and continue the original operation. Read and deletion authorization remain separate; reuse only valid scoped connections. Do not hide permission causes or loop prompts. Plain Windows Cargo output lacks the complete signed package; use [free local development signing](../build/WINDOWS-PACKAGING.md#free-local-development-certificate). Missing components report cleanup not started, not an unconfirmed running worker.

## Remaining native acceptance

Windows signed installation, UAC, cross-account and NTFS/junction combinations remain unverified natively. macOS implements parent generations plus retained vnode invalidations; APFS/HFS execution, TCC, signed services, cancellation and automation remain target-device work. APFS in-flight rename/attribute-read synchronization and the complete counter-mutation domain remain open: [implementation](../reference/MACOS-ANCESTRY.md), [source evidence](../reference/MACOS-ANCESTRY-EVIDENCE.md). Darwin compilation is not that evidence.

Android/iOS currently provide only some lower-level library targets, not an accepted mobile application.
