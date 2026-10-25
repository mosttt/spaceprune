# macOS 删除祖先移动边界：公开原语评估

[English](../../en/reference/MACOS-ANCESTRY-EVIDENCE.md)

macOS 祖先移动防护的顺序为：在原子捕获选中项后、实际删除前，比较持有 FD 的父目录 generation，并检查从捕获前持续保留的 `kqueue` 失效事件。普通执行与特权 cleanup helper 复用同一实现，不移动未选祖先，也不增加它们的写权限要求。[实现、检查结果与原生验收入口](MACOS-ANCESTRY.md)记录当前交付状态。

Linux 定向回归和 Darwin 双架构编译检查已通过；原生 APFS/HFS 验收尚未执行。HFS 目录计数的锁同步有固定源码依据；APFS 普通、compound、交换 rename 的同步点和完整计数变更域仍需确认。本评估保留此前排除纯通知屏障等方案的依据：`NOTE_RENAME` 本身不能保证此前可见的移动通知已经发布。

## 保持的清理契约

验证根是身份锚点。对 `allowed/nested/file` 的选择只能捕获该文件；不能为了锁住路径而移动 `allowed`、`nested` 或其兄弟，不能增加这些未选祖先的写权限要求。祖先移入排除目录、移出根、同父改名，以及切走再切回，都必须在任何实际用户对象删除前被拒绝。失败后仍通过已有 NOREPLACE 恢复路径保留内容，零删除不得计作已释放空间。

DirectorySubtree 继续捕获运行时后代，单文件继续核对选择身份；普通与提升执行使用同一实现。F_GETPATH 只用于失败后的恢复定位，不成为删除授权依据。

## XNU 的通知发布晚于文件系统改名

固定版本 `xnu-4570.71.2` 和 `xnu-11215.81.4` 都有以下顺序：

1. `VNOP_RENAME`／`VNOP_RENAMEX` 调用文件系统操作。
2. 文件系统操作成功返回后，`post_rename` 才对源 vnode 发布 `NOTE_RENAME`。
3. `VNOP_COMPOUND_RENAME` 也先执行文件系统 compound 操作，然后发布通知。

Apple 开源 HFS 的 `hfs_vnop_rename` 先完成目录修改、更新 vnode 名称及父节点，再调用 `hfs_unlockfour` 释放相关 cnode 锁，最后返回到上述 XNU 包装层。因而存在“目录位置已经改变且目录锁已释放，但 NOTE_RENAME 尚未发布”的窗口。读取目录不能像 Linux 的父 inode i_rwsem 那样把这个窗口封住。

可构造的调度反例是：写方移动 `allowed` 至 `protected/moved`，在释放 HFS 锁后、XNU 发布通知前被调度出去；清理方经已持有的 `allowed` FD 捕获 `file`，读取目录并检查一个暂时为空的 kqueue，随后删除；写方恢复后才发布移动通知。这是根据原始源码得出的时序反例，本次没有在 macOS 内核上注入该暂停点。

“rename 返回后再调用 kevent 能读到通知”的普通功能测试只能证明完成态的观察，不能证明上述并发窗口不存在。增加睡眠、重复读取或等待一段安静时间也没有建立同步先后关系。

来源：

