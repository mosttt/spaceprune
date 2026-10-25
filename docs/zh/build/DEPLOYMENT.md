# Core 与 CLI 部署指南

本仓库的 Core 与 CLI 开源，自有代码使用 [MIT License](../../../LICENSE)。本页说明已经构建好的 CLI 如何安装、
升级和与个人配置配合使用。各平台工具链、debug/release 构建、六种目标和归档制作统一见
[构建、打包和分发](BUILDING.md)，GitHub 发布流程见 [CI 与 Release](CI.md)。

离线 CLI 不需要账号、模型服务或数据库。

## 1. 安装已构建的 CLI

选择与目标操作系统和 CPU 架构一致的归档，核对发布方提供的 SHA-256 后解压。
Linux/macOS 可执行文件名为 `spaceprune`，Windows 为 `spaceprune.exe`。
可以从解压目录直接运行，也可以把完整运行目录加入目标用户的 `PATH`。不要仅复制 CLI：Linux 需保留三个相邻 helper，
Windows 需保留 `SpacePruneNative.msi`，macOS 需保留完整 `SpacePruneAuthorization.app`。源码安装方式见
[安装本地 CLI](BUILDING.md#4-安装本地-cli)。

先执行 `spaceprune --version`、`spaceprune --help`，确认实际运行的是刚安装的版本，再执行只读的
`spaceprune scan --no-progress`。扫描结果来自目标用户自己的配置与权限；管理员账户上的结果不能代替普通用户验收。

正式工作流为 Windows 载荷提供 Authenticode、为 macOS 载荷执行代码签名和公证，再生成归档及校验和；
CLI 没有图形安装器，Windows 归档中的 MSI 仅安装原生权限组件。签署流程是否成功以本次发布记录和实际文件为准。
只读扫描和终端交互验证应分别在目标平台进行；涉及删除的验证只使用专门准备的测试数据。

## 2. 二进制与个人配置分开保存

个人设置和 Recipe 不放在发行归档中。运行时由 CLI 按平台发现应用 `config.toml`，再加载
`recipes/<ID>/<ID>.toml`。位置、XDG 优先级、Windows/macOS 路径及完整字段以
[配置参考](../usage/CONFIGURATION.md#2-默认配置发现) 为准。

配置目录通常包含：

```text
spaceprune/
├── config.toml
└── recipes/
    └── personal/
        ├── personal.toml
        └── variables.toml
```

`config.toml` 保存当前 Recipe、Desktop 与 CLI 共享的 `language` 偏好，以及独立的 CLI 主题。清理方案、变量映射和 include 文件保存在各自 Recipe 目录；
桌面外观由 Desktop 单独保存。升级 CLI 二进制不会自动重写这些个人文件。

使用独立测试配置时传入 `--config <PATH>`，不要为了测试覆盖真实用户的配置。
从其他设备复制 Recipe 后，先检查路径映射，再在目标机器只读扫描；同一个配置文件不保证在另一台电脑上命中相同内容。

## 3. 运行时工具与权限

普通文件清理在当前用户权限下执行。Recipe 使用 Snapper 或 Linux 系统清理 Handler 时，目标系统还需要对应的
工具。相关可选工具、Handler 和参数见 [CLI 使用与配置速查](../usage/CLI.md#内置-handler-与外部插件)。
工具不存在或权限不足时，应检查具体返回结果，不能把没有读取到的范围当作已经检查或清理成功。

Snapper 删除使用用户明确选择的快照，并通过系统授权执行；编译 CLI 不会预先授予这些权限。
Linux 的授权组件由 Core 构建，CLI 和 Desktop 共用系统安装位置。Linux 归档中的
`spaceprune-readonly-helper`、`spaceprune-cleanup-helper`、`spaceprune-helper-installer` 应与 `spaceprune` 保持相邻且版本一致；移动 CLI 时一起移动组件。
运行 `spaceprune authorization status` 检查，`spaceprune authorization install` 显式安装或更新系统副本。
确需权限的交互式操作会自动准备缺失、过旧或与随包版本不同的组件；后台任务不会弹窗安装。详见 [系统授权组件](../usage/CLI.md#linux-系统授权组件)。

`authorization status/install` 管理命令仅支持 Linux。Windows 由前台操作安装并核对固定随包 MSI，
macOS 由签名授权 App 注册／更新固定服务；安装批准不等于读取或删除批准。平台条件分别见
[Windows 组件打包](WINDOWS-PACKAGING.md)和 [macOS 组件打包](MACOS-PACKAGING.md)。

macOS 已接入父目录 generation 和持续保留的 vnode 事件，检测到祖先移动或观察失败时停止删除并走恢复路径。
原生 APFS/HFS 验收以及 APFS 在途 rename 的同步依据仍待完成；当前清理测试使用隔离生成的数据，不能将签名安装成功当作删除边界已通过。验证命令见[实现与原生验收](../reference/MACOS-ANCESTRY.md)。
macOS 也没有用户可调用的完整原生服务卸载入口，删除 CLI 或授权 App 文件不等于注销系统服务。

外部插件是独立运行时扩展，不会因为编译 CLI 而自动打入发行归档。安装、权限和协议要求见
[插件协议](../reference/PLUGIN-PROTOCOL.md)。

## 4. 自动化与升级

自动化使用 Linux systemd 用户服务、macOS LaunchAgent 或 Windows 当前用户计划任务。
安装前，用目标用户和实际应用 `config.toml` 手动完成只读扫描，再检查自动化选择的模块、时间和启用状态。
具体命令与配置统一见 [自动化](../usage/CLI.md#自动化)。

安装的任务记录 CLI runner 的路径和应用配置路径。二进制移动或重命名后，需要重新安装相关自动化以更新路径；
不要让计划仍指向临时构建目录或已删除的旧版本。自动清理会按已安装计划执行推荐规则，不会每次弹出手动清理确认。

升级前保留正在使用的完整发行目录及个人配置备份；等待当前扫描或清理结束后再替换同批 CLI 和随包组件。
升级后重新检查版本、加载现有配置并运行只读扫描。回退程序版本不意味着可以自动回退已执行的文件删除，
也不意味着旧版本必然理解新版本保存的所有规则。

## 5. 分发材料

对外分发 Core/CLI 二进制时保留本仓库 MIT License、随制品提供的 Rust 依赖与标准库 notice；
musl 归档还需要本次实际链接的 musl notice。材料生成与归档文件清单见
[构建、打包和分发](BUILDING.md#6-许可证签名和正式发布)。

嵌入 Core 的其他应用应按自身依赖和分发物保留相应许可材料。
