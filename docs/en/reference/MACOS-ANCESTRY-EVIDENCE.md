# macOS ancestor-movement boundary: assessment of public primitives

[简体中文](../../zh/reference/MACOS-ANCESTRY-EVIDENCE.md)

macOS ancestor-movement protection follows this sequence: after atomically capturing the selection and before deleting it, compare parent-directory generations through held FDs and check `kqueue` invalidations retained since before capture. Ordinary execution and the privileged cleanup helper share this implementation. It neither moves unselected ancestors nor adds write-permission requirements for them. The [implementation, checks and native acceptance commands](MACOS-ANCESTRY.md) describe the current delivery.

Targeted Linux regression tests and checks for both Darwin architectures passed; native APFS/HFS acceptance has not run. Fixed HFS source establishes directory-counter lock synchronization. APFS synchronization for ordinary, compound and exchange rename, and its complete counter-mutation domain, still need confirmation. This assessment retains the evidence against earlier notification-only approaches: `NOTE_RENAME` alone does not establish that every already-visible move has published its notification.

## Preserve the cleanup contract

The verified root is the identity anchor. Selecting `allowed/nested/file` authorizes capturing that file only. It does not authorize moving `allowed`, `nested` or their siblings, or adding write-permission requirements for those unselected ancestors. Moving an ancestor into an exclusion, outside the root, to another name in the same parent, or away and back must be rejected before deleting actual user objects. Failure must preserve content through the existing NOREPLACE recovery path; zero deletions must not be reported as reclaimed space.

DirectorySubtree continues to capture runtime descendants; a single-file selection continues to check the selected identity. Ordinary and elevated execution use the same implementation. F_GETPATH is for locating retained content after failure, not authorizing deletion.

## XNU publishes notifications after the filesystem rename

Both fixed versions, `xnu-4570.71.2` and `xnu-11215.81.4`, have this ordering:

1. `VNOP_RENAME` / `VNOP_RENAMEX` calls the filesystem operation.
2. After the filesystem returns success, `post_rename` publishes `NOTE_RENAME` for the source vnode.
3. `VNOP_COMPOUND_RENAME` also performs the filesystem operation before posting notifications.

Apple's HFS `hfs_vnop_rename` changes the directory, updates the vnode's name and parent, releases the cnode locks through `hfs_unlockfour`, and only then returns to the XNU wrapper. There is consequently a window in which the namespace has changed and filesystem locks are released, but `NOTE_RENAME` has not been published. Reading a directory cannot close this window as Linux's parent-inode i_rwsem does.

A source-derived schedule is: the writer moves `allowed` to `protected/moved`, then is descheduled after releasing HFS locks but before XNU notification publication. The cleaner captures `file` through its existing `allowed` FD, reads the directory, sees an empty kqueue and deletes. The writer resumes and publishes the notification later. This is a source-ordering counterexample, not a pause-point experiment on a macOS kernel.

A test that calls kevent after rename returns proves observation of a completed operation, not exclusion of this concurrent window. Sleeping, repeated reads or waiting for a quiet interval creates no synchronization relationship.

Sources:

