# Recipe 编写与只读验证

本文介绍 Core/CLI 可执行的清理方案：文件布局、变量映射、模块、任务、平台条件和验证步骤。
完整字段定义见 [配置参考](CONFIGURATION.md)，命令使用见 [CLI](CLI.md)。
扫描、保存配置和执行清理是独立动作；先核对实际命中，再确认清理范围。AI 会话及云端业务由各自应用维护。

## 1. 选择配置位置

先按[默认配置发现](CONFIGURATION.md#2-默认配置发现)找到应用入口，或用 `--config` 显式选择。以下示例使用 Linux 的 HOME fallback；各平台的 Recipe 内部布局相同，平台路径差异通过变量映射和条件任务表达。

## 2. 推荐文件布局

以下是 Linux 使用 HOME fallback 时的示例；其他候选根目录内部使用完全相同的文件布局：

```text
~/.config/spaceprune/
├── config.toml
└── recipes/
    └── personal/
        ├── personal.toml
        ├── personal.generated.toml
        └── variables.toml
```

`personal` 是 recipe ID，目录和入口文件必须同名。`variable-map` 与 `include` 使用相对路径，生成结果不应依赖
该 recipe 目录之外的本地文件。

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

include = [
  "official:minimal",
  "./personal.generated.toml",
]
```

`variables.toml` 除必需的整数 `schema-version = 2` 外，只维护逻辑路径到平台环境或固定目录的映射。

`personal.generated.toml` 除必需的整数 `schema-version = 2` 外，只维护模块、任务、路径规则和推荐策略：

```toml
schema-version = 2

[modules.personal]
name = "Personal data"
```

recipe 入口、`personal.generated.toml`、common 或平台专用 include 等每个物理配置文件都会在合并前独立验证，
不能从入口继承 `schema-version`。字段必须是整数 `2`，不能写成字符串 `"2"`。

这种布局让应用入口稳定，并把每套清理配置完整封装在自己的 recipe 目录中。

## 3. 变量映射

### 3.1 基本原则

任务路径使用逻辑变量：

```toml
roots = [{ path = "${TOOL_CACHE}", include = ["*"] }]
```

不要在任务配置里假设 `TOOL_CACHE` 等于某个系统环境变量。对应关系写进 `variables.toml`：

```toml
schema-version = 2

[variables.TOOL_HOME]
environment = "TOOL_HOME"
fallback = "${DATA_HOME}/tool"

[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"
```

### 3.2 平台映射

```toml
[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"

[platforms.macos.variables.TOOL_CACHE]
value = "${CACHE_HOME}/Tool"

[platforms.windows.variables.TOOL_CACHE]
value = "${CACHE_HOME}/Tool/Cache"
```

合并顺序：

1. 内置公共；
2. 内置当前平台；
3. 用户公共；
4. 用户当前平台。

用户同名变量替换完整定义。配置不能只提供一个孤立的 fallback 而省略 `environment` 或 `value`。

### 3.3 内置变量

可直接使用：

```text
HOME
CONFIG_HOME
CACHE_HOME
DATA_HOME
STATE_HOME
TEMP_HOME
```

工具专属目录应创建新逻辑变量，例如 `BROWSER_CACHE`、`AGENT_HOME`、`PROJECT_ROOT`。

### 3.4 语法

清理配置只接受：

```text
${NAME}
```

fallback 放在映射定义中。不要在任务路径中写带 fallback 的表达式。字符串开头的 `~` 使用映射后的 `HOME`，
但共享配置优先使用显式逻辑变量。

include 路径和 `variable-map` 路径不展开，必须是相对 recipe 入口或绝对本地路径。

## 4. 模块、任务、tags 与选择器

模块：

```toml
[modules.tool]
name = "Tool data"
description = "Rebuildable data produced by Tool"
order = 20
```

模块 ID 不能使用固定命令保留字 `module`、`plugin`、`automation`、`recipe`、`theme`、`language`、`authorization`、`scan` 或 `clean`。

任务：

```toml
[modules.tool.tasks.logs]
name = "Logs"
handler = "builtin.filesystem/paths"
tags = ["logs", "tool"]
```

选择器：

```toml
[modules.tool.selectors.logs]
name = "Logs only"
match-tags = ["logs"]
```

区别：

- `tags` 是任务自身属性，也会传给 Handler；
- `match-tags` 是选择器的查询条件；
- 一个任务必须包含选择器要求的全部标签；
- 多个选择器同时使用时，全部 `match-tags` 合并，仍按 AND 匹配；
- 不使用选择器时运行模块内全部已启用且平台匹配的任务。

选择器 ID 不能使用固定长选项名 `config`、`scan`、`clean`、`select`、`yes`、`output`、`no-progress`、
`help` 或 `version`。普通模块可以使用 `all` 作为动态选择器。

上例对应：

```bash
spaceprune tool --logs --scan
```

## 5. filesystem 任务

```toml
[modules.tool.tasks.logs]
name = "Tool logs"
handler = "builtin.filesystem/paths"
tags = ["tool", "logs"]

[modules.tool.tasks.logs.options]
files = true
directories = true

[[modules.tool.tasks.logs.options.roots]]
path = "${TOOL_HOME}/logs"
include = ["*.log", "archive/**/*.log"]
exclude = ["keep/**", "*.keep"]

[modules.tool.tasks.logs.recommend]
enabled = true
older-than = "30d"
min-size = "1MiB"
```

### 5.1 root

每个 root 独立定义：

- `path`：根路径；
- `include`：相对 root 的白名单；
- `exclude`：相对 root 的黑名单，优先级更高。

应优先使用明确白名单，避免扫描整个 HOME、磁盘根目录或包含大量无关数据的父目录。

### 5.2 glob

- `*`：单层匹配；
- `?`：单字符；
- `[...]`：字符类；
- `**`：递归目录。

glob 相对于 root。不要把绝对路径写进 include 或 exclude。

### 5.3 推荐

- `enabled = false`：全部 optional；
- `older-than`：最新修改时间必须早于该时长；目录按整个目录树中最新的修改时间判断；
- `min-size`：逻辑容量阈值；
- 同时存在时必须全部满足。

`older-than` 表示距今未修改的时长，不是创建时间或保留期限。始终称为“未修改时长”，不要按对象存在时间理解。
修改时间过新或无法取得时，项目为 optional。

不确定的数据应设为 optional，而不是用宽松规则自动推荐。

## 6. 跨平台配置

任务逻辑相同、路径不同：优先使用变量映射。

任务结构也不同：使用条件 include。

```toml
include = [
  "./common.toml",
  { path = "./linux.toml", platforms = ["linux"] },
  { path = "./windows.toml", platforms = ["windows"] },
]
```

上例的 `common.toml`、`linux.toml` 和 `windows.toml` 都必须各自包含整数 `schema-version = 2`。

任务只在部分平台可用：

```toml
[modules.tool.tasks.native-cache]
name = "Native cache"
handler = "builtin.filesystem/paths"
platforms = ["linux", "macos"]
```

不要复制整份 recipe 入口来区分平台，除非方案本身完全不同。

## 7. 不应默认清理的内容

- 文档、照片、源码和未确认的用户数据；
- 浏览器完整 profile；
- 密码库、认证令牌、SSH、GPG 和密钥目录；
- 配置文件本身；
- 数据库主文件；
- 当前运行所需的 lock、socket 或 active session；
- 工作目录中含义不明的构建产物；
- 磁盘根目录、整个 HOME、整个 APPDATA 或整个 Application Support；
- 依赖 root / sudo 的系统范围操作。

如用户明确要求，仍应通过窄 root、明确 include 和保守推荐规则表达。

## 8. 专用 Handler

普通文件路径使用：

```text
builtin.filesystem/paths
```

只有需要领域 API 或系统命令时才使用：

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

这些能力存在于程序中，但 `official:minimal` 不调用。使用前应核对配置参考中的专用 Handler 字段及 options。

开发工具缓存目录不存在时，显示 0 B 和无可清理内容，不计为大小未知。权限不足或目录身份变化仍显示诊断；工具以后重新创建缓存时，重新扫描会计量新内容。

### Linux 系统资源的大小

日志扫描会统计 `/var/log/journal` 与 `/run/log/journal` 的文件逻辑大小；崩溃转储显示当前转储目录的逻辑大小。它们是占用参考，保留期限和系统过期规则决定实际处理内容，不把整个目录计入预计释放量。

系统临时文件使用 `systemd-tmpfiles --clean --dry-run` 预览已安装的系统规则，再读取候选文件元数据，遵守原生的过期条件和排除项，不读取文件正文、不执行清理。此预览需要 systemd 256 或更新版本；不支持、权限不足或预览未完成时会说明大小未知的原因，原清理入口仍然可用。检查有时间和输出限制，并响应取消。

临时文件规则可能覆盖单独列出的崩溃转储，共享文件和被进程占用的文件也不保证立即释放空间。这些逻辑值供选择时参考，不重复加入可回收空间合计。CLI 与使用 Core 的界面读取同一计量结果。

## 9. 外部插件

配置需要未内置 Handler 时：

1. 确认插件已经本地安装；
2. 在 `requires.plugins` 写版本要求；
3. 使用完整 Handler ID；
4. 先运行插件 inspect；
5. 只读扫描验证。

```toml
[requires.plugins]
"community.example" = "^1.0"
```

插件必须已经位于本地可安装目录。

## 10. 自动化

只有用户明确要求时才生成：

```toml
[automations.daily]
name = "Daily cleanup"
modules = ["cache", "tool"]
at = "03:00"
```

启用自动化前先验证：

- 所有目标任务都适合无人值守；
- 推荐规则足够保守；
- 不需要交互选择；
- 自动化使用的配置路径稳定；
- 手动 `automation run` 成功。

生成配置不等于安装自动化。

## 11. 只读验证

第一步：

```bash
spaceprune scan --config /absolute/path/to/config.toml --no-progress
```

结构化检查：

```bash
spaceprune scan --config /absolute/path/to/config.toml \
  --output json --no-progress
```

检查：

- `config_source` 是目标 `recipes/<ID>/<ID>.toml`；
- 模块和任务数量符合预期；
- 所有 location 都在允许范围；
- 没有变量缺失、循环或非法表达式；
- recipe 入口、variable-map 和每个 include 文件都各自包含整数 `schema-version = 2`；
- 没有 include 缺失或平台拼写错误；
- recommended 项确实可重建；
- exclude 项未出现在结果；
- 不存在意外的系统级目录。

默认发现验证：

```bash
spaceprune scan --no-progress
```

只有在显式路径验证成功后才验证默认发现。

