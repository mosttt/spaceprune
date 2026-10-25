# macOS ancestor-movement protection: implementation and native acceptance

[简体中文](../../zh/reference/MACOS-ANCESTRY.md)

Core checks ancestor movement using parent-directory generations and retained vnode invalidations. Ordinary cleanup and the privileged cleanup helper share the sequence “capture selection → check ancestors → delete.” Movement or observation failure stops deletion and invokes existing NOREPLACE restoration. Unselected ancestors are not moved and need no additional write permission.

Linux regression tests and checks for both Darwin architectures passed; native APFS/HFS tests have not run. Fixed HFS source supports the directory-generation lock relationship. APFS synchronization and its complete counter-mutation domain for ordinary, compound and exchange rename remain open; the evidence is retained in the [primitive assessment](MACOS-ANCESTRY-EVIDENCE.md).

## Check held directories after capture and before deletion

The shared entry point is [`filesystem/unix.rs`](../../../crates/spaceprune-access/src/filesystem/unix.rs). The Darwin guard is separate from Linux's implementation, in [`ancestry_macos.rs`](../../../crates/spaceprune-access/src/filesystem/unix/ancestry_macos.rs). For `root/A/B/file`:

1. Register observations on held root/A/B descriptors. Root/A subscribes to `NOTE_WRITE | NOTE_EXTEND | NOTE_ATTRIB | NOTE_LINK`; A/B subscribes to `NOTE_RENAME | NOTE_DELETE | NOTE_REVOKE`. Combine masks for overlapping descriptors and register each once.
2. Read root/A generations with `fgetattrlist`, then revalidate A/B names, identities and mount boundaries relative to held parents.
3. Atomically capture the selected file into private staging inside B.
4. Before deletion, reread root/A generations and check invalidations retained since registration. A changed count or invalidation returns `TargetChanged`; unavailable observation returns an explicit error.

Root remains an identity anchor: its own `NOTE_RENAME` is not observed. The deepest unselected parent B does not subscribe to content changes, so staging, capture and runtime-child mutations do not interrupt normal cleanup. A direct child of the verified root has no unselected ancestor and needs no generation attribute. An unrelated direct sibling rename in root/A conservatively stops the selection; filtering those events by ancestor name would miss mutations leading up to counter rollover.

Failures after capture still use existing restoration. If the original name is occupied, restoration does not overwrite the new object; it retains the original contents and reports their actual recovery location. The ancestor check precedes deletion, so a rejected selection reports no reclaimed space. DirectorySubtree's runtime-descendant authorization is unchanged, and F_GETPATH is not used to authorize deletion.

## Missing attributes and events are not “unchanged”

The FD request is `ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_GEN_COUNT` with `FSOPT_ATTR_CMN_EXTENDED`. [`ancestry_generation.rs`](../../../crates/spaceprune-access/src/filesystem/unix/ancestry_generation.rs) decodes the raw response: 28 bytes total, five returned-attribute bitmaps and one native-endian `u32` generation. Only the matching length, support bits and a nonzero count are accepted. A successful syscall with an unsupported attribute, a truncated response or zero returns `Unsupported` and stops deletion. The user receives the corresponding English or Chinese reason rather than another administrator prompt masking a missing volume capability.

Registration does not use `EV_CLEAR`, drain queued events or reset the baseline. Fixed XNU's [`knote_fill_kevent_with_sdata`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_event.c#L943-L996) clears `kn_fflags` only with `EV_CLEAR`; [`knote_process`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_event.c#L4424-L4447) reactivates ordinary non-CLEAR events. Reading an ordinary mutation event therefore does not erase it for subsequent checks. Vnode revocation's EOF/ONESHOT also stops the current capture; production restores on the first error rather than reusing that guard.

## Checks performed

- Targeted Linux `spaceprune-access` filesystem tests: 34 passed, 3 existing isolated/subprocess fixtures ignored, 0 failed.
- Message-contract tests: 2 passed; filesystem-plugin library tests: 26 passed.
- Strict `spaceprune-access --all-targets` Clippy: Linux, Intel Darwin (minimum 10.13) and Apple Silicon Darwin (minimum 11.0) passed.
- Strict all-target Clippy for CLI and the `spaceprune-readonly-helper` package: both Darwin architectures passed, including cleanup/helper-installer consumers.

Shared boundaries fixtures now create files under the canonical temporary parent, so macOS `/var` aliases cannot make NOFOLLOW fail before reaching the intended boundary. A Linux check using a temporary-directory symlink passed 5 cases with 1 existing mount fixture ignored; see `temporary-alias.log`. Production path validation is unchanged.

## Run the native cases on macOS

Run from a complete Core checkout containing this change, as an ordinary user without `sudo` so the 0555 ancestor-permission check is meaningful. The default temporary directory is usually on APFS. To test another mounted, writable HFS volume, first set `TMPDIR` to an empty directory dedicated to this test on that volume. Every case operates only in its own generated temporary child directories, not real cleanup candidates.

```sh
cargo test --locked -p spaceprune-access \
  filesystem::unix::ancestry::tests:: -- --nocapture

cargo test --locked -p spaceprune-access \
  filesystem::tests::ancestry:: -- --nocapture

cargo test --locked -p spaceprune-access \
  filesystem::tests::boundaries:: -- --nocapture
```

The two macOS movement counterexamples are no longer ignored; omit `--ignored`. Darwin-specific cases cover FD generation detecting rename/away-and-back, retained sibling invalidation despite equal raw counters and repeated checks, staging without self-contamination, identity-anchored root rename, `RENAME_SWAP` when supported, movement before registration and event-queue failure. Shared cases cover same-parent/outside-root/deep-ancestor moves, exclusions, zero-deletion restoration and 0555 ancestors. Restoration conflicts are checked by `boundaries::descendant_restore_never_overwrites_a_new_name_after_cancellation`.

The equal-counter case manually aligns the recorded baseline with the current value to isolate retained-event behavior; it does not execute four billion renames. If HFS rejects exchange with `ENOTSUP`, the case prints that limitation and checks only that the rejected operation has no effects. That branch does not count as acceptance of successful-exchange protection.

Include the OS version, CPU architecture, test-volume format and command output with feedback. Missing generations, staging self-invalidation, incorrect rejection of an identity-anchored root or mismatched restored content remain implementation issues to fix. The filesystem owner continues to establish APFS in-flight rename/read synchronization and the complete counter-mutation domain; ordinary tests performed after rename returns do not replace that timing evidence.