- [XNU 4570.71.2 `kpi_vfs.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-4570.71.2/bsd/vfs/kpi_vfs.c#L4227-L4407)：旧部署范围内也有同一发布顺序。
- [XNU 11215.81.4 `kpi_vfs.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/kpi_vfs.c#L4716-L4897)：`VNOP_RENAME`、`post_rename` 及 compound 路径。
- [Apple HFS `hfs_vnops.c`](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L5391-L5486)：名称更新、解锁和返回顺序。

## 父目录 generation：已采用的路线

此前只依据 `getattrlist(2)` 的数据修改描述，一概排除 `ATTR_CMN_GEN_COUNT`，结论过强。[Apple 当前 `generationIdentifier` 文档](https://developer.apple.com/documentation/foundation/urlresourcevalues/generationidentifier)（macOS 10.10 起）明确：

> For URLs which refer to the same directory inode, the generation identifier will change when direct children of that directory are added, removed or renamed.

对 `root/A/B/file`，实现观察未选祖先 A、B 的直接父目录 **root、A**：先注册观察，再读取基线并重核身份，捕获 file 后比较这两个目录的 generation。A/B 的同父改名、跨父移动和移走再移回会修改原父目录的直属子项；在 B 内建立 staging 并捕获 file 只修改 B，不污染 root/A 的计数。因此这不是递归计数，也不需要移动未选祖先。其他程序修改 root/A 的无关直属子项会保守拒绝该次选择，原生验证需确认其交互影响。

### HFS 源码显示计数在解锁前更新

固定 HFS `d1bac2f062e6e9c0dfcce302d9aacb10173d0eea` 对应 **hfs-715.100.10**，Apple 于 2026-04-17 导入。其路径为：

1. [`hfs_vnop_rename` 转发 `hfs_vnop_renamex`](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L4442-L4560)，该版本接受普通／exclusive rename，不支持的交换等 flag 返回 ENOTSUP。
2. renamex [独占锁住源父、源项、目标父、目标项 cnode](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L4602-L4616)。完成 `cat_rename` 后，跨父分支在 5366 行增加目标父 generation；同父和跨父路径均在 5383 行增加源父 generation；[5442 行才释放锁](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L5305-L5442)。
3. `hfs_vnop_getattr` 的 generation 请求[获取同一 cnode 的共享锁](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L895-L911)，[读取 `hfs_get_gencount(cp)`](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L1158-L1172)，到 1257 行才解锁。

这给出了该 HFS 目录 rename 路径的同步依据：读取计数会等待尚未释放的改名锁；到此前的“文件系统已解锁、通知尚未发布”窗口时，generation 已经改变。HFS 对一般文件数据并发写[另有一致性限制](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_cnode.c#L1736-L1775)，不能把这里的目录锁顺序扩张为文件内容快照保证。

固定 XNU `xnu-12377.1.9` 的 [`fgetattrlist`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.1.9/bsd/vfs/vfs_attrlist.c#L3485-L3528) 直接从持有的 FD 取得 vnode；[`ATTR_CMN_GEN_COUNT` 映射到 `va_write_gencount`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.1.9/bsd/vfs/vfs_attrlist.c#L565-L572)，再[调用文件系统读取属性](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.1.9/bsd/vfs/vfs_attrlist.c#L3418-L3430)。所需权限是读取属性，不是未选祖先的写权限。实现研究可优先使用这个 FD 接口，避免重新解析路径以及 Foundation URL 属性缓存。应检查 `ATTR_CMN_RETURNED_ATTRS` 的支持位与非零计数；调用成功但属性不支持，不能解释为目录未变。

### 回绕防护已接入，APFS 同步与覆盖域仍待验证

Apple 文档说明 generation identifier 并非所有卷都支持。实时下载的 [Apple File System Reference](https://developer.apple.com/support/downloads/Apple-File-System-Reference.pdf) 是 2020-06-22 版，第 76 页将 `write_generation_counter` 定义为 inode 或其数据修改时增加的计数，并明确允许溢出回到零。它描述磁盘格式，没有公开 APFS 在普通、compound、交换 rename 与属性读取之间的运行时锁顺序；不能直接套用 HFS 的证明。

HFS 的 [32 位 raw 计数也会回绕，只是跳过零](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_cnode.c#L1799-L1848)，非零值的周期为 `M = 2^32 − 1`。单独比较 raw 计数不是无限历史的无碰撞 token；官方 Foundation opaque identifier 与该 raw 值的一一映射也未证明。实现采用“父目录 generation＋持续保留的失效观察”，观察需要覆盖**同一计数的完整变更域**，不能只监听未选祖先自身的移动。

2026-10-22 的只读核对给出了这一差别的具体算法反例。假设对 `root/A/B/file` 比较 root/A 的 raw generation，却只监听 A/B 的 `NOTE_RENAME` 等自身移动事件：

1. 建立基线后，外部进程对 root 下一个无关兄弟项完成 `M−1` 次同父改名。HFS 的同父 rename [每次增加源父 generation](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vnops.c#L5363-L5383)；这些操作不移动 A/B。
2. 随后将 A 移入排除目录。root 的计数恰好回到基线值；把这一 rename 暂停在 HFS 解锁后、XNU `post_rename` 前。A 的直属子项 B 未变，所以 A 的观察计数也不变。
3. 清理方经已持有的 B FD 捕获 file。staging/capture 只改变 B，不改变 root/A；比较父目录计数、读取 A/B 的自身移动事件都可能看似未变，但选中项已随未选祖先越界。

这个反例不要求数十亿个并发 rename：此前的兄弟项改名可以串行完成。XNU 的 [`post_rename`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/kpi_vfs.c#L4800-L4829) 将 `NOTE_WRITE`（部分目录移动还含 `NOTE_LINK`）发送给源／目标父目录，将 `NOTE_RENAME` 发送给被移动项。因此只保留 A/B 的自身移动事件，看不到此前令 root 计数推进的兄弟项变更；覆盖 root/A 直属项修改的 sticky 观察才有机会拒绝这条序列。后续仍须核对所支持文件系统的其他计数推进路径，不能把这份 rename 事件清单直接当作 APFS 的完整覆盖证明。

这是固定 HFS／XNU 调用顺序下的候选算法反例，尚无默认执行窗口内的原生复现。[`OperationContext::workflow`](../../../crates/spaceprune-access/src/operation.rs) 默认采用 3600 秒单调截止时间，`new` 接受调用方时限；这些时限没有给出期间外部目录变更次数严格小于 `M` 的可证明上界。不得用“实际很难回绕”代替无碰撞或完整失效观察的依据。

当前实现对 root/A 注册 `NOTE_WRITE | NOTE_EXTEND | NOTE_ATTRIB | NOTE_LINK`，对 A/B 注册 `NOTE_RENAME | NOTE_DELETE | NOTE_REVOKE`，重叠 FD 合并注册；不使用 `EV_CLEAR`、不排空事件、不重置基线。这样可保留上述兄弟项改名的失效，而 B 内 staging／运行时后代捕获不会触发父目录观察。没有增加未选祖先的写权限要求。属性缺失、支持位不完整或计数为零会明确返回 `Unsupported`，不继续删除；直接位于验证根下、没有未选祖先的选择不需要这项属性。原生测试以人工对齐 raw 基线检查事件是否仍拒绝选择，并重复检查保留性；它不是四十亿次改名实测。APFS 同步点和事件完整覆盖域继续作为验收条件。

## 其他已核对入口不能直接替代

XNU 的 mount rename 锁只覆盖相应的跨父目录目录改名路径，不是用户态可以直接持有的通用祖先锁；同父目录改名不满足该分支条件。额外移动一个私有空目录，不能据此声称同步了所有祖先改名。非 compound 路径的 `vnode_link_lock` 也不能推导为所有文件系统和调用路径都有的用户态观察屏障。见 [`renameat_internal`](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/vfs_syscalls.c#L9461-L9544)。

`F_SETLEASE` 并非普通进程或仅凭 UID 0 就能使用的替代。XNU `vnode_setlease` 首先检查 `com.apple.private.vfs.file-leases`，不具备该 entitlement 时返回 EPERM；目录也只支持 read lease。不能通过再次请求管理员密码修复这一限制。见 [`vfs_subr.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/vfs_subr.c#L12618-L12656) 及同文件 [获取检查](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/vfs/vfs_subr.c#L12855-L12876)。本次没有请求或尝试绕过 entitlement。

即使具有该 entitlement，简单安装 lease 也没有覆盖所有在途操作。较新固定 XNU 中，rename 先完成 `vnode_breakdirlease`，释放 vnode mutex 后才调用 `vn_rename`；获取 lease 则另行取得该 mutex。源码允许旧 rename 已通过 break 检查、新 lease 随后安装、旧 rename 再执行 VNOP 的时序，未发现安装时排空这类旧操作的保证。因此“取得 entitlement”不是充分实施条件。依据为 [`vfs_syscalls.c`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_syscalls.c#L9902-L9916) 与 [`vfs_subr.c`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_subr.c#L12857-L12937)；这是时序推论，尚未做内核暂停点实测。

2026-10-22 又核对了可能被误解为安装屏障的 expected-count 参数。[`F_SETLEASE` 分派](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_descrip.c#L5097-L5114)明确其含义是当前 open/write count，目的是容许调用方自身的多次打开；[`check_for_open_conflict`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_subr.c#L12802-L12828)检查 `v_writecount`／`v_usecount` 和可写 mmap，不检查 rename epoch 或在途 rename 数量。

更直接的反例写在 [`wait_for_lease_break`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_subr.c#L13111-L13126)：旧目录 lease 被释放、等待者被唤醒，但另一个新 lease 已先安装时，若新 lease 没有 pending-break 标志，旧等待者按“无冲突”继续。因而“当前持有新 read lease”不能证明旧 rename 已排空，expected-count 也不能补上这一保证。这仍是固定源码的时序依据，不是新增的 macOS 原生运行结果。

`ATTR_CMNEXT_RECURSIVE_GENCOUNT` 仅适用于标记了 maintain-dir-stats 的 APFS 目录，其他目录返回零。把它设为所有旧选择的新前提会改变使用条件；自己捕获后代也会改变递归计数，不能把任意变化都解释为祖先越界。见 [recursive gencount 原始手册](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/getattrlist.2#L1338-L1346)。这项限制不能套用到上述非递归父目录 generation 候选。

新版 XNU 的 `RENAME_RESOLVE_BENEATH`／`O_RESOLVE_BENEATH` 只约束路径解析，不能冻结根内组件名称或记录切走再切回；提高最低 OS 不能单独解决祖先窗口。旧版本没有这些 flag，不能把 `renameatx_np` 函数的 availability 当作 flag 的可用性。见固定 [rename 手册](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/rename.2#L147-L178) 和 [namei 检查](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_lookup.c#L1383-L1404)。

Endpoint Security 确有公开的 `es_sync_client`（当前文档标 macOS 27.0），但它把 marker 放在 client 消息队尾，等待前面的消息 handled，不保证此前已获得 ALLOW 的 syscall 完成。订阅、清除授权缓存、开始拒绝 rename 再等待消息同步，仍缺少对旧在途 rename 的排空保证。普通系统级 ES client 需要 root、Apple 批准的 entitlement 和 Full Disk Access；并非必须使用 system extension，但任何这类新增安装能力都不能代替上述同步保证。见 [Apple API 文档](https://developer.apple.com/documentation/endpointsecurity/es_sync_client(_:_:)) 及 [WWDC 2020](https://developer.apple.com/videos/play/wwdc2020/10159/)。

`NSFileCoordinator` 和 advisory locks 只能约束合作参与方。HFS 的全卷 freeze 则连发起方后续需要 journal 锁的修改也会等待 thaw，不能让清理者在冻结期间独享捕获；先 thaw 再捕获会重新打开窗口。它还影响整卷其他工作，不适合作为普通选择的新增前提。该结论来自 [HFS freeze 状态](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_readwrite.c#L2297-L2322) 与 [journal 等待](https://github.com/apple-oss-distributions/hfs/blob/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea/core/hfs_vfsutils.c#L1197-L1245)，不据此推断 APFS 私有实现。

## FSEvents 同步刷新尚不能提供祖先移动屏障

[Apple 文档](https://developer.apple.com/documentation/coreservices/1445629-fseventstreamflushsync)说明，该函数同步送达因创建 stream 时的 latency 参数而延迟的事件；返回时，已发生事件的 callback 已被调用。文档描述的是事件投递，没有说明“文件系统名称修改已经可见，但负责通知的线程尚未发布事件”是否也必须被等待。

固定 XNU `xnu-12377.1.9`（commit `f6217f891ac0bb64f3d375211650a4c1ff8ca1ea`）给出了决定这个区别的顺序：

1. [`renameat_internal`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_syscalls.c#L9911-L10063)先执行 `vn_rename`、释放 vnode link／mount rename 锁，再调用 `add_fsevent(FSE_RENAME)`。
2. [`add_fsevent`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_fsevents.c#L365-L424)内部才读取 `mach_absolute_time()`，用作事件时间；不是在命名空间修改前预留事件序号。
3. [`watcher_add_event`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_fsevents.c#L1531-L1547)才更新 watcher 的 `max_event_id`；[`FSEVENTS_GET_CURRENT_ID`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_fsevents.c#L2236-L2239)只返回这个值。`num_pending_rename` 也在 `add_fsevent` 分配事件后才增加，并不是已进入 rename syscall 的计数器。

因此，“等待当前事件 ID”或“等待本次 capture 自己产生的事件”不能当作祖先移动屏障：祖先 rename 先完成文件系统名称修改，在既有 HFS 证据所示的解锁后、通知前暂停；清理线程随后捕获后代并完成自己的事件发布；祖先线程最后恢复，才得到更晚的事件 ID。投递完 capture 事件，仍不代表更早发生的祖先名称修改已经被观察。这个反例来自源码时序推论，本次没有做 macOS 内核暂停点实测。

`FSEventStreamFlushSync` 的用户态 framework/service 实现不在本次公开源码范围内。上述证据排除了直接以队列/ID顺序作证明，尚不能裁定该函数是否另有未写入公开文档的在途 VFS 操作屏障。普通“rename 返回后再 flush”的原生测试无法区分这两种语义；此时事件已走过发布点。

## 留给原生验收的可执行反例

移动测试使用 Unix 共享测试源，2026-10-22 已取消 macOS 两项移动反例的 ignore；尚未接入防护的其他 Unix 平台继续明确 ignored。未移动祖先的 0555 权限兼容测试在 Unix 上正常参与测试。夹具使用 canonical 临时路径，避免 macOS 的 `/var` 临时路径别名先触发“链接祖先”错误，从而没有到达要验证的移动时序。

在可用的 macOS 开发机上，用仓库原工具链运行：

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

前两项验证祖先移动后零删除及内容恢复，0555 用例保护原权限契约；Darwin 专属测试补充 FD generation、兄弟事件保留、重复检查、自污染、根身份锚点、交换及队列故障。每项只操作自身生成的临时目录。它们没有内核通知发布暂停点，原生结果仍需与文件系统同步依据一起判断。完整命令及卷选择方法见[实现记录](MACOS-ANCESTRY.md)。

## 下一步与交付状态

生产接入已完成，macOS 原生验收继续保持未完成。下一步验证父目录 generation 实现，不再重复已排除的纯通知 flush 路线：

1. 在原生 APFS/HFS 上运行上述已接入的测试，检查 generation 支持位与非零值、同父改名、跨父移动、移走再移回、交换、0555、恢复和 B 内 staging/capture 不污染 root/A 的基线。该检查用于发现属性不支持、映射错误和自污染；在途操作顺序仍需同步依据。
2. 确认 APFS 的属性读取是否与目录名称修改使用同一同步边界，并完成 raw 计数回绕／持续失效观察的处理依据。持续观察须覆盖被采样父目录的完整计数变更域，而非仅监控 A/B 自身移动。固定 XNU 的 [`vnode_getattr`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/kpi_vfs.c#L2831-L2837)与 [`VNOP_GETATTR`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/kpi_vfs.c#L3815-L3828)在这两层直接转发到文件系统，不能据此补出 APFS 的 namespace 同步保证。向 Apple 求证的具体问题为：

> For a directory on APFS, does fgetattrlist(fd, ATTR_CMN_GEN_COUNT, FSOPT_ATTR_CMN_EXTENDED) observe generation changes under the same synchronization boundary as changes to its direct children made by rename/renameatx_np, including same-parent, cross-parent, compound and RENAME_SWAP operations? Can a namespace mutation be visible to a subsequent descendant capture while an attribute read still returns the pre-mutation count? What is the supported strategy for detecting count rollover without forgetting an ancestor moved away and back?

若此候选不能满足完整契约，再求证以下两种最小能力之一，而不是继续叠加路径重查：

1. 原子祖先条件捕获：在同一 namespace 同步边界内核对验证根、原祖先身份／名字、观察 token 和选中项，仅将选中项 NOREPLACE 捕获到私有 staging。token 安装须覆盖已经进入而尚未完成的 rename；同父改名、跨父移动、compound rename 和切走再切回均使其失效。
2. 通知发布屏障：沿现有 watch → 身份重核 → capture → after_capture 流程，保证 capture 前已经改变祖先位置的操作，其失效通知在屏障返回前可见，包括文件系统已解锁但尚未发布通知的窗口。单纯排空当前消息队列不满足此条件。

2026-10-18 的 FSEvents 核对将通知屏障问题收敛到以下可向 Apple 确认的具体契约：

> After a filesystem rename has made the namespace change visible and released its filesystem locks, but before the renaming thread reaches `post_rename` / `add_fsevent(FSE_RENAME)`, does `FSEventStreamFlushSync` on another thread wait for that in-flight rename and deliver its invalidation before returning? Does the guarantee cover both same-parent and cross-parent directory renames, compound rename paths, and an ancestor moved away and back? If not, is there a supported API providing that barrier without changing or capturing unselected ancestors?

需要平台支持的暂停点或等价可控机制，将祖先 rename 暂停在“名称已修改、事件尚未发布”之间；此时屏障必须等待或报错，不能成功返回并进入删除。通知完成后应拒绝该选择，通过现有 NOREPLACE 恢复保留内容。普通 rename 返回后再 flush 的测试无法区分这一窗口。

这两项是 generation 路线之外的待确认能力规格，不是现有 API 名称。当前 Darwin guard 已接入；filesystem owner 根据原生结果和后续同步依据修正它，验收对应在途窗口、同父改名／ABA、compound 路径和计数回绕，同时保留正常 0555 清理和恢复行为。

若只能依靠特殊 entitlement、系统扩展或捕获未选祖先，需要先由项目负责人确认其安装/权限或清理范围契约变化；它们不能作为现有调用方的静默前提。仅提供原生 runner 不会自动解决这一设计依赖。Android、iOS 及其他 Unix 的等价屏障也未因本次调查关闭。
