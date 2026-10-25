# SpacePrune 配置参考

SpacePrune 使用 TOML Schema 2，并明确区分应用配置与清理方案：`config.toml` 保存当前 recipe、共享语言和 CLI 主题，
`recipes/<ID>/<ID>.toml` 描述实际清理内容。

## 1. 应用配置

```toml
schema-version = 2
recipe = "personal"
language = "system"

[theme]
id = "aurora"
```

| 字段             | 类型    | 必需 | 说明                         |
| ---------------- | ------- | ---- | ---------------------------- |
| `schema-version` | 整数    | 是   | 当前固定为 `2`               |
| `recipe`         | 稳定 ID | 是   | 当前使用的本地清理方案       |
| `language`       | 字符串 | 否 | `system`（默认）、`en` 或 `zh-CN`，CLI 与桌面共享 |
| `theme.id`       | 稳定 ID | 否   | 终端主题；默认 `aurora` |

`language` 缺省跟随系统；未支持或无法识别的系统语言、未知的已保存语言值均回退英文。`spaceprune language set <system|en|zh-CN>` 保存手动偏好；读取语言不加载活动 Recipe。

`theme.id` 可选 `aurora`（极光）、`orbit`（星轨）、`graphite`（石墨）、`forest`（苔原）。
桌面主题和明暗模式由桌面端单独保存，不受此字段影响；两者也可以在桌面“设置”中分别修改。
旧主题 ID 读取时自动对应到新风格，不会改写配置；对应关系见 [终端主题](CLI.md#终端主题)。

`recipe = "personal"` 固定解析为相对应用配置目录的 `recipes/personal/personal.toml`。recipe ID 同时是目录名和
入口文件名，不再配置任意入口路径。

## 2. 默认配置发现

`--config <PATH>` 始终优先。未指定时按顺序选择第一个已存在的文件；都不存在时首次写入使用第一候选位置：

| 平台            | 路径                                                                                                                   |
| --------------- | ---------------------------------------------------------------------------------------------------------------------- |
| Linux / Android | `$XDG_CONFIG_HOME/spaceprune/config.toml`；`$HOME/.config/spaceprune/config.toml`                                      |
| macOS / iOS     | `$XDG_CONFIG_HOME/spaceprune/config.toml`；`$HOME/Library/Application Support/SpacePrune/config.toml`                  |
| Windows         | `%APPDATA%\SpacePrune\config.toml`；仅当 `APPDATA` 不可用时使用 `%USERPROFILE%\AppData\Roaming\SpacePrune\config.toml` |

空的 XDG_CONFIG_HOME、HOME、APPDATA 或 USERPROFILE 会被忽略。找不到应用配置时加载 `official:minimal`。

每个候选根目录内部都使用同一布局。Linux 未设置 `XDG_CONFIG_HOME` 时，全部清理文件位于：

```text
~/.config/spaceprune/
├── config.toml
└── recipes/
    └── personal/
        ├── personal.toml
        ├── personal.generated.toml
        └── variables.toml
```

`personal.toml` 是 recipe 入口。它的 `variable-map` 与本地 `include` 都相对于
`recipes/personal/` 解析，因此每个 recipe 可以单独复制、备份和审查。

清理方案入口示例：

```toml
schema-version = 2
name = "Personal SpacePrune"
variable-map = "./variables.toml"

include = [
  "official:minimal",
  "./personal.generated.toml",
]
```

| 字段               | 类型               | 必需 | 说明                                 |
| ------------------ | ------------------ | ---- | ------------------------------------ |
| `schema-version`   | 整数               | 是   | 当前固定为 `2`                       |
| `name`             | 字符串             | 是   | 方案显示名称                         |
| `variable-map`     | 字符串路径         | 否   | 用户变量映射；只能出现在 recipe 入口 |
| `include`          | 字符串或条件表数组 | 否   | 先加载并合并的配置                   |
| `requires.plugins` | 字符串映射         | 否   | 插件 ID 到 SemVer 要求               |
| `modules`          | 表                 | 否   | 动态模块                             |
| `automations`      | 表                 | 否   | 自动化定义                           |

recipe 入口和所有 include 文件都必须各自声明整数 `schema-version = 2`。SpacePrune 在合并前逐个验证物理文件，
因此 include 文件不能继承入口版本字段，也不是可省略元数据的 TOML 片段。

选择用户 recipe 后不会隐式合并内嵌方案。需要基础任务时在 recipe 入口显式 include `official:minimal`。

配置与主题选择命令：

```bash
spaceprune recipe
spaceprune recipe list
spaceprune recipe set personal
spaceprune theme
spaceprune theme list
spaceprune theme set aurora
```

裸 `recipe` 和 `theme` 命令进入交互选择；`set <ID>` 直接切换；`list` 只读列出。两种设置都会只更新应用
`config.toml` 中对应字段并保留其他内容和注释。默认应用配置不存在时，首次设置主题或语言会同时创建
`recipes/default/default.toml`，内容 include `official:minimal`。

手工方案的文件布局、平台映射和验证步骤见 [Recipe 编写指南](RECIPE-GUIDE.md)。
AI 会话和草稿生成由桌面应用维护；CLI 直接读取已保存的 Recipe。

面向脚本的固定扫描与清理接口不会依赖动态模块子命令名：

```bash
spaceprune scan --module personal --selector logs --output json --no-progress
spaceprune scan --module personal --task logs --output json --no-progress
spaceprune clean --module personal --selector logs --yes
```

`--selector` 和 `--task` 可重复，且都要求 `--module`。selector 将 `match-tags` 合并后按 AND 匹配；task 则按稳定
任务 ID 精确限定。固定 `scan` / `clean` 子命令不能和旧式 `--scan`、`--clean`、`--select` 标志混用。

## 3. 内嵌最小方案

唯一内嵌清理 recipe 是：

```text
official:minimal
```

源码位于 `recipes/official/minimal.toml`，通过 `include_str!` 编译进 `spaceprune-config`。运行已构建二进制时
不会读取源码目录。

方案包含一个 `cache` 模块：

| 任务             | 平台                       | root            | 推荐规则    |
| ---------------- | -------------------------- | --------------- | ----------- |
| `user-cache`     | Linux、macOS、Android、iOS | `${CACHE_HOME}` | 30 天未活动 |
| `user-temporary` | Windows                    | `${TEMP_HOME}`  | 7 天未活动  |

每个任务只选择 root 的直接子项。方案不包含特定应用、Agent、语言工具链、快照、系统管理命令或自动化。

## 4. include

普通 include：

```toml
include = ["official:minimal", "./common.toml"]
```

平台条件 include：

```toml
include = [
  { path = "./linux.toml", platforms = ["linux"] },
  { path = "./desktop.toml", platforms = ["macos", "windows"] },
]
```

规则：

- `official:<name>` 引用编译进二进制的资源；当前只有 `official:minimal`；
- 本地路径相对于声明 include 的文件；
- 官方资源只能 include 其他 `official:` 资源；
- 不匹配当前平台的条件项在解析路径和读取文件前跳过；
- 支持的平台名为 `linux`、`macos`、`windows`、`android`、`ios`；
- include 循环、未知平台、缺失文件和类型冲突都会失败；
- 非 `official:` include 必须是本地文件路径。

### 4.1 合并规则

1. include 按数组顺序深度优先解析；
2. 被 include 的内容先进入结果；
3. 当前文件最后覆盖；
4. 表递归合并；
5. 数组和标量整体替换；
6. 相同位置类型不同是错误。

数组不会自动拼接。覆盖 roots、tags、platforms 或 modules 数组时必须写出完整结果。

`enabled = false` 可关闭被 include 的模块、选择器、任务或自动化。

## 5. 变量映射

清理配置中的 `${NAME}` 是逻辑变量，不会直接读取同名环境变量。环境变量名、平台目录和 fallback
由独立映射文件维护。

内置映射源码：

```text
mappings/official/variables.toml
```

它同样通过 `include_str!` 编译进二进制。

### 5.1 用户映射文件

recipe 入口通过 `variable-map` 指向用户映射：

```toml
schema-version = 2
name = "Personal SpacePrune"
variable-map = "./variables.toml"
```

`variables.toml`：

```toml
schema-version = 2

[variables.TOOL_HOME]
environment = "TOOL_HOME"
fallback = "${HOME}/.tool"

[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"

[platforms.windows.variables.TOOL_CACHE]
value = "${CACHE_HOME}/Tool/Cache"
```

`variable-map`：

- 只能出现在 recipe 入口；
- 相对于 recipe 入口所在目录解析；
- 必须是本地文件；
- 路径本身不进行 `${NAME}` 或 `~` 展开；
- 省略时仅使用内置映射。

### 5.2 变量定义

环境来源：

```toml
[variables.TOOL_HOME]
environment = "TOOL_HOME"
fallback = "${HOME}/.tool"
```

固定或组合值：

```toml
[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"
```

每个变量必须二选一：

- `environment`：读取指定进程环境变量，并按字面量使用其值；
- `value`：使用配置值。

`fallback` 只能与 `environment` 一起使用。环境变量缺失或为空时才使用 fallback。`value` 和 `fallback`
都可以引用其他映射变量。

变量名必须以 ASCII 字母或下划线开头，并且只包含 ASCII 字母、数字和下划线。environment 名必须非空，
且不能包含 NUL 或 `=`；因此可以映射 Windows 的 `ProgramFiles(x86)` 等名称。

### 5.3 平台覆盖

```toml
[variables.ROOT]
value = "${HOME}/common"

[platforms.linux.variables.ROOT]
value = "${DATA_HOME}/linux-root"

[platforms.windows.variables.ROOT]
value = "${DATA_HOME}/WindowsRoot"
```

活动定义按以下顺序合并，后者优先：

1. 内置公共变量；
2. 内置当前平台变量；
3. 用户公共变量；
4. 用户当前平台变量。

同名变量按完整定义替换，不做字段级拼接。因此用户覆盖一个变量时，应写出该变量完整的 `environment` /
`value` / `fallback` 定义。

映射文件的平台名只接受 `linux`、`macos`、`windows`、`android`、`ios`。

### 5.4 内置逻辑变量

| 变量          | Linux / Android                                      | macOS / iOS                           | Windows                                             |
| ------------- | ---------------------------------------------------- | ------------------------------------- | --------------------------------------------------- |
| `HOME`        | `HOME` 环境变量                                      | `HOME` 环境变量                       | `USERPROFILE`                                       |
| `CONFIG_HOME` | `XDG_CONFIG_HOME`，fallback 为 `${HOME}/.config`     | `${HOME}/Library/Application Support` | `APPDATA`，fallback 为 `${HOME}/AppData/Roaming`    |
| `CACHE_HOME`  | `XDG_CACHE_HOME`，fallback 为 `${HOME}/.cache`       | `${HOME}/Library/Caches`              | `LOCALAPPDATA`，fallback 为 `${HOME}/AppData/Local` |
| `DATA_HOME`   | `XDG_DATA_HOME`，fallback 为 `${HOME}/.local/share`  | `${HOME}/Library/Application Support` | `LOCALAPPDATA`，fallback 为 `${HOME}/AppData/Local` |
| `STATE_HOME`  | `XDG_STATE_HOME`，fallback 为 `${HOME}/.local/state` | `${HOME}/Library/Application Support` | `LOCALAPPDATA`，fallback 为 `${HOME}/AppData/Local` |
| `TEMP_HOME`   | `TMPDIR`，fallback 为 `/tmp`                         | `TMPDIR`，fallback 为 `/tmp`          | `TEMP`，fallback 为 `${CACHE_HOME}/Temp`            |

表中的环境变量名称是内置 TOML 数据，不是变量展开器中的特判。

### 5.5 展开规则

任务选项：

```toml
[modules.example.tasks.cache.options]
roots = [{ path = "${TOOL_CACHE}", include = ["*"] }]
```

规则：

- 语法只接受 `${NAME}`；
- fallback 写在变量映射，不写在清理路径表达式；
- 字符串开头的 `~`、`~/` 或 `~\` 使用映射后的 `HOME`；
- 不执行 shell、命令替换或脚本；
- 未定义、当前不可用、循环引用、未闭合表达式或非法名称都会失败；
- 展开递归应用于当前平台已启用任务的 `options`；
- `recommend`、include 路径和 `variable-map` 路径不展开；
- 不匹配当前平台或被禁用的任务不会要求其变量可用。

插件最终收到的是已展开的选项值。

## 6. 模块

```toml
[modules.personal]
name = "Personal"
description = "User-selected rebuildable data"
order = 20
enabled = true
```

模块 ID：

- 只允许小写 ASCII 字母、数字、点和连字符；
- `module`、`plugin`、`automation`、`recipe`、`theme`、`language`、`authorization`、`scan` 与 `clean` 为固定命令保留；
- 直接成为 CLI 动态子命令。

`spaceprune module list` 列出已启用模块，`spaceprune module list --all` 同时列出禁用模块。其他模块可正常定义
并使用名为 `all` 的动态选择器。

显示顺序按 `order`，相同值按 ID 排序。

## 7. 选择器

```toml
[modules.personal.selectors.logs]
name = "Logs"
match-tags = ["logs"]
enabled = true
```

`tags` 是任务自身属性，也会进入 `TaskRequest`；`match-tags` 是选择器的查询条件。运行
`spaceprune personal --logs --scan` 时，只保留同时包含全部 `match-tags` 的任务。多个选择器同时使用时，
它们要求的标签合并后仍按 AND 匹配。不使用选择器时运行模块内全部已启用且平台匹配的任务。选择器不改变
任务配置。

选择器 ID 不能与固定长选项 `config`、`scan`、`clean`、`select`、`yes`、`output`、`no-progress`、
`help` 或 `version` 重名。`all` 不是固定长选项，可供普通模块使用。

## 8. 任务

```toml
[modules.personal.tasks.logs]
name = "Logs"
description = "Old application logs"
handler = "builtin.filesystem/paths"
enabled = true
order = 10
tags = ["personal", "logs"]
platforms = ["linux", "macos"]

[modules.personal.tasks.logs.options]
roots = [{ path = "${TOOL_HOME}/logs", include = ["*.log"] }]

[modules.personal.tasks.logs.recommend]
older-than = "30d"
```

| 字段          | 必需 | 说明                        |
| ------------- | ---- | --------------------------- |
| `name`        | 是   | 显示名称                    |
| `description` | 否   | 说明                        |
| `handler`     | 是   | 完整 `plugin-id/handler-id` |
| `enabled`     | 否   | 默认 `true`                 |
| `order`       | 否   | 默认 `0`                    |
| `tags`        | 否   | 选择器过滤标签              |
| `platforms`   | 否   | 空数组表示平台中立          |
| `options`     | 否   | Handler 配置                |
| `recommend`   | 否   | Handler 推荐规则            |

Handler ID 必须恰好包含一个 `/`。

## 9. filesystem Handler

ID：

```text
builtin.filesystem/paths
```

完整示例：

```toml
[modules.personal.tasks.logs.options]
files = true
directories = true

[[modules.personal.tasks.logs.options.roots]]
path = "${TOOL_HOME}/logs"
include = ["*.log", "archive/**/*.log"]
exclude = ["keep/**", "*.keep"]

[modules.personal.tasks.logs.recommend]
enabled = true
older-than = "30d"
min-size = "1MiB"
```

### 9.1 options

| 字段          | 默认值 | 说明               |
| ------------- | ------ | ------------------ |
| `roots`       | 必需   | 一个或多个 root    |
| `files`       | `true` | 是否产生文件清理项 |
| `directories` | `true` | 是否产生目录清理项 |
| `elevation` | `auto` | 文件清理按需提权：`auto`、`never`、Linux 的 `sudo` / `pkexec`、Windows 的 `uac`、macOS 的 `native` |

macOS 的 `auto` 使用原生签名清理服务；打包、授权要求和当前删除边界限制见 [macOS 原生组件打包](../build/MACOS-PACKAGING.md)。

每个 root：

| 字段      | 默认值  | 说明                             |
| --------- | ------- | -------------------------------- |
| `path`    | 必需    | 已映射路径或相对路径             |
| `include` | `["*"]` | 相对于 root 的白名单             |
| `exclude` | `[]`    | 相对于 root 的黑名单，优先级更高 |

glob 支持 `*`、`?`、字符类和 `**`。include 或 exclude 中包含路径分隔符时执行递归匹配。

Windows 的 `auto` 使用独立签名清理组件，通过 UAC 为当前已确认的计划建立会话。同一批受限项目复用连接，
仅重试结构化 AccessDenied 项，部分已完成的计量不会重复累加；文件占用和只读卷不当作可提权的权限错误。
只读组件与清理组件是不同程序，后台没有已有管理员权限时返回需要前台处理，不弹出隐藏窗口。
组件须位于受保护的 Program Files 安装目录并通过签名检查；未签名或便携开发产物不能获得管理员删除能力。
此接线尚待 Windows 原生签名安装与 UAC 验收，交叉构建不等于原生验证。

Linux 清理先使用当前权限执行，只把确实因权限不足失败的已确认项目合并成一次提权请求。CLI 的 `auto` 在终端使用 sudo；桌面版明确使用图形授权，不因从终端启动而把密码请求转到终端。sudo 先尝试使用系统已有认证，失效时才请求密码一次；密码不经过应用、方案或 AI。用户拒绝后，本次批处理停止授权尝试，不逐文件弹窗。`never` 不请求管理员权限。

只读检查授权与删除授权有不同用途；Snapper 的 sudo 认证可在系统仍认可时复用，Polkit 是否复用取决于系统对具体动作的授权。应用不把保存的布尔标记当作有效权限，也不保存密码。提权重试只接受本次扫描绑定身份的原选中项，重新校验 root、include、exclude；目录以文件描述符逐层打开，不跟随祖先符号链接或跨越挂载点。替换过的项目要求重新扫描。停止后不会授权新的清理请求，正在执行的请求会协作停止；已经删除的内容不会恢复。

### 9.2 recommend

| 字段         | 默认值 | 说明                        |
| ------------ | ------ | --------------------------- |
| `enabled`    | `true` | false 时所有项目为 optional |
| `older-than` | 无     | 最新修改时间必须早于该时长  |
| `min-size`   | `0B`   | 逻辑容量必须达到阈值        |

两个阈值同时存在时必须全部满足。

`older-than` 比较清理项距今未修改的时长，不是创建时间或保留期限。文档统一使用“未修改时长”这一名称。目录使用整个目录树中最新的修改时间；修改时间过新
或无法取得时，该项目为 optional。

### 9.3 清理边界

- 不跟随符号链接；
- 目录 root 本身不能成为单个清理项；
- 清理项必须仍位于配置 root 内；
- 清理前重新验证 include 与 exclude；
- 含被排除后代的目录不会整体删除；
- 相对 root 以扫描进程当前工作目录解析。

同一次计划中，如果不同任务选中了相同路径或父子路径，引擎只保留最外层项目，容量不会重复计入，路径也不会
重复删除。执行到某项时若该项已被前一项删除，会记为 0 B 的幂等成功；如果扫描时缺失的 root 随后重新出现，
执行器不会把它当作原扫描结果授权清理。

## 10. 推荐、计划与模式

- recommended：默认勾选，`--clean` 会选择；
- optional：默认不勾选，`--select` 可选择；
- unavailable：只展示，不能进入计划。

`--clean` 每次都重新扫描并生成计划。自动化同样重新扫描，不复用历史清理项。

## 11. 自动化

```toml
[automations.daily]
name = "Daily cleanup"
enabled = true
modules = ["cache", "personal"]
at = "03:00"
```

| 字段      | 说明                                   |
| --------- | -------------------------------------- |
| `name`    | 显示名称                               |
| `enabled` | 默认 true                              |
| `modules` | 空数组表示全部启用模块                 |
| `at`      | 零填充的本地时间 `HH:MM`，默认 `03:00` |

自动化始终重新扫描并执行 recommended 项，不提供另一套选择模式或触发器抽象。

安装时会把当前应用 `config.toml` 的规范化路径写入 systemd user unit、LaunchAgent 或 Windows 当前用户计划任务，
运行时再通过该入口解析当前 Recipe。调度器不会直接引用 `recipes/<ID>/<ID>.toml`；仅通过核心
`load_recipe` 加载且没有应用入口的会话会拒绝安装自动化，避免生成无法由 CLI 重载的任务。
核心安装 API 还要求显式提供实现 `automation run <id>` 的绝对 runner 路径；CLI 使用自身路径，桌面等宿主应
提供随安装包部署的 CLI runner，而不是把 GUI 当前进程隐式写入调度器。

`official:minimal` 没有自动化定义。

## 12. 专用 Handler

程序提供第 9 节的跨平台 `builtin.filesystem/paths`，以及以下默认方案不调用的
Linux 专用 Handler：

```text
builtin.snapper/snapshots
builtin.system-linux/package-cache
builtin.system-linux/journal
builtin.system-linux/tmpfiles
builtin.system-linux/coredumps
builtin.system-linux/go-modcache  # existing Recipe alias
builtin.developer-tools/go-modcache
builtin.developer-tools/go-buildcache
builtin.developer-tools/cargo-registry-cache
builtin.developer-tools/npm-cache
builtin.developer-tools/docker-build-cache
builtin.developer-tools/docker-dangling-images
builtin.developer-tools/docker-unused-images
builtin.developer-tools/docker-stopped-containers
```

这些 Handler 需要用户或第三方 recipe 显式配置。普通路径规则应优先使用 filesystem Handler。

### 12.1 Snapper

`builtin.snapper/snapshots` 只在 Linux 注册。

支持两种检查方式：`snapshots` 查询快照信息，并在按方案扫描时只读统计快照内文件的逻辑大小；`rust-targets` 保留原有的快照内 Rust 构建产物检查。没有指定 `scan-mode` 的已有方案仍使用 `rust-targets`。逻辑大小统计在 Core 执行方案扫描时完成。

`options`：

| 字段            | 默认值    | 说明                                            |
| --------------- | --------- | ----------------------------------------------- |
| `configs`       | `[]`      | Snapper 配置名；空数组表示发现全部              |
| `scan-mode`     | `rust-targets` | `snapshots` 查询快照信息；`rust-targets` 检查快照内的构建产物 |
| `snapshot-ids`  | 无        | `snapshots` 模式限定具体快照编号；设置后 `configs` 必须只有一个配置，且 pre/post 必须完整选择 |
| `snapshot-host-id` | 无 | 指定快照编号时，由程序保存原电脑身份；扫描和清理前自动核对 |
| `snapshot-identities` | 无 | 指定快照编号时，同时保存每个快照的 `number`、`date`、`type` 和可选的 `pre-number`，防止同号快照被重建后误认 |
| `snapshot-types` | `[]` | 快照组类型范围：`single`、`pre`、`post`、`pre/post`；空数组包括全部，完整配对使用 `pre/post` |
| `cleanup-algorithms` | `[]` | Snapper 清理标签范围；空数组包括全部，`[""]` 仅包含未设置标签的快照；一组内每个成员都必须匹配 |
| `relative-path` | 无        | 快照子卷内的相对区域；不能是绝对路径或包含 `..` |
| `snapper`       | `snapper` | Snapper 程序                                    |
| `find`          | `find`    | GNU find 程序                                   |
| `du`            | `du`      | GNU du 程序                                     |
| `elevation`     | `auto`    | `auto`、`never`、`sudo` 或 `pkexec`；`auto` 在终端使用 sudo，在桌面使用图形授权 |
| `max-depth`     | `20`      | 查找 `target` 目录的最大深度，必须大于 0        |

`recommend`（默认值随检查方式区分）：

| 字段 | `rust-targets` 默认值 | `snapshots` 默认值 | 说明 |
| --- | --- | --- | --- |
| `enabled` | `true` | `false` | 是否自动推荐符合条件的快照 |
| `min-target-size` | `1GiB` | 不使用 | 构建产物检查的容量阈值 |
| `min-snapshot-size` | 不使用 | `0B` | 快照模式按 Snapper 报告的占用判断；0 表示无大小条件 |
| `older-than-days` | 不使用 | `0` | 快照模式的最小天数；0 表示无时间条件，以组内最新快照为准 |
| `keep-latest` | `3` | `0` | 最新多少个快照保持为可手动选择；0 表示没有这项保留条件 |

快照方案应明确使用 `snapshots` 模式，并根据清理目标与检查结果写出推荐条件，不继承旧方案的 1GiB 或保留 3 个默认规则。推荐中的 `min-snapshot-size` 仍按 Snapper 报告的占用判断；Snapper 未报告占用且方案要求最小容量时，项目不会自动推荐，不用文件逻辑大小替代这个条件。

按方案扫描时，`du --apparent-size --block-size=1` 统计所选范围内快照的逻辑大小，只遍历文件元数据，不查找 `target`、读取文件正文、跨越文件系统或跟随符号链接。同一配置的快照合并统计，`#0` 不参与；pre/post 配对大小为两个成员之和。普通权限失败后才按 `elevation` 请求只读授权，`never` 保留未知值；拒绝授权或统计失败不会把部分数据当成完整大小，也不改变快照的原推荐状态。普通统计过程支持取消，已经进入系统提权命令时仍受系统命令的退出行为约束。

两种模式均将 pre/post 合并为一个选择单元；`#0`、active 或 default 快照不能清理。`rust-targets` 仍将 manual、important 和 pre/post 配对项默认列为可手动选择。`snapshots` 的推荐由方案中明确写出的条件决定，不按名称额外分类。用户可以调整可选项，清理前会重新核对快照状态及完整配对，实际删除调用 `snapper delete --sync`。

同一次清理中，指向同一 Snapper 配置、使用相同程序与提权方式的已选任务会合并成一条删除命令；不同规则仍分别报告结果。只合并已确认的快照编号，每个配置执行前重新校验身份及保护状态。通常不再逐快照要求密码；如果读取快照列表本身需要管理员权限，删除前的核验也可能请求一次授权。sudo 的认证结果在本次清理内复用，删除失败后不会逐项重试或追加提权核对；无法确认的结果如实报告为失败。停止操作会等待已经启动的系统命令结束，再停止后续配置。

Btrfs 数据块可能共享，快照逻辑大小、Snapper 报告占用和旧模式的构建产物容量都不代表精确可释放空间。`logical_bytes` 只用于展示，`reclaimable_bytes` 保持未知；清理结果也不把逻辑大小计作实际释放空间。

可重复使用的规则不写 `snapshot-ids`、`snapshot-host-id` 或 `snapshot-identities`。每次执行扫描时都会重新查询配置中的当前快照，按类型、清理标签、时间、大小及保留数量生成结果。下面演示如何表达一组自定义条件，45 天和 2 个不是产品预置推荐：

```toml
[modules.recovery.tasks.snapshots]
name = "Old recovery points"
handler = "builtin.snapper/snapshots"
platforms = ["linux"]

[modules.recovery.tasks.snapshots.options]
configs = ["root"]
scan-mode = "snapshots"
snapshot-types = ["single", "pre/post"]
cleanup-algorithms = ["timeline", "number"]
elevation = "auto"

[modules.recovery.tasks.snapshots.recommend]
enabled = true
older-than-days = 45
min-snapshot-size = "0B"
keep-latest = 2
```

`keep-latest` 按整个配置中编号最新的非当前快照计数；只要一个配对成员落在保留数量内，整组就不默认推荐。时间以组内最新创建时间为准。数量、时间和大小条件影响推荐，用户仍可明确勾选可选项；当前／默认／活动快照则不可清理。类型和清理标签限制任务范围，清理前也会重新核对。

扫描报告把当次电脑、配置子卷、快照编号与创建身份绑定在本机选中项中，清理时重新验证；这些是一次执行的保护信息，不写入可复用 Recipe。扫描后新建的快照不会自动进入已有确认范围，同编号被替换或配置改变时必须重新扫描。

仍带 `snapshot-ids` 的旧方案表示当时明确选中的固定快照，原有约束保持不变。它们消失后，不会自动改选未来快照。替换固定选择时，应先检查旧任务和实际配置，将被替换的任务设置为 `enabled = false`，保留其他任务，检查并保存新的可复用规则。没有明确替换时，添加动态规则仍保留旧任务。固定选择同步到另一台电脑后不能直接执行；可复用规则可以同步，但仍需在目标电脑重新扫描并核对配置是否对应。

### 12.2 Linux 系统清理

以下 Linux Handler 共用 options：

| 字段          | 默认值 | 说明                                                        |
| ------------- | ------ | ----------------------------------------------------------- |
| `manager`     | `auto` | `package-cache` 使用；识别 zypper、apt/apt-get、dnf、pacman |
| `vacuum-time` | `30d`  | `journal` 传给 `journalctl --vacuum-time` 的时长            |
| `elevation`   | `auto` | `auto`、`never`、`sudo` 或 `pkexec`                         |

它们的 `recommend.enabled` 默认为 true；设为 false 时项目为 optional。缺少相应命令或软件包管理器不受支持时，
项目为 unavailable。软件包和 systemd 清理分别调用选定的软件包管理器、`journalctl` 或 `systemd-tmpfiles`。
系统缓存扫描遇到不可读子树时会跳过该子树、保留其余可读容量，并用 warning 明确说明显示值是下界；
不会因为一个受保护的下载临时目录而丢弃整个缓存计量。
`coredumps` 使用 `systemd-tmpfiles --clean --prefix=/var/lib/systemd/coredump`，遵循系统的保留规则；
`go-modcache` 自动读取当前普通用户 Go 环境的 `GOMODCACHE`，扫描绑定目录及父目录身份，清理不重新按管理员环境发现路径。
当前实现使用原生逐后代捕获，不调用 `go clean`、不修改目录 mode；新增后代仍在所选目录范围，移出对象与外链目标不跟随。
CLI 自带普通 worker 入口，App 使用随包 cleanup helper；其他 Rust 宿主的分发方式见
[开发指南](../build/DEVELOPMENT.md#linux-go-缓存的-worker-分发)。本人 UID/GID 的只读模块可通过 user namespace 清理，
真实能力不足才按 `elevation` 请求权限，`never` 不请求；缺少 worker 会明确报告组件不可用。

仍调用 `go clean -modcache` 的旧版本有并发目录边界风险：其他进程移动或替换后代时，可能影响所选树外内容。
使用旧版程序或旧发行包时，请保持真实缓存的 `go-modcache` 任务 `enabled = false`，直到换成含原生 adapter 和匹配组件的新版。
只更新文档或源码不会更新已安装程序；新版源码不再有上述停用要求，已有 Recipe 无需迁移。
实现与验证记录见 [Go 缓存边界记录](../development/VALIDATION.md)。

### 开发工具缓存

`builtin.developer-tools` 的 Docker 生命周期支持 Linux、Windows、macOS 桌面，Android/iOS 不注册这些能力；下列 Go/Cargo/npm 缓存接口仍仅支持 Linux。每项先检查原生工具是否存在，不存在就跳过，不扫描缓存或申请权限。工具彼此独立，不接受任意路径或 shell 命令。Docker 配置与边界见 [Docker 清理](DOCKER.md)。

| Handler | 范围 | 清理后的影响 |
| --- | --- | --- |
| `go-modcache` | Go 模块缓存 | 依赖可能需要重新下载 |
| `go-buildcache` | Go 构建缓存 | 后续需要重新编译 |
| `cargo-registry-cache` | Cargo `registry/cache` 下载归档 | 离线构建可能需要重新下载；保留源码、Git 检出、凭据和工具链 |
| `npm-cache` | npm `_cacache` 内容寻址缓存 | 离线安装可能需要重新下载；保留 node_modules、配置和凭据 |

```toml
[modules.developer]
name = "开发缓存"
[modules.developer.tasks.npm]
name = "npm 下载缓存"
handler = "builtin.developer-tools/npm-cache"
platforms = ["linux"]
[modules.developer.tasks.npm.options]
elevation = "auto"
[modules.developer.tasks.npm.recommend]
enabled = false
```

`elevation` 支持 `auto`、`never`、`sudo`、`pkexec`，默认 `auto`；`recommend.enabled` 默认 `true`。示例先设为可选，方便核对范围与离线需求。领域任务不支持年龄或大小过滤。扫描只读；执行使用冻结身份的原生文件操作，不调用 cargo/npm/go clean。旧 `builtin.system-linux/go-modcache` 入口继续可用，仍由 `builtin.system-linux` 的开关控制。

## 13. 本地 recipe

每个本地 recipe 使用稳定 ID、同名目录和同名入口文件：

```text
recipes/
└── example/
    ├── example.toml
    ├── example.generated.toml
    └── variables.toml
```

其中 `example` 是 recipe ID。只有同时存在 `recipes/example/example.toml` 时，`recipe list` 才会列出它，
`recipe set example` 才能选中它。入口内部仍可 include 同目录文件：

```toml
include = ["official:minimal", "./example.generated.toml"]
```

除 `official:minimal` 外，include 必须是本地 TOML。使用前应审查内容，且只有当前 recipe 入口明确引用的文件才会生效。

## 14. Recipe bundle

公共配置 API 把 `recipes/<ID>/` 视为一个完整 bundle，而不是只同步入口文件：

```json
{
  "id": "personal",
  "files": {
    "personal.toml": "schema-version = 2\nname = \"Personal\"\n...",
    "personal.generated.toml": "schema-version = 2\n...",
    "variables.toml": "schema-version = 2\n..."
  }
}
```

`files` 的键必须是以 `/` 分隔的 bundle 内相对路径，值必须是 UTF-8 文本。导入时要求：

- `id` 是稳定 Recipe ID，且存在同名入口 `<ID>.toml`；
- 不接受空路径、绝对路径、反斜杠、`.` 或 `..`；
- 导出只接受普通文件和目录，拒绝符号链接及其他特殊文件；
- 所有文件先写入临时目录，并由 `ConfigLoader` 从入口完成 include、Schema、变量和任务校验；
- 校验成功后才替换目标目录，失败时保留原 bundle；导入不会自动切换当前 Recipe。

`ConfigStore::export_recipe_bundle` 与 `ConfigStore::upsert_recipe_bundle` 是桌面端、同步客户端和其他宿主应复用的
边界。云端存储的 revision 不能代替本地语义校验；下载后必须在目标设备重新导入并运行只读扫描。

## 15. 完整个人配置示例

`~/.config/spaceprune/config.toml`：

```toml
schema-version = 2
recipe = "personal"

[theme]
id = "aurora"
```

`~/.config/spaceprune/recipes/personal/personal.toml`：

```toml
schema-version = 2
name = "Personal SpacePrune"
variable-map = "./variables.toml"

include = ["official:minimal", "./personal.generated.toml"]

[automations.daily]
name = "Daily cleanup"
modules = ["cache", "personal"]
at = "03:00"
```

`~/.config/spaceprune/recipes/personal/personal.generated.toml`：

```toml
schema-version = 2

[modules.personal]
name = "Personal caches"
order = 20

[modules.personal.tasks.example]
name = "Example cache"
handler = "builtin.filesystem/paths"
tags = ["cache"]

[modules.personal.tasks.example.options]
roots = [
  { path = "${EXAMPLE_CACHE}", include = ["*"], exclude = ["keep/**"] },
]

[modules.personal.tasks.example.recommend]
older-than = "14d"
```

`~/.config/spaceprune/recipes/personal/variables.toml`：

```toml
schema-version = 2

[variables.EXAMPLE_HOME]
environment = "EXAMPLE_HOME"
fallback = "${DATA_HOME}/example"

[variables.EXAMPLE_CACHE]
value = "${EXAMPLE_HOME}/cache"

[platforms.windows.variables.EXAMPLE_HOME]
value = "${DATA_HOME}/Example"
```

只读验证：

```bash
spaceprune scan --config /absolute/path/to/config.toml --no-progress
spaceprune scan --config /absolute/path/to/config.toml --output json --no-progress
```

## 16. 常见错误

- 把 include 文件当成无版本片段，遗漏整数 `schema-version = 2`；
- recipe 目录名与入口文件名没有使用同一个 ID；
- 应用配置选择了 `recipes` 下不存在的 recipe ID；
- recipe 入口忘记 include `official:minimal`，因此只剩自定义模块；
- 把 `[theme]` 写进 recipe，而不是应用 `config.toml`；
- `variable-map` 写在 include 片段而不是 recipe 入口；
- 清理配置直接使用未映射的环境变量名；
- 在路径中写 fallback，而不是在映射定义中写；
- 用户平台覆盖只写部分字段，导致完整定义缺少 `environment` 或 `value`；
- roots 数组覆盖后遗漏原有项；
- include 或映射文件使用 `~`、`${NAME}` 等不会展开的 bootstrap 路径；
- glob 相对于 root，却误写成绝对路径；
- 用 `--clean --yes` 验证新配置，而没有先运行 `--scan`。

## Bundle 公共合同

`spaceprune-contracts::bundle::RecipeBundle` 是 bundle 的唯一公共 Rust 数据定义；JSON 仅包含 `id` 与
`files`，拒绝未知顶层字段。完整文件映射与当前平台解析结果分开保留。源文件编辑由 config 中的
`rename_recipe_bundle` / `update_bundle_automation` 完成，不从扫描或解析报告重建整个方案。

所有 bundle 路径与目录前缀共用一份可移植路径验证。除穿越、绝对路径和 Windows 保留名称外，
`a.toml` 与 `A.toml/x.toml`、`Parts/a.toml` 与 `parts/b.toml` 等 ASCII 大小写冲突也会被拒绝。
本机导入先验证整个命名空间，再创建临时目录；验证失败不会覆盖原方案。此纯校验不代替目标文件系统
上的原生身份、链接和权限检查，也不声明覆盖所有 Unicode 文件系统规范化规则。

## Schema 2 与范围声明

应用配置、Recipe 入口、每个 include 和 variable-map 都使用整数 `schema-version = 2`。旧版 1 配置明确拒绝，
不静默按新范围解释；插件 manifest 仍使用其独立的 schema 1。迁移时只修改版本号，并检查新加入的用途与范围约束。

`usage = "recurring"`（默认）保存可复用规则，每次扫描重新枚举；`usage = "one_shot"` 保存带 host/对象身份绑定的
一次性选择，不能启用自动计划。一次性文件使用 `exact_file`，目录使用 `directory_children` 或 `directory_subtree`；
两种目录范围都保留选中目录的动态子树语义和硬排除。`directory_children` 仅将直接子项作为选择单位，不能降格成递归文件匹配。

周期根可以使用字符串表达式、`{ user_home = [".cache", "example"] }` 或
`{ local_absolute = "/local/observed/cache", host_id = "本机身份" }`。本机绝对位置必须与当前主机身份一致；
更换设备需要重新绑定。文件身份仅属于一次性绑定和本次执行计划，不能把当前文件库存保存为周期规则。

公开 JSON Schema 位于 `schemas/`，由 `spaceprune-contracts` 的 Rust 类型生成。
运行 `cargo run --locked -p spaceprune-contracts --bin export-schemas` 更新产物；合同测试检查产物是否与类型一致。
文件系统 Handler 的 options/recommend schema 也从实际解析类型生成，包含 scope 和 bindings。
