# CLI 使用与配置速查


Windows 上因占用而跳过的项目会显示具体被锁文件，以及系统 Restart Manager 能识别到的程序名称和 PID。这些是失败时的诊断快照，不用于终止进程。系统无法识别占用者时，明确提示未知并保留路径与原生原因；其他平台同样保留该回退。结构化失败中的 `file_usage` 为可选字段，兼容旧插件和历史记录。
终端按原因汇总未完成项目，每组给出数量、未清理大小估算与处理建议。目录已经部分清理时单独说明；仅扣除口径一致且已确认的处理量，避免重复统计。文件占用不会反复申请管理员授权。完整路径和原生诊断仍可在 JSON 输出中查看。Windows 后台工具进程使用无窗口创建标志，交互式授权保持可见。Windows 11 可选使用原生标题栏颜色，旧系统保留系统装饰。

明确因文件占用而无法完成的项目，会汇总为“正在使用，已跳过”，普通终端不再重复打印原生错误；可关闭相关程序后重新扫描，清理不会强制关闭程序或为占用问题重复申请管理员权限。授权失败、处理器异常和结果不确定仍按错误处理。`--output json` 保留原始结果、失败详情与退出语义，并附带 `unfinished_items` 中按扫描结果估算的未清理大小。该逻辑内容大小与已处理量、实际释放空间分开统计；部分文件系统项目在计量口径一致时扣除已确认处理量，无法计量或数据矛盾时保留未知。