- [XNU 4570.71.2 `kpi_vfs.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-4570.71.2/bsd/vfs/kpi_vfs.c#L4227-L4407): the same ordering exists in the older deployment range.
- [XNU 11215.81.4 `kpi_vfs.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/kpi_vfs.c#L4716-L4897): ordinary, extended and compound rename paths.
- [Fixed Apple HFS `hfs_vnops.c`](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L5391-L5486): name update, unlock and return ordering.

## Parent-directory generation is the implemented route

The earlier blanket exclusion of `ATTR_CMN_GEN_COUNT` based only on getattrlist's file-data wording was too strong. Apple's current [generationIdentifier documentation](https://developer.apple.com/documentation/foundation/urlresourcevalues/generationidentifier), available since macOS 10.10, states:

> For URLs which refer to the same directory inode, the generation identifier will change when direct children of that directory are added, removed or renamed.

For `root/A/B/file`, the implementation observes **root and A**, the direct parents of the unselected ancestors A and B. Register observations before sampling the baseline and revalidating identities, capture file, then compare these two generations. Same-parent renames, cross-parent moves and away-and-back moves of A/B change their original parents' direct entries. Staging and capture inside B change B, not root/A. This is a nonrecursive counter and requires no movement of unselected ancestors. Unrelated direct-child changes in root/A conservatively reject the selection; native validation must assess that interaction.

### HFS updates the counter before unlocking

Fixed HFS commit `d1bac2f062e6e9c0dfcce302d9aacb10173d0eea` is **hfs-715.100.10**, imported by Apple on 2026-04-17:

1. [`hfs_vnop_rename` forwards to `hfs_vnop_renamex`](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L4442-L4560). This version accepts ordinary/exclusive rename and rejects unsupported flags such as exchange with ENOTSUP.
2. renamex [exclusively locks the source parent, source, target parent and target cnodes](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L4602-L4616). After `cat_rename`, cross-parent movement increments the target parent's generation at line 5366. Both same-parent and cross-parent paths increment the source parent's generation at line 5383; [unlock occurs only at line 5442](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L5305-L5442).
3. A generation request in `hfs_vnop_getattr` [takes the same cnode's shared lock](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L895-L911), [reads `hfs_get_gencount(cp)`](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L1158-L1172), and unlocks at line 1257.

This establishes synchronization for the checked HFS directory-rename path: the reader waits for the rename's exclusive lock, and the count is already updated in the filesystem-unlocked/notification-unpublished window. HFS documents [separate consistency limits for concurrent file-data writes](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_cnode.c#L1736-L1775); the directory-lock result does not establish file-content snapshots.

In fixed XNU `xnu-12377.1.9`, [`fgetattrlist`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.1.9/bsd/vfs/vfs_attrlist.c#L3485-L3528) obtains the vnode directly from the held FD. [`ATTR_CMN_GEN_COUNT` maps to `va_write_gencount`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.1.9/bsd/vfs/vfs_attrlist.c#L565-L572), then [calls the filesystem for attributes](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.1.9/bsd/vfs/vfs_attrlist.c#L3418-L3430). Authorization is attribute-read access, not write access to an unselected ancestor. Prefer this FD interface over resolving a path or relying on Foundation's URL-property cache. Request `ATTR_CMN_RETURNED_ATTRS` and check support and a nonzero count; a successful call with an unsupported attribute is not evidence of an unchanged directory.

### Rollover handling is integrated; APFS synchronization and coverage remain open

Apple documents that generation identifiers are not supported on every volume. The downloaded [Apple File System Reference](https://developer.apple.com/support/downloads/Apple-File-System-Reference.pdf), dated 2020-06-22, defines `write_generation_counter` on page 76 as incrementing when the inode or its data changes and explicitly permits overflow to zero. This is an on-disk format description, not APFS runtime lock ordering between ordinary, compound or exchange rename and attribute reads. HFS's proof cannot simply be transferred to APFS.

[HFS's 32-bit raw counter also wraps, skipping zero](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_cnode.c#L1799-L1848), so its nonzero cycle has length `M = 2^32 − 1`. Raw equality is not a collision-free token for unlimited history. A one-to-one mapping between Foundation's opaque identifier and this raw value is also unproved. The implementation combines parent generation with sticky invalidation. Observation must cover **the complete mutation domain of that same counter**, not just the unselected ancestors' own movements.

The 2026-10-22 read-only review supplies a concrete algorithmic counterexample. Suppose `root/A/B/file` uses root/A generation comparisons but only watches A/B for self-movement events such as `NOTE_RENAME`:

1. After establishing the baseline, an external process completes `M−1` same-parent renames of an unrelated sibling under root. Each HFS same-parent rename [increments the source parent's generation](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L5363-L5383); none moves A/B.
2. Move A into an excluded directory. Root's count returns exactly to the baseline. Pause this rename after HFS unlocks but before XNU `post_rename`. A's direct child B is unchanged, so A's observed count is unchanged too.
3. Capture file through the held B FD. Staging/capture changes B, not root/A. Parent-generation comparisons and A/B's self-movement event queues can both appear unchanged, although the selection has crossed the boundary with its unselected ancestor.

This sequence does not require billions of concurrent renames: the earlier sibling renames can finish serially. XNU [`post_rename`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/kpi_vfs.c#L4800-L4829) sends `NOTE_WRITE` (also `NOTE_LINK` for some directory moves) to the source/target parents and `NOTE_RENAME` to the moved object. Watching only A/B self-movement misses the sibling mutations that advanced root's count. Sticky observation of root/A direct-child mutations can reject this sequence. Other counter-advancing paths on supported filesystems still need examination; this rename event list alone does not establish full APFS coverage.

This is a candidate-algorithm counterexample derived from fixed HFS/XNU ordering, not a native reproduction within the default execution window. [`OperationContext::workflow`](../../../crates/spaceprune-access/src/operation.rs) defaults to a 3600-second monotonic deadline, while `new` accepts a caller-selected timeout. Those deadlines provide no proved bound that external directory mutations remain strictly below `M`. Practical difficulty of wrapping is not a collision-freedom or complete-invalidation argument.

The implementation registers `NOTE_WRITE | NOTE_EXTEND | NOTE_ATTRIB | NOTE_LINK` on root/A and `NOTE_RENAME | NOTE_DELETE | NOTE_REVOKE` on A/B, combining masks for overlapping FDs. It does not use `EV_CLEAR`, drain events or reset the baseline. This retains the sibling-rename invalidation above without observing staging/runtime-child changes inside B. It adds no write-permission requirement for unselected ancestors. Missing attributes, incomplete support bits or zero counts return an explicit `Unsupported` error and stop deletion. A selection directly under the verified root has no unselected ancestor and needs no generation attribute. The native test manually aligns the raw baseline to check that retained events still reject the selection, including repeated checks; it does not perform four billion renames. APFS synchronization and complete event coverage remain acceptance conditions.

## Other examined interfaces do not directly replace the missing primitive

XNU's mount rename lock covers particular cross-parent directory moves; userspace cannot hold it as a general ancestor lock. Same-parent rename does not meet that branch condition. Moving an extra private empty directory therefore does not establish synchronization with every ancestor rename. The noncompound path's `vnode_link_lock` likewise provides no general userspace observation fence covering every filesystem and dispatch path. See [`renameat_internal`](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/vfs_syscalls.c#L9461-L9544).

`F_SETLEASE` is unavailable merely by being an ordinary process or UID 0. XNU `vnode_setlease` first requires `com.apple.private.vfs.file-leases`, returning EPERM without it; directories support read leases only. Asking for the administrator password again cannot resolve this restriction. See [`vfs_subr.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/vfs_subr.c#L12618-L12656) and its [acquisition checks](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/vfs_subr.c#L12855-L12876). No entitlement was requested or bypassed.

Even with that entitlement, simply installing a lease does not cover all in-flight operations. In the newer fixed XNU, rename completes `vnode_breakdirlease`, releases the vnode mutex, then calls `vn_rename`; lease acquisition separately takes that mutex. An old rename can pass its break check, a new lease can be installed, and the old rename can then perform its VNOP. No installation-time drain of those operations was established. Entitlement acquisition is therefore insufficient. Sources: [`vfs_syscalls.c`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_syscalls.c#L9902-L9916) and [`vfs_subr.c`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_subr.c#L12857-L12937). This is a timing inference, not a kernel pause-point experiment.

The 2026-10-22 review also checked the expected-count argument, which might otherwise be mistaken for an installation fence. [`F_SETLEASE` dispatch](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_descrip.c#L5097-L5114) explicitly defines it as the current open/write count, allowing the caller's own multiple opens. [`check_for_open_conflict`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_subr.c#L12802-L12828) checks `v_writecount` / `v_usecount` and writable mappings, not a rename epoch or in-flight rename count.

A more direct counterexample is encoded in [`wait_for_lease_break`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_subr.c#L13111-L13126): an old directory lease is released and its waiter wakes, but another lease has already been installed. If the new lease lacks pending-break flags, the old waiter proceeds as if there were no conflict. Holding a new read lease does not prove that old renames have drained, and expected-count does not add that guarantee. This remains fixed-source timing evidence, not a new macOS native result.

`ATTR_CMNEXT_RECURSIVE_GENCOUNT` is restricted to APFS directories marked maintain-dir-stats; other directories return zero. Requiring it for all existing selections changes their conditions of use. Capturing descendants also advances this recursive count, so arbitrary changes cannot be classified as ancestor boundary violations. See the [original manual](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/getattrlist.2#L1338-L1346). This restriction does not apply to the nonrecursive parent-generation candidate above.

Newer XNU `RENAME_RESOLVE_BENEATH` / `O_RESOLVE_BENEATH` constrains path resolution; it does not freeze component names within the root or record away-and-back moves. Raising the minimum OS version alone is insufficient. Older versions lack these flags; the availability of `renameatx_np` itself does not establish flag availability. See the fixed [rename manual](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/rename.2#L147-L178) and [namei checks](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_lookup.c#L1383-L1404).

Endpoint Security exposes `es_sync_client` (current documentation labels it macOS 27.0), but it queues a marker and waits for earlier messages to be handled; it does not guarantee completion of syscalls already allowed. Subscribing, clearing authorization caches, denying subsequent renames and waiting for message synchronization still lacks an in-flight-rename drain. A normal system-wide ES client requires root, Apple's approved entitlement and Full Disk Access. It need not always be a system extension, but extra installation capabilities do not replace the missing synchronization guarantee. Sources: [Apple API documentation](https://developer.apple.com/documentation/endpointsecurity/es_sync_client(_:_:)) and [WWDC 2020](https://developer.apple.com/videos/play/wwdc2020/10159/).

`NSFileCoordinator` and advisory locks constrain cooperating participants only. HFS whole-volume freeze also blocks subsequent modifications by the freezing process when they need the journal lock; it cannot give the cleaner exclusive capture while frozen. Thawing before capture reopens the window. It affects unrelated work on the whole volume and is unsuitable as a new prerequisite for ordinary selections. Sources: [HFS freeze state](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_readwrite.c#L2297-L2322) and [journal waiting](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vfsutils.c#L1197-L1245). This does not establish APFS private behavior.

## FSEvents synchronous flushing has not established an ancestor fence

[Apple's documentation](https://developer.apple.com/documentation/coreservices/1445629-fseventstreamflushsync) says the call synchronously delivers events delayed by the stream's latency setting, with callbacks invoked for events that have occurred before it returns. It describes delivery, without specifying whether it must also wait for a namespace change already visible in the filesystem whose notification has not yet been published.

Fixed XNU `xnu-12377.1.9`, commit `f6217f891ac0bb64f3d375211650a4c1ff8ca1ea`, distinguishes these cases:

1. [`renameat_internal`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_syscalls.c#L9911-L10063) performs `vn_rename`, releases vnode-link/mount-rename locks, then calls `add_fsevent(FSE_RENAME)`.
2. [`add_fsevent`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_fsevents.c#L365-L424) reads `mach_absolute_time()` only then to assign the event time. It does not reserve a sequence before namespace mutation.
3. [`watcher_add_event`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_fsevents.c#L1531-L1547) subsequently updates the watcher's `max_event_id`; [`FSEVENTS_GET_CURRENT_ID`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_fsevents.c#L2236-L2239) only returns it. `num_pending_rename` also increments after event allocation inside `add_fsevent`, not on entry to the rename syscall.

Waiting for the current event ID or the capture's own event is therefore not an ancestor fence. An ancestor rename can change the namespace, pause in the HFS unlock-before-notification window, and receive a later event ID only after the cleaner captures a descendant and publishes its own event. Delivery of the capture event does not establish observation of the earlier ancestor mutation. This is a source-derived schedule, not a macOS kernel pause-point result.

The userspace framework/service implementation of `FSEventStreamFlushSync` was outside the inspected public source. The evidence excludes a proof based directly on queue/ID ordering; it does not establish whether the function has some additional, undocumented in-flight VFS fence. Ordinary rename-then-flush tests cannot distinguish that possibility because notification publication has already occurred.

## Existing executable counterexamples for native validation

Movement tests share Unix test sources. The two macOS movement counterexamples were unignored on 2026-10-22; other Unix platforms without the guard remain explicitly ignored. The unchanged-ancestry 0555-permission test runs normally on Unix. Fixtures use canonical temporary paths so macOS `/var` aliases cannot fail first as a symlink-ancestor error and prevent reaching the intended schedule.

On an available macOS development machine, use the repository toolchain:

```sh
cargo test --locked -p spaceprune-access \
  filesystem::tests::boundaries::a_selected_file_is_not_deleted_after_its_ancestor_moves_into_an_exclusion \
  -- --exact --nocapture

cargo test --locked -p spaceprune-access \
  filesystem::tests::ancestry::moving_any_unselected_ancestor_including_away_and_back_preserves_the_selection \
  -- --exact --nocapture

cargo test --locked -p spaceprune-access \
  filesystem::tests::ancestry::unchanged_ancestry_needs_no_new_parent_write_permissions \
  -- --exact --nocapture

cargo test --locked -p spaceprune-access \
  filesystem::unix::ancestry::tests:: -- --nocapture
```

The first two check zero deletion and content restoration after ancestor movement; the 0555 test protects the original permission contract. Darwin-specific tests cover FD generation, retained sibling events, repeated checks, self-contamination, root identity anchoring, exchange and queue failure. Each touches only its own temporary directories. These tests contain no kernel notification-publication pause point, so native results must be assessed alongside filesystem synchronization evidence. See the [implementation record](MACOS-ANCESTRY.md) for complete commands and volume selection.

## Next step and delivery status

Production integration is complete; macOS native acceptance remains unfinished. Validate the parent-generation implementation rather than repeating the excluded notification-only flush route:

1. Run the integrated tests on native APFS/HFS to check generation support and nonzero values, same-parent and cross-parent moves, away/back, exchange, 0555 permissions, recovery and absence of root/A contamination from staging/capture inside B. These tests detect unsupported attributes, mapping errors and self-contamination; in-flight ordering still requires synchronization evidence.
2. Establish whether APFS attribute reads share a synchronization boundary with directory namespace mutation, and establish rollover/sticky-invalidation handling. Observation must cover the sampled parents' complete counter-mutation domain, not just A/B self-movement. Fixed XNU [`vnode_getattr`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/kpi_vfs.c#L2831-L2837) and [`VNOP_GETATTR`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/kpi_vfs.c#L3815-L3828) directly dispatch to the filesystem at these layers; they do not supply the missing APFS namespace synchronization guarantee. The concrete question for Apple is:

> For a directory on APFS, does fgetattrlist(fd, ATTR_CMN_GEN_COUNT, FSOPT_ATTR_CMN_EXTENDED) observe generation changes under the same synchronization boundary as changes to its direct children made by rename/renameatx_np, including same-parent, cross-parent, compound and RENAME_SWAP operations? Can a namespace mutation be visible to a subsequent descendant capture while an attribute read still returns the pre-mutation count? What is the supported strategy for detecting count rollover without forgetting an ancestor moved away and back?

If this candidate cannot preserve the complete contract, establish one of these minimum capabilities rather than adding repeated path checks:

1. Atomic ancestry-conditional capture: within one namespace synchronization boundary, check the verified root, original ancestor identities/names, observation token and selection, and capture only the selection into private staging with NOREPLACE. Token installation must cover already-entered, unfinished renames. Same-parent, cross-parent, compound and away/back movement must invalidate it.
2. A notification-publication fence: along watch → identity revalidation → capture → after_capture, guarantee that invalidations for ancestor changes already visible before capture are observable before the fence returns, including the filesystem-unlocked/notification-unpublished window. Draining the currently queued messages is insufficient.

The 2026-10-18 FSEvents review narrowed the platform question to:

> After a filesystem rename has made the namespace change visible and released its filesystem locks, but before the renaming thread reaches `post_rename` / `add_fsevent(FSE_RENAME)`, does `FSEventStreamFlushSync` on another thread wait for that in-flight rename and deliver its invalidation before returning? Does the guarantee cover both same-parent and cross-parent directory renames, compound rename paths, and an ancestor moved away and back? If not, is there a supported API providing that barrier without changing or capturing unselected ancestors?

A platform-supported pause point or equivalent controlled mechanism must stop ancestor rename after namespace modification but before notification publication. The fence must then wait or fail, not return success and permit deletion. Once notification completes, reject the selection and preserve content through existing NOREPLACE recovery. A rename-returned-then-flush test cannot expose this distinction.

These are capability specifications beyond the generation route, not names of existing APIs. The Darwin guard is now integrated. The filesystem owner should use native results and subsequent synchronization evidence to correct it and accept the in-flight window, same-parent/ABA, compound paths and rollover, while preserving normal 0555 cleanup and recovery behavior.

A route requiring special entitlements, a system extension or capturing unselected ancestors needs the project owner's explicit decision about changed installation/permission or cleanup-scope contracts; none may become a silent prerequisite for existing callers. A native runner alone does not resolve this design dependency. Android, iOS and other Unix equivalents remain open too.
