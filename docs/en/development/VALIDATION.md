# Core/CLI validation status

Separate existing evidence from native checks still required. Document cleanup does not change implementation or turn older tests into acceptance of a new installer. Select affected tests using [development](../build/DEVELOPMENT.md), without repeating unrelated suites.

## Existing evidence

Configuration and real generated-directory fixtures cover multi-file sources, save/reload, recurring matches for new contents, exact selection, exclusions, single-use plans and partial receipts. Failures do not replay deletion; moved-directory restoration never overwrites new contents. Linux native authorization, login revocation, scheduling and Btrfs Snapper authentication/protection/free-space observations have real-environment evidence for those configurations, not every filesystem/platform combination.

The latest Windows-feedback fix passed one filesystem authorization-reconciliation test and ten CLI cleanup-output tests. Eighteen shared permission failures are explained once with at most five locations in human output, while JSON retains all failures. Component-not-started, unconfirmed execution and already-applied partial effects remain distinct.

Fourteen Docker plugin tests and the Core configuration→probe→scan→confirm→exact-apply fixtures passed. The real local Engine was only probed/scanned read-only; no real Docker cleanup was performed. Linux Go/Cargo/npm fixtures cover independent enablement, discovery, reusable saved rules and receipts. Cross-compilation establishes compilation only.

## Measurement and display

Keep ordinary logical size separate from snapshot-volume free-space changes. Snapper `#0 current` is Current system/Not applicable; no historical snapshots means none can be cleaned. Only actual measurement failures are unknown, protected objects remain visible but disabled, and snapshots do not inflate unknown ordinary-file counts. Shared blocks, concurrent disk activity and Docker shared layers cannot be summed as actual reclaimed bytes.

## Next native checks

Use [Windows packaging](../build/WINDOWS-PACKAGING.md) for trusted installation, automatic component update, UAC accept/deny/cancel, cross-account, links and busy targets. Use [macOS packaging](../build/MACOS-PACKAGING.md) and [ancestor tests](../reference/MACOS-ANCESTRY.md#run-the-native-cases-on-macos) for signing, permissions and APFS/HFS; attribute synchronization and full invalidation coverage also need evidence. Test Docker on disposable Windows/macOS resources without unscoped prune. Mobile library builds do not establish native application acceptance.