本文保留完整命令接口、终端操作和常用配置示例。首次使用请先看 [项目快速开始](../../../README.zh-CN.md#快速开始)。
所有配置字段、完整个人配置和常见错误见 [配置参考](CONFIGURATION.md)。AI 分析与账号同步在桌面端使用。

## 固定命令接口

不指定模式时进入交互选择。`scan` 只读扫描，`clean` 清理推荐项并要求确认；只有明确传入 `--yes` 才跳过该确认。

```text
spaceprune [module] [module selectors] [--scan|--clean|--select] [options]
spaceprune scan [--module <id>] [--selector <id>...] [--task <id>...] [options]
spaceprune clean [--module <id>] [--selector <id>...] [--task <id>...] [options]
spaceprune authorization status
spaceprune authorization install [--graphical]
spaceprune module list [--all]
spaceprune plugin <catalog|list|inspect|install|remove|enable|disable>
spaceprune automation <list|install|uninstall|run>
spaceprune recipe
spaceprune recipe list
spaceprune recipe set <recipe-id>
spaceprune recipe follow
spaceprune theme
spaceprune theme list
spaceprune theme set <theme-id>
spaceprune language [list|set <system|en|zh-CN>]
```

`spaceprune module list` 按配置顺序列出已启用模块，增加 `--all` 后同时列出禁用模块。`module`、`plugin`、
`automation`、`recipe`、`theme`、`language`、`authorization`、`scan` 和 `clean` 是固定命令保留字，不能用作模块 ID；其他模块中的 `--all`
仍按动态 selector 解析。
固定选项名 `config`、`scan`、`clean`、`select`、`yes`、`output`、`no-progress`、`help` 和 `version`
不能用作 selector ID。

`scan` / `clean` 子命令是不会随 Recipe 模块名变化的稳定脚本入口：`--module` 精确指定一个模块，重复
`--selector` 按标签 AND 过滤任务，重复 `--task` 精确限定任务；`--selector` 和 `--task` 都要求同时提供
`--module`。动态的 `spaceprune cache --scan` 形式继续保留，适合人类发现和调用配置模块。固定子命令不能再与
旧式 `--scan`、`--clean` 或 `--select` 模式标志组合。

主要选项：

| 选项                     | 作用                           |
| ------------------------ | ------------------------------ |
| `--config <PATH>`        | 使用指定应用入口 `config.toml` |
| `--scan`                 | 只扫描                         |
| `--clean`                | 清理推荐项                     |
| `--select`               | 交互选择                       |
| `--yes`、`-y`            | 跳过清理前的确认，仅用于 `clean` / `--clean` |
| `--output <human\|json>` | 选择输出格式                   |
| `--no-progress`          | 关闭诊断流中的进度输出         |

各子命令的帮助只展示适用选项。方案、主题、语言和插件管理等不会列出清理模式或 `-y`；
`automation run` 保留 `--no-progress`，只读扫描不提供 `-y`。系统授权与清理业务确认相互独立，`-y` 不会绕过系统授权。

扫描或清理期间按 Ctrl+C 会协作式取消剩余工作并返回退出码 130；JSON 报告中的 `cancelled` 同步为 true。
终端中临时切换到独立进度画面，显示当前操作、运行时间、已返回的检查／清理数量和当前步骤提供的真实计数。
工具暂时没有新输出时，耗时和动画仍会更新，并提示仍在工作；没有可靠总量时不显示猜测的百分比。
Snapper 逻辑大小统计与 `delete --sync` 的 Btrfs 同步等待会分别说明。停止后若当前系统命令尚未返回，
会显示正在等待当前操作结束；已完成的删除不会撤销，也不会因停止而重试删除命令。

系统密码输入前回到原终端并暂停动画，保留授权说明；输入结束后恢复进度画面。有效 sudo 认证直接复用，不为刷新进度重复请求密码。
扫描结束后回到原终端并留下耗时摘要，再进入选择／确认；清理结束后恢复命令历史并显示最终结果。
缩放时只重绘独立进度画面，低高度窗口减少显示行数，扫描前的命令和授权记录保留。进度不读取键盘或改变回显，只写入 stderr，stdout 的 JSON 报告保持可解析；
重定向 stderr 或使用 `TERM=dumb` 时输出普通日志，`--no-progress` 关闭全部进度和阶段摘要。

交互选择界面使用 `h/l` 收缩或展开当前模块/任务，使用 `-`/`+` 收缩或展开全部模块。进入界面时终端标题
临时显示为 `SpacePrune — Interactive cleanup`，退出时恢复原标题。

选择清单的 `Logical size` 列显示文件逻辑大小。Snapper 的快照会只读统计文件与目录元数据，不读取文件正文；这项统计可能需要一段时间，进度会显示正在统计逻辑大小。快照内部目录权限不足时按方案的 `elevation` 请求授权；只读取 Snapper 列表已有权限时不要求密码，sudo 授权仍有效时可以复用。拒绝授权或统计未完成时保留 `unknown`，仍可查看快照信息。

分组大小完整时显示普通数值；只有部分项目可统计时用 `+` 标注已知小计，全部未知时显示 `unknown`，不会把未知容量显示成 `0 B`。Snapper 的 `#0` 表示当前系统，而不是可清理的历史快照。插件将其命名为“当前系统 (#0)”并标记逻辑大小不适用；CLI 显示“不适用”（英文为 `N/A`），不计入未知项。只有当前系统的任务和模块也显示不适用。真实历史快照统计失败时仍显示未知，普通空目录仍显示 `0 B`。

顶部选择汇总和清理确认将普通项目的预计处理量与已选快照组数分开显示；快照不会混入普通项目的未知数。分类来自插件的计量元数据，外部插件可采用同样的快照分组方式。快照间可能共享数据块，表中的逻辑大小不能作为实际释放空间，条目详情会说明区别。

普通项目仍有未知处理量时，按 `U` 定位下一项已选的未知项目；界面自动展开其模块与项目，必要时清除搜索过滤，但不改变勾选。详情显示原路径及处理量估计，便于判断；项目和模块行仍按原逻辑大小显示。

清理完成后，普通文件继续显示处理大小与未知项目数；Snapper 快照不计入普通文件的未知数，单独显示成功处理的快照组数（pre/post 对算一组）。每个实际删除批次还显示所在卷空闲空间的净变化，例如 `root: +2.00 GiB`。这是删除前与 `snapper delete --sync` 返回后的测量差值，同卷其他读写也会影响结果；负数表示空闲空间减少，`+0 B` 表示没有净变化。无法测量时单独显示 `unavailable` 和原因，不改变删除结果，也不把快照重新算入普通文件未知项。

执行前复核时已经不存在的快照单独显示 `already absent`，不会算成本次新删除；这些快照本次额外释放为 `0 B`，不会因未执行删除测量而显示 `unavailable`。如果同批还有实际删除，仍保留实际删除批次的卷空间变化。

`--output json` 保留原始逐项结果，并增加 `cleanup_size_summary`。已不存在的快照组数位于 `snapshot_already_absent_group_count`，快照的批次测量位于 `snapshot_space_changes`，单次合并删除只记录一次，不按任务数重复累计。

Linux 普通文件清理遇到权限不足时，会汇总本次已确认的受影响项目，按方案设置请求系统授权。已有 sudo 认证有效时直接复用，过期时才出现系统密码提示；拒绝后不逐文件重复请求。Snapper 使用 sudo 时取得的有效认证也可以复用，读取列表或只读检查权限本身不会赋予删除权限。可以在 filesystem 任务的 options 中设置 `elevation = "never"`，明确禁止自动提权。

已知未完成原因每类只显示一行：数量、未清理大小估算和简短处理建议。占用项目另列路径和可用的程序名称；完整原生诊断保存在 JSON 中，不重复解释权限。部分清理项目只估算剩余大小。扫描选择页汇总无法读取或正在使用的项目，其余可读项目仍可选择；其他原因的错误继续显示。清理结果未确认的项目应重新扫描核对。

## Linux 系统授权组件

普通文件与 Snapper 扫描先使用当前权限，实际遇到权限不足才请求系统只读授权；有效连接可供后续检查复用。
前台操作确需权限时，会先验证并按需安装或更新随包组件，成功后继续原操作。准备或授权失败时会显示具体原因。以下命令用于检查状态或手动维护，不是普通扫描的前置步骤：

```bash
spaceprune authorization status
spaceprune authorization install
```

`status` 不申请权限，支持 `--output json`，就绪返回 0、未就绪返回 1；检查与安装不依赖当前方案或配置能否加载。
`install` 从 CLI 相邻目录取得同版读取、清理与安装组件，通过 Core 安装器将整套组件安装到
`/usr/libexec/spaceprune/native-6-4`，然后校验 root 所有权、路径权限、摘要与各自协议。
终端使用前台 sudo 隐藏密码输入，Ctrl+C 可取消；桌面会话可用 `install --graphical` 触发系统认证窗口。非交互运行仅尝试已有系统认证。
普通可读任务和非交互任务不自动安装。
读取权限与清理权限仍按用途分别申请。Desktop 设置中的“安装/更新系统授权组件”使用同一个安装服务，作为维护入口保留。
源码用户先构建 CLI 与相邻的三个组件：

```bash
cargo build --locked -p spaceprune-cli -p spaceprune-readonly-helper
./target/debug/spaceprune authorization install
```

## 共享显示语言

CLI 与桌面共用应用 `config.toml` 的顶层 `language` 字段。缺省 `system` 跟随系统，支持英文 `en` 和简体中文 `zh-CN`；系统语言不支持或无法识别时使用英文。手动设置优先于系统语言。

```bash
spaceprune language
spaceprune language list
spaceprune language set zh-CN
spaceprune language set en
spaceprune language set system
```

`language` 只读显示偏好与当前解析语言，`list` 列出 `en`（English）、`zh-CN`（简体中文）及独立的 `system` 跟随系统模式，`set` 保存共享偏好；可用 `--config PATH` 指定应用入口。读取不依赖活动 Recipe 是否可用，语言不会写入 Recipe。机器 JSON 保持稳定键、ID 和原有英文产品消息；路径、用户名称、第三方插件文本与系统原始输出保留原样。

## 终端主题

CLI 与桌面共用四种风格，默认使用极光。终端采用对应的深色配色：

| 主题 | ID | 风格 |
| ---- | ---- | ---- |
| 极光 | `aurora` | 冰蓝与青色 |
| 星轨 | `orbit` | 紫色与玫红 |
| 石墨 | `graphite` | 冷蓝与青灰 |
| 苔原 | `forest` | 柔和绿色 |

```bash
spaceprune theme
spaceprune theme list
spaceprune theme set orbit
spaceprune --config /absolute/path/to/config.toml theme set forest
```

直接运行 `spaceprune theme` 会进入预览界面：方向键或 `1`–`4` 切换主题时，完整界面和状态色立即重绘；
Enter 保存，Esc 取消。`theme set <ID>` 直接设置，`theme list` 只读列出主题和颜色样本。
`theme --output json` 直接返回当前主题，不进入选择界面，也不修改配置。

主题用于命令帮助与错误、普通文本输出、进度和交互选择界面。JSON 与重定向输出保持纯文本。
CLI 主题保存在应用配置的 `[theme]` 段中；保存会保留 `config.toml` 的其他内容和注释。
`theme list --output json` 可用于脚本读取当前状态：

```toml
[theme]
id = "aurora"
```

CLI 和桌面主题独立保存。桌面“设置”中可以分别选择两者；修改 CLI 主题不会改变桌面外观，修改桌面主题
或明暗模式也不会改变 CLI 配色。

旧配置仍可直接启动：Tokyo Night、Nord 对应极光；Catppuccin Mocha、Dracula、Monokai 对应星轨；
VS Code Dark Modern、Light Modern、Quiet Light 对应石墨；Solarized Dark 对应苔原。
读取不会改写原配置，再次保存时使用新的四种 ID。旧主题不再出现在选择器中。

## 配置发现

`--config <PATH>` 选择应用入口 `config.toml`，不是直接选择 Recipe 文件。未指定时按平台发现默认入口；没有用户配置时加载 `official:minimal`。默认位置、XDG 优先级和完整目录布局见[配置发现](CONFIGURATION.md#2-默认配置发现)。

## recipe 选择

```bash
spaceprune recipe
spaceprune recipe list
spaceprune recipe set personal
```

直接运行 `spaceprune recipe` 会交互预览并选择 `recipes` 下符合 `<ID>/<ID>.toml` 约定的方案；Enter 保存，Esc 取消。
`recipe set <ID>` 直接切换，且只接受当前 `recipes` 下实际存在的方案；`recipe list` 会显示名称、入口路径和活动状态。
选择保存在 `config.selection.toml`，只覆盖 CLI 使用的方案，不改变默认方案及自动清理配置。未手动覆盖时，CLI 跟随图形应用最近使用的方案；没有使用记录时采用默认方案。`recipe follow` 恢复跟随；`--config` 选择另一份配置及其对应的选择记录。已删除的方案会回退到仍存在的选择或默认方案。
`recipe --output json` 直接返回当前方案 ID、名称、入口与配置路径，不打开选择界面、不写配置；
`--recipe <ID>` 仅覆盖本次命令使用的方案，不改变已保存的选择。安装后的系统计划携带此参数，切换默认方案不会改变已有计划。显式指定的方案不存在时直接报错，不回退到其他方案。

`recipe list --output json` 返回方案列表，`recipe set <ID> --output json` 则明确执行切换并返回结果。

## 内置最小方案

仓库只保留一个内置清理 recipe：

```text
recipes/official/minimal.toml  ->  official:minimal
```

内嵌 recipe 命名空间保持单层，当前不定义额外分类目录。

它提供离线且可预测的开箱即用行为：

| 平台                          | 基础任务                   | 推荐条件    |
| ----------------------------- | -------------------------- | ----------- |
| Linux / macOS / Android / iOS | 当前用户缓存目录的直接子项 | 30 天未修改 |
| Windows                       | 当前用户临时目录的直接子项 | 7 天未修改  |

`official:minimal` 不包含特定 Agent、编程语言、包管理器、快照工具或自动化。Snapper、Linux 系统缓存
等能力仍可由用户配置调用，但不会默认进入清理范围。

用户 recipe 若要保留最小方案，应在自己的入口文件中显式 include：

```toml
schema-version = 2
name = "Personal SpacePrune"

include = ["official:minimal", "./personal.generated.toml"]
```

省略 `official:minimal` 表示使用完全独立的方案。

## 本地 recipe

SpacePrune 使用“最小内置 + 显式本地扩展”：

1. 二进制只编译 `official:minimal`；
2. `config.toml` 只按 ID 选择一个 `recipes/<ID>/<ID>.toml`；
3. 选中的 recipe 可以继续 include 同目录中的本地 TOML；
4. SpacePrune 只读取当前 recipe 明确引用的本地文件。

### 完整方案

一个 Recipe 包含 `recipes/<ID>/` 下全部 UTF-8 配置文件。导入、导出与原子替换约束见[配置参考](CONFIGURATION.md#14-recipe-bundle)。在另一台电脑导入后，应重新扫描，确认当前路径与命中内容。

## 编写清理规则

规则、变量映射、include 和推荐条件集中在[Recipe 编写指南](RECIPE-GUIDE.md)，完整字段与专用 Handler 选项见[配置参考](CONFIGURATION.md)。先用 `spaceprune scan --config <PATH> --output json --no-progress` 检查命中，再进入交互选择；不要用清理命令验证新规则。

## 自动化

```toml
[automations.daily]
name = "Daily cleanup"
modules = ["cache"]
at = "03:00"
```

```bash
spaceprune automation list
spaceprune automation install daily
spaceprune automation run daily
spaceprune automation uninstall daily
```

Linux 使用 systemd user unit，macOS 使用 LaunchAgent，Windows 使用当前用户计划任务。`official:minimal`
不预设自动化。安装内容固定引用应用 `config.toml`，每次触发时重新解析当前 Recipe；直接加载单个 Recipe、
没有应用入口的核心会话不会安装一个无法重载的自动化任务。核心安装 API 要求宿主显式传入 CLI runner；
CLI 使用自身路径，GUI 宿主不能把当前 GUI 可执行文件当作定时清理 runner。

后台运行没有权限时不会停下来等待密码。可处理的推荐项仍会执行，受限内容保留；请在终端手动运行同一个计划，授权后重新扫描剩余内容。
`automation run --output json` 的 `status` 区分 `completed`、`completed_with_issues`、`needs_authorization`、`failed` 和 `cancelled`。
只有无问题的 `completed` 返回 0；扫描警告、待授权和失败返回 1，用户取消返回 130，超时返回 1。扫描阶段停止时 `plan` 和 `execution` 为 null。
执行报告始终保留实际成功项与已处理大小；待授权不表示此前的处理被回滚。普通 `scan` 的部分结果退出规则不受影响。

## 内置 Handler 与外部插件

普通路径需求优先使用 `builtin.filesystem/paths`。需要领域 API、特殊分组或系统命令时使用专用 Handler。

静态注册的完整能力：

| Handler ID                           | 平台   | 用途                                             |
| ------------------------------------ | ------ | ------------------------------------------------ |
| `builtin.filesystem/paths`           | 全平台 | 受 root/include/exclude 边界约束的普通文件与目录 |
| `builtin.snapper/snapshots`         | Linux  | Snapper 快照发现、保护、分组与删除               |
| `builtin.system-linux/package-cache` | Linux  | zypper、apt/apt-get、dnf 或 pacman 软件包缓存    |
| `builtin.system-linux/journal`       | Linux  | systemd journal vacuum                           |
| `builtin.system-linux/tmpfiles`      | Linux  | systemd-tmpfiles 清理                            |
| `builtin.system-linux/coredumps`     | Linux  | 按 systemd-tmpfiles 保留规则清理 coredump        |
| `builtin.system-linux/go-modcache`   | Linux  | Go module cache                                  |
| `builtin.developer-tools/go-modcache` | Linux | Go 模块缓存 |
| `builtin.developer-tools/go-buildcache` | Linux | Go 构建缓存 |
| `builtin.developer-tools/cargo-registry-cache` | Linux | Cargo 注册源下载缓存 |
| `builtin.developer-tools/npm-cache` | Linux | npm 内容寻址缓存 |
| `builtin.developer-tools/docker-build-cache` | Linux / Windows / macOS | 构建缓存 |
| `builtin.developer-tools/docker-dangling-images` | Linux / Windows / macOS | 悬空镜像 |
| `builtin.developer-tools/docker-unused-images` | Linux / Windows / macOS | 未使用的有标签镜像（手动可选） |
| `builtin.developer-tools/docker-stopped-containers` | Linux / Windows / macOS | 已停止容器（手动可选） |

`official:minimal` 只调用 filesystem Handler；其余 6 个必须由 Recipe 显式引用。每个专用 Handler 的 options、
recommend 与 unavailable 语义见[配置参考](CONFIGURATION.md#12-专用-handler)。
系统缓存扫描先使用当前权限，遇到权限不足时由共享只读服务按需授权。仍有不可读子树时保留可读容量，并将显示值标记为下界；扫描不会执行清理。

外部插件使用本地目录安装：

```bash
spaceprune plugin catalog
spaceprune plugin disable builtin.snapper
spaceprune plugin enable builtin.snapper
spaceprune plugin install /path/to/plugin
spaceprune plugin list
spaceprune plugin inspect community.example
spaceprune plugin remove community.example
```

`plugin list` 按“内置插件”和“外部插件”分组，以表格显示名称、插件 ID、适用平台和启用状态。
Snapper 和 Linux 系统清理属于内置插件，Linux 是它们的平台属性；不再单列“平台能力”。列表和 Desktop 使用同一个插件目录，停用或不支持当前平台的插件仍显示实际状态。

`plugin catalog --output=json` 统一返回内置插件、外部插件、完整 Handler ID、版本、平台支持状态和本机启停状态。
`plugin list/inspect --output=json` 保留原外部插件元数据格式。目录读取不会启动插件或申请系统权限。
若已有外部安装冒用内置 ID，目录显示 `id conflict`，并保留同名真实内置能力。冲突包不能运行，
可通过 `plugin remove <id>` 仅删除该外部安装；新的同名外部安装会被拒绝。

`plugin disable/enable <id>` 对后续操作生效，由 CLI 与 Desktop 共用用户配置目录下的 `plugins.toml`，
不会改写 Recipe，也不受 `--config` 选中的方案影响。缺少该文件时全部启用。一般扫描跳过已停用任务；
明确指定停用任务则提示 `plugin-disabled`。改变启停后，已加载会话和旧扫描的清理入口会要求重新加载、重新扫描，
重复保存相同状态不会失效。已开始的操作不会被中途强制终止。


进程协议详见 [插件协议](../reference/PLUGIN-PROTOCOL.md)。

文件系统任务行（例如同名的 `target` 文件夹）获得焦点时，Details 会显示该任务配置中的完整根路径；
有多个根时分别列出。文件和目录清理项仍显示自己的 `Location`。路径仅用于展示，不改变勾选范围或执行计划。
模块、项目/文件夹、文件分别使用主题主色、辅色和浅色文字；每行已知的逻辑大小与该行名称同色。
勾选或移动焦点不会改变层级颜色：勾选框和状态列单独表达全选、部分选中、未选和不可用。
全选的标记与状态词使用绿色，部分选中标记使用琥珀色；右侧的选择数量和已知选中大小与左侧名称、逻辑大小同色。
未知大小使用琥珀色，未选、不可用和“不适用”使用次要文字色。
Details 的路径、选择数量和已知数值沿用当前层级颜色，选择状态词和原因沿用表格中的状态色，字段标签和说明弱化显示。
底部快捷键区分按键和动作，窄屏优先显示主要操作，其余快捷键可按 `?` 查看。
四主题与桌面使用同一配色家族，主题选择仍分别保存。

扫描实时显示已统计内容大小及符合推荐条件的大小；清理随确认结果更新已处理大小估算。Windows 扩展路径前缀只在用户可见文本中隐藏，JSON 和底层文件身份保持原样。
