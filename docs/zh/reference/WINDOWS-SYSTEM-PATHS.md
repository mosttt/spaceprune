# Windows 系统位置与路径表示

`spaceprune-windows-elevation::SystemLocations` 提供只读的系统位置快照。调用方可以查询系统卷、Windows 目录和受保护的系统目录，但该对象不表示读取或删除权限。

`query()` 使用 `GetSystemWindowsDirectoryW` 与 Known Folder API 取得 Windows、Program Files、Program Files (x86) 和 ProgramData 的配置位置。系统卷来自 `GetVolumePathNameW`；实际目录和卷 GUID 别名由原生 API 与文件系统规范路径建立，不直接读取 `SystemDrive`、`SystemRoot`、`ProgramFiles` 环境变量，也不假设 Windows 必须位于 `C:\Windows`。

`is_system_volume()` 只匹配系统卷根本身，不把同盘的普通子目录称作系统卷。`is_protected()` 用于扫描产生的规范路径，按照目录边界匹配受保护位置；例如 `Windows\System32` 在 Windows 目录内，而 `Windows.old` 不是该目录的后代。位于其他盘上的已配置 Program Files 或 ProgramData 同样受到保护。

比较保留 UTF-16 原始单元，兼容普通 DOS、扩展 DOS、卷 GUID 和 UNC 表示；实际大小写比较使用 Windows `CompareStringOrdinal`。不把有损展示字符串用于路径判断。未知设备命名空间、相对路径、NUL、ADS 和含有 `.`/`..` 的非规范路径不能被据此判定为普通可处理路径。

已配置目录可能经过 junction。建立卷别名之前先解析其现有规范路径，原配置拼写也保留为受保护范围。这里的字符串/目录分类不代替执行阶段的句柄、卷与文件身份、权限及范围检查。

必要的系统位置或卷映射无法确认时，`query()` 返回 `None`；调用方应报告位置不可用，不能退回到可修改的环境变量猜测系统目录。调用方可缓存成功的系统位置快照，但不能把它当作授权缓存，也不应永久缓存查询失败。

可移植测试验证路径表示、目录分隔边界、盘符/卷区别、异常命名空间及 UTF-16 保留。Windows 专用测试另验证真实 OS 位置、卷 GUID/规范路径、Unicode 大小写、目录迁移及独立进程中的环境变量干扰。交叉编译不代表这些 Windows 原生用例已经运行。

## Windows 维护插件

`builtin.system-windows/recycle-bin` 通过 Shell 枚举当前用户的回收站。执行时绑定用户确认项目的 Shell 身份、所有者及时间，重新枚举后只永久删除该项目；已还原、已改变或后来放入的项目不会被替换清理。失败可能已有部分效果，不自动重试。

`builtin.system-windows/component-store` 使用系统 DISM `/Online /Cleanup-Image /StartComponentCleanup /NoRestart` 清理过时的 Windows 更新组件。需要 UAC 时通过已签名的清理组件（能力协议 6）。不使用 `/ResetBase`，不停止更新服务，不将 SoftwareDistribution、WinSxS 或 CbsTemp 当普通文件删除。维护动作不提供容量估计，完成后可能需要重启；它不代表清空所有下载的更新。两个处理器均默认供用户自行选择。

`builtin.memory` 是独立的 Windows 工作集整理操作，支持手动选择及按可执行路径的方案规则，不进入磁盘释放统计。只列出可打开的当前用户非关键进程；选择绑定 PID 和创建时间，调用 `K32EmptyWorkingSet` 时保持进程句柄。不会结束进程或请求管理员权限。显示的是观察到的工作集下降，不保证实际空出等量物理内存，应用可能立即重新加载页面。

`SystemLocations::is_reviewable_cache` 只识别系统目录下的 Temp、SystemTemp 以及 ProgramData 下的 ChocolateyHttpCache。分类不提供权限，不绕过路径身份及重解析点检查，其他系统维护目录继续受保护。
