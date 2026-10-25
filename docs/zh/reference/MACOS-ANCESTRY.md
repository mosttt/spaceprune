# macOS 祖先移动防护：实现与原生验收

[English](../../en/reference/MACOS-ANCESTRY.md)

Core 通过父目录 generation 与持续保留的 vnode 失效事件检查祖先移动。普通清理和特权 cleanup helper 使用同一条“捕获选中项 → 核对祖先 → 删除”路径。移动或观察失效时停止删除，沿现有 NOREPLACE 路径恢复；不移动未选祖先，也不增加它们的写权限要求。

Linux 回归和 Darwin 双架构编译检查已通过；原生 APFS/HFS 测试尚未执行。固定 HFS 源码支持目录 generation 的锁同步；APFS 在普通、compound、交换 rename 下的同步与完整计数变更域仍需确认，依据保存在[原语评估](MACOS-ANCESTRY-EVIDENCE.md)。

## 捕获后、删除前检查持有的目录

入口位于 [`filesystem/unix.rs`](../../../crates/spaceprune-access/src/filesystem/unix.rs)，Darwin guard 独立于 Linux 实现，位于 [`ancestry_macos.rs`](../../../crates/spaceprune-access/src/filesystem/unix/ancestry_macos.rs)。对 `root/A/B/file`：

1. 为持有的 root/A/B 描述符注册观察；root/A 监听 `NOTE_WRITE | NOTE_EXTEND | NOTE_ATTRIB | NOTE_LINK`，A/B 监听 `NOTE_RENAME | NOTE_DELETE | NOTE_REVOKE`。重叠描述符合并 mask，只注册一次。
2. 通过 `fgetattrlist` 读取 root/A generation，再从已持有的父目录核对 A/B 名称、身份及挂载边界。
3. 将选中的 file 原子捕获到 B 内的私有 staging。
4. 删除前重新读取 root/A generation，并检查从注册起保留的失效事件。任一计数变化或失效事件都返回 `TargetChanged`；观察服务不可用会明确报错。

root 是身份锚点，不监听它自身的 `NOTE_RENAME`。最深的未选父目录 B 不监听内容变更，避免 staging、捕获和运行时子项产生的自身事件中断正常清理。没有未选祖先的根下直接选择不需要 generation。其他程序改名 root/A 的无关直属兄弟项会保守中止本次选择；这些事件不能按祖先名称过滤，否则会漏掉计数回绕前的变化。

捕获后失败仍经过已有恢复路径；原名称已被占用时不覆盖新对象，保留原内容并报告实际恢复位置。祖先检查发生在实际删除前，失败的选择不会报告释放空间。该实现未改变 DirectorySubtree 对运行时后代的授权范围，也未把 F_GETPATH 用作删除授权条件。

## 缺失属性与事件不能被当作“未变化”

FD 属性请求为 `ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_GEN_COUNT`，使用 `FSOPT_ATTR_CMN_EXTENDED`。原始响应由 [`ancestry_generation.rs`](../../../crates/spaceprune-access/src/filesystem/unix/ancestry_generation.rs) 解码：总长度 28 字节、五个 returned-attribute 位图以及一个原生字节序 `u32` generation。只接受匹配的支持位、长度及非零计数；调用成功但不支持属性、截断响应和零值都返回 `Unsupported`，不继续删除。用户会看到对应的中英文原因，而不是再次请求管理员授权来掩盖卷能力缺失。

`kqueue` 注册不使用 `EV_CLEAR`，不排空事件，也不重置基线。固定 XNU 的 [`knote_fill_kevent_with_sdata`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_event.c#L943-L996) 只在 `EV_CLEAR` 时清除 `kn_fflags`；[`knote_process`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_event.c#L4424-L4447) 会重新激活普通非 CLEAR 事件。因此读取普通变更事件不会让后续检查忘记变化。vnode 撤销产生的 EOF/ONESHOT 也会中止当前捕获；生产流程在首次错误后恢复，不再复用该 guard。

## 已执行的检查

- Linux `spaceprune-access` 文件系统定向测试：34 通过，3 项既有隔离/子进程夹具 ignored，0 失败。
- 消息合同测试：2 通过；文件系统插件库测试：26 通过。
- `spaceprune-access --all-targets` 严格 Clippy：Linux、Intel Darwin（最低 10.13）和 Apple Silicon Darwin（最低 11.0）通过。
- CLI 与 `spaceprune-readonly-helper` 包的所有 target 严格 Clippy：两个 Darwin 架构通过，包含 cleanup/helper-installer 消费方。

共享 boundaries 夹具现在在 canonical 临时父目录下创建文件，避免 macOS `/var` 别名让 NOFOLLOW 提前拒绝、未到达真实边界。Linux 使用临时目录符号链接的定向检查为 5 通过、1 项原挂载夹具 ignored，见 `temporary-alias.log`；没有修改生产路径校验。

## 在 macOS 上运行原生用例

从包含本次改动的完整 Core 源码运行，使用普通用户，不加 `sudo`，以使 0555 祖先权限测试有效。默认临时目录所在卷通常为 APFS。验证另一个已挂载、可写的 HFS 卷时，可先将 `TMPDIR` 设置为该卷上专供本轮测试的空目录；所有用例只操作自己创建的临时子目录，不使用真实待清理文件。

```sh
cargo test --locked -p spaceprune-access \
  filesystem::unix::ancestry::tests:: -- --nocapture

cargo test --locked -p spaceprune-access \
  filesystem::tests::ancestry:: -- --nocapture

cargo test --locked -p spaceprune-access \
  filesystem::tests::boundaries:: -- --nocapture
```

macOS 两项祖先移动反例已取消 ignore，命令不再加 `--ignored`。Darwin 专属测试覆盖：FD generation 检出改名/移走再移回、无关兄弟变更在 raw 值相等时仍被事件拒绝且重复检查不丢失、自身 staging 不污染观察、根改名仍按身份使用、支持时的 `RENAME_SWAP`、注册前移动及事件队列故障。共享测试覆盖同父/跨根/深层移动、排除目录、零删除恢复和 0555 祖先；恢复冲突由 `boundaries::descendant_restore_never_overwrites_a_new_name_after_cancellation` 验证。

raw 值相等的用例人工将记录基线对齐当前计数，以隔离验证持续事件的作用；它没有实际执行四十亿次改名。HFS 对交换返回 `ENOTSUP` 时，用例明确打印不支持，并只检查被拒操作没有副作用；这一分支不算成功交换的保护验收。

反馈请附上系统版本、CPU 架构、测试卷格式以及命令输出。缺 generation、正常 staging 自触发、身份锚点被错误拒绝或恢复内容不符，都作为实现问题继续修复。APFS 在途 rename 与读取计数的同步依据及完整变更域仍由 filesystem owner 跟进；普通改名返回后的测试不能代替这一时序依据。
