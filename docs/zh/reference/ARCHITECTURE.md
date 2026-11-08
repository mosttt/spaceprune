# SpacePrune 架构

本文描述仓库当前实现的结构、数据流和边界。

## 1. 系统定位

SpacePrune 是清理执行器，不是写死清理目标的命令集合。核心模型为：

```text
配置方案
  -> include 合并
  -> 平台变量映射
  -> 模块与任务
  -> Handler 扫描
  -> 结构化报告
  -> 推荐或用户选择
  -> 清理计划
  -> Handler 执行
```

核心只固定运行模式：

- scan：只读扫描；
- clean：执行推荐计划；
- select：用户选择计划；
- plugin：统一能力目录、本机启停与本地外部插件管理；
- automation：配置化自动化管理。

模块 ID、选择器、任务、路径和推荐规则均来自配置。

桌面应用和其他宿主通过公共 library API 提供或载入 Recipe，再复用相同的校验、扫描、计划和执行链。
Core、CLI 与内置插件在本仓库中独立构建和运行。

## 2. 设计原则

### 2.1 配置表达意图

配置负责：

- 模块和任务组织；
- 平台约束；
- root、include 白名单和 exclude 黑名单；
- 推荐条件；
- Handler 选择；
- 自动化范围；
- 插件版本要求；
- 用户变量映射入口。

### 2.2 映射吸收平台路径差异

任务只引用 `${NAME}`。实际环境变量名、平台目录和 fallback 位于独立变量映射 TOML。变量展开器只实现
通用解析、合并、引用和循环检测，不包含 `HOME`、`APPDATA`、`XDG_CACHE_HOME` 等名称的特判。

### 2.3 Handler 表达能力

- filesystem Handler 处理普通路径（1 个）；
- Snapper Handler 处理快照发现、分组、保护和删除（1 个）；
- Linux system Handler 调用包管理器与 systemd，并保留旧 Go 入口（5 个）；
- developer-tools Handler 管理 Linux Cargo、Go 与 npm 缓存（4 个），以及桌面本机 Docker 生命周期（4 个）；
- 外部进程插件提供本地安装的领域能力。

Linux 上静态装配前 4 类共 11 个 Handler；其他桌面平台只注册适用于该平台的子集。

### 2.4 最小内置

二进制只内嵌 `official:minimal`。它提供基础用户缓存或临时目录清理，不包含个人工具偏好。其他 recipe
由用户以本地文件提供。

### 2.5 运行时确定性

应用配置、recipe、变量映射和插件都从本地路径发现；当前 recipe 明确决定本次运行的全部范围。

### 2.6 输入、权限与执行边界

Recipe 和导入的 bundle 都需要通过本机配置校验。扫描得到的观察与本次确认的执行计划分开保存；
选择一个文件不会提升成父目录，目录选择仍遵守硬排除和身份/挂载边界。
通用权限服务供 CLI、扫描、清理及其他宿主复用。只读许可不授予删除许可，远程配置不授予本机权限。

## 3. 仓库结构

```text
spaceprune/
├─ Cargo.toml
├─ crates/
│  ├─ spaceprune/                 高层 library API 与应用装配，无 CLI/终端依赖
│  ├─ spaceprune-cli/             Clap、CLI/TUI、终端主题、输出与 spaceprune bin
│  ├─ spaceprune-contracts/       公共纯数据：bundle、模块/任务/自动化、Handler DTO 与路径验证
│  ├─ spaceprune-config/          应用配置读写、recipe 发现、include、映射、内嵌资源
│  ├─ spaceprune-engine/          扫描、计划、执行、事件
│  ├─ spaceprune-access/          操作上下文、通用权限、原生访问及受控进程
│  ├─ spaceprune-privileged-helper/  公共 metadata/cleanup 服务与安装入口
│  ├─ spaceprune-platform/        自动化平台 facade
│  ├─ spaceprune-platform-linux/  Linux systemd 自动化
│  ├─ spaceprune-plugin-api/      Handler 执行接口、取消、进度 sink；重导出公共 DTO
│  └─ spaceprune-plugin-host/     外部插件发现、安装、进程协议
├─ plugins/
│  ├─ cross-platform/
│  │  ├─ filesystem/
│  │  ├─ memory/
│  │  └─ developer-tools/
│  ├─ linux/
│  │  ├─ snapper/
│  │  └─ system-linux/
│  ├─ windows/
│  │  └─ system-windows/
│  └─ android/
│     └─ mobile-documents/
├─ recipes/
│  └─ official/
│     └─ minimal.toml
├─ mappings/
│  └─ official/
│     └─ variables.toml
├─ docs/
└─ .github/workflows/
```

其他应用通过公共 Core API 集成，不属于本 Cargo workspace。

插件按实际支持的平台组织源码。跨平台领域只保留一个插件，在内部用平台模块分离原生实现；系统专属插件放在对应系统目录。内存管理采用一个领域插件、多个平台适配的结构，放在 `cross-platform/memory`；当前可用能力仍以实现和平台声明为准。目录整理不改变 crate 名称、插件 ID、Handler ID 或方案语义。目录分组不代替编译条件和运行时能力报告；跨平台插件也可能只在部分系统提供某个 Handler。

## 4. crate 依赖方向

依赖关系按职责分层：

- `spaceprune-plugin-api` 与 `spaceprune-config` 是互不依赖的基础 crate；
- `spaceprune-engine` 依赖这两个基础 crate；
- 静态 Handler 依赖 `spaceprune-engine` 与 `spaceprune-plugin-api`；
- `spaceprune-plugin-host` 依赖配置、核心和插件 API；
- 平台 crate 依赖配置与核心，Linux 平台 crate 同时使用插件 API；
- `spaceprune` 高层 library 统一装配配置、Engine、平台、静态 Handler 和外部插件宿主；
- `spaceprune-cli` 单向依赖 `spaceprune`，负责 Clap、终端输入输出和 TUI，不被 library 或 Desktop 反向依赖。

边界保持：

- plugin-api 不依赖执行器；
- config 负责应用 `config.toml` 与 recipe 的发现、读取和持久化，不依赖终端、插件进程或平台命令；
- `spaceprune` 与 `spaceprune-engine` 不依赖 Clap、Ratatui 或 Crossterm；
- `spaceprune-engine` 也不依赖 systemd、launchd 或 Windows Task Scheduler；
- Handler 不决定全局运行模式；
- CLI 只负责参数和终端输入输出；桌面端通过 `spaceprune::api` 使用结构化接口。

`spaceprune::api::SpacePrune` 提供配置加载、`scan`、`execute` 和事件总线；同时重新导出 `ScanRequest`、报告、
`ConfigStore`、`RecipeBundle`、插件与自动化管理类型。GUI 不启动 CLI 子进程，也不复制装配逻辑。

## 5. 启动流程

CLI 的 `main.rs` 只处理原生安装器分派和进程入口。`application.rs` 管理原启动顺序、配置加载、命令分派和退出码；`application/cleanup.rs` 管理一次扫描后的推荐／手选／确认与执行。两种确认入口最终共用一处执行和报告归并，不增加第二份扫描状态。

`commands/{modules,plugins,automations,recipes,themes,language}.rs` 分别适配管理命令，原 `management.rs` 已移除。配置与平台操作仍调用公共 Core；`runtime.rs` 保留信号与操作上下文，`selection`、`progress` 和 `output` 保留交互与输出。授权安装命令仍先于主题和活动方案加载，损坏的方案不阻止修复授权组件。

清理命令和管理命令的启动流程：

1. CLI 先解析系统显示语言，并从显式 `--config` 或默认应用配置读取共享语言偏好；这一步不加载活动 Recipe；
2. 授权管理命令先分流；其他命令由 `ConfigStore` 读取主题 ID，再构造固定命令树；
3. language、插件、theme 和 recipe 管理命令在清理配置加载前分流，JSON 输出保持既有机器合同；
4. `ConfigLoader` 读取应用配置中的 recipe ID；
5. ID `personal` 按约定解析为 `recipes/personal/personal.toml`；
6. 深度优先解析 recipe include；
7. 合并并反序列化 `ResolvedConfig`；
8. 校验 Schema、ID、Handler ID、平台和自动化引用；
9. 加载内嵌变量映射，并合并 recipe 入口声明的用户映射；
10. 解析活动变量并展开任务 `options`；
11. 注册静态 Handler，并发现当前方案实际需要的外部插件；
12. EngineBuilder 校验活动任务后扫描、计划和执行。

`module`、`plugin`、`automation`、`recipe`、`theme`、`language`、`authorization`、`scan` 和 `clean` 由统一的固定命令常量保留，不能成为配置模块 ID。
`ConfigStore` 位于 `spaceprune-config`，负责保留格式地更新应用配置；交互式 picker 位于 `spaceprune-cli`。

## 6. 配置发现的 bootstrap

应用配置位置必须在 recipe 和变量映射加载前确定。候选位置按平台排列：

```text
Linux / Android: XDG_CONFIG_HOME/spaceprune/config.toml
                 HOME/.config/spaceprune/config.toml
macOS / iOS:     XDG_CONFIG_HOME/spaceprune/config.toml
                 HOME/Library/Application Support/SpacePrune/config.toml
Windows:         APPDATA\SpacePrune\config.toml
                 USERPROFILE\AppData\Roaming\SpacePrune\config.toml（APPDATA 不可用时）
```

显式 `--config` 优先。默认发现选择首个已存在文件；都不存在时设置命令写入第一候选位置。候选根目录内部始终使用
`config.toml` 与 `recipes/<ID>/<ID>.toml` 的统一布局。这部分不参与任务路径展开，空环境变量被忽略。

如果没有用户配置，普通清理命令加载 `official:minimal`。首次设置主题或语言会创建默认应用配置和
`recipes/default/default.toml`。

## 7. include 解析

每个 Source 是：

```text
File(absolute path)
Official(resource name)
```

解析算法：

1. 加载当前 Source；
2. 验证 `schema-version`；
3. 取出 include；
4. 在访问路径前跳过不匹配平台的条件 include；
5. 递归解析子 Source；
6. 按声明顺序合并子结果；
7. 最后合并当前文件；
8. 缓存本次根加载过程中的结果。

表递归合并；数组和标量替换；类型冲突报错。解析栈用于检测循环。

需要保留源文件的编辑器可调用 `ConfigLoader::load_path_with_sources`，同时取得未展开声明、当前执行视图和实际生效的本地来源顺序。来源按相同的 include 合并顺序返回，重复引用的缓存子树仍保留每次出现；未启用平台的路径不解析、不读取，内嵌资源没有可编辑的本地路径。原 `load_path` 和 `load_path_with_definition` 入口保持原返回合同。保存时仍以完整 `RecipeBundle` 为准，不能用执行视图替换多文件源码。

`variable-map` 只能存在于 recipe 根 Source。这样映射路径始终相对于明确的 recipe 目录。

## 8. 变量映射

### 8.1 文件模型

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

`VariableDefinition` 包含：

- `environment: Option<String>`；
- `value: Option<String>`；
- `fallback: Option<String>`。

environment 与 value 互斥；fallback 只允许和 environment 配合。

### 8.2 合并

活动定义按以下层级覆盖：

```text
内嵌公共
  -> 内嵌当前平台
  -> 用户公共
  -> 用户当前平台
```

同名变量替换完整定义。内嵌与用户文件使用同一 Schema。

### 8.3 解析

解析器维护：

- definitions：活动定义；
- cache：已解析值或当前不可用；
- stack：递归引用栈。

环境值按字面量使用，空值视为缺失。缺失且有 fallback 时解析 fallback；没有 fallback 时该变量保持不可用。
引用不存在的变量、循环、非法表达式或非法名称直接失败。

### 8.4 展开范围

配置反序列化完成后，ConfigLoader 递归展开：

- 已启用模块中当前平台已启用任务的 `options`。

不展开：

- include 路径；
- `variable-map` 路径；
- 推荐规则；
- 显示文本；
- 当前平台不会执行的任务。

因此静态和外部 Handler 接收到的 TaskRequest 已包含最终选项值，不需要自己读取环境变量。

## 9. 内嵌最小方案

```text
recipes/official/minimal.toml
          │ include_str!
          ▼
official:minimal
```

方案只有 `cache` 模块：

- Linux、macOS、Android、iOS：`${CACHE_HOME}` 的直接子项；
- Windows：`${TEMP_HOME}` 的直接子项。

平台变量由 `mappings/official/variables.toml` 提供。recipe 不包含环境变量名或平台 fallback。

## 10. recipe 输入边界

应用配置通过稳定 ID 选择一个本地 recipe。目录约定是：

```text
recipes/<ID>/<ID>.toml
```

`ConfigStore` 只枚举 ID 语法有效、目录名与入口文件名一致且入口含有字符串 `name` 的方案。`recipe set <ID>`
只能选择枚举结果，避免保存悬空路径。选中的 recipe 入口可引用编译时资源 `official:minimal` 和相对本地 include；
运行时不会隐式启用其他 recipe 或未引用文件。

### 10.1 bundle 导入与导出边界

`RecipeBundle` 使用稳定 `id` 和 `BTreeMap<相对路径, UTF-8 内容>` 表示 `recipes/<ID>/` 的完整快照。
`ConfigStore::export_recipe_bundle` 递归收集普通文件，拒绝符号链接和特殊文件，并把路径统一成 `/` 分隔。

`upsert_recipe_bundle` 要求同名入口 `<ID>.toml`，拒绝绝对路径、`.`、`..` 和反斜杠路径。它先在同一 recipes 根目录
创建 staging，写入全部文件，再从 staging 入口调用 `ConfigLoader` 做完整语义校验；成功后才目录级替换，失败时删除
staging 并保留原方案。导入 bundle 不会隐式切换活动 Recipe。

## 11. Engine

`EngineBuilder` 持有：

- `ResolvedConfig`；
- `PluginRegistry`；
- `EventBus`。

构建阶段：

1. 遍历启用模块和当前平台任务；
2. 检查 Handler 已注册；
3. 将 TOML options/recommend 序列化为 JSON；
4. 调用 Handler `validate`。

`Engine` 提供：

- `scan`；
- `execute`；
- `config`；
- `event_bus`。

## 12. 扫描

扫描流程：

1. 解析模块范围；
2. 将选择器转换为必须标签集合；
3. 校验可选的精确任务 ID 集合；
4. 过滤 disabled、平台不匹配、任务 ID 不匹配和标签不匹配任务；
5. 按 order 与任务 ID 排序；
6. 构造 TaskRequest；
7. 调用 Handler `scan`；
8. 稳定排序并校验项目 ID、父项引用和层级无环；
9. 汇总 TaskReport、ModuleReport 和 ScanReport。

单任务错误记录在报告中，不阻止其他任务扫描。Handler 返回重复 ID、空 ID、悬空父项或层级环时，
该任务扫描失败。`ScanReport.cancelled` 表示报告是否因取消而不完整；不完整报告不能生成清理计划。

## 13. 计划

报告中的项目分为：

- recommended；
- optional；
- unavailable。

`plan_recommended` 只选择 recommended。`plan_selected` 校验用户键：

- 项目必须存在；
- unavailable 不能进入计划；
- 父子项目不能产生重复删除；
- 计划保留原始 TaskRequest，保证清理使用与扫描一致的最终选项。

## 14. 执行

`Engine::execute` 按任务组调用 Handler `clean`。每组记录：

- attempted；
- succeeded；
- reclaimed_bytes；
- unknown_reclaimed_items（可空，表示成功、部分完成或结果不确定的项目中处理大小未知的数量）；
- 单项 failure；
- 任务级错误。

一个任务组失败不会中止其他任务组。取消令牌在任务和项目边界协作式检查。

公共 `spaceprune-access::OperationContext` 保存操作 ID、generation、交互方式、资源预算及绝对期限。
原生子操作保留这些身份和预算，并收紧期限；父取消通过 `CancellationToken::cancel()` 传播到仍在运行的
子树，取消一个子操作不会取消兄弟操作。Handler、ScanAccess 和 CleanupAccess 直接接收原上下文，
`NativeScanAccess::new()` 不另存一份时钟。宿主使用 facade 的 `load*_in_context`、`scan*_in_context`、
`execute_in_context` 贯穿装配、校验、扫描和权限重试；旧 token 便利入口仅在顶层创建独立操作。
CLI 安装 Ctrl+C 后才装配插件；交互清理在用户确认后开启独立期限，自动化扫描与清理共用一个期限。
取消请求与实际退出仍须由调用方分别确认，裸 atomic 仅供原生遍历读取信号。
核心校验 `attempted = succeeded + failures`；未取消时还要求 `attempted` 等于组内选中项数量。
`ScanReport.cancelled` 与 `ExecutionReport.cancelled` 表达取消或期限届满导致的提前结束，新增可选
`stop_reason` 为 `cancelled` 或 `deadline_exceeded`；旧序列化报告没有此字段时按兼容规则处理。
终态事件携带同样原因；事件信封新增 `operation`（字符串 ID 与 generation），保留事件 body 原数值 ID。
超时保留已完成的效果及计量；不完整扫描不能产生清理计划。CLI 超时退出 1，显式取消退出 130。

跨进程通过公共 `OperationTransfer` 传递 ID/generation、系统单调时钟的绝对截止时间与输出预算，
等待授权或排队不重新获得工作额度。接收端只能收紧预算，强制非交互，并通过控制目录/管道 EOF 接收取消；
操作身份本身不授予任何读写能力。普通工作流使用 `OperationContext::workflow`，60 分钟总期限、
64 MiB 响应预算和 5 分钟输出等待；基本 native/tool 调用保留较小默认预算，具体协议可进一步收紧。
只读协议 6 限制请求文件为 4 MiB、单请求最长 120 秒；清理协议 4 校验握手、批次与结果的操作身份。

`MetadataService::close()` 先停止接受旧代只读请求，对正在授权、等待连接和读取结果的请求发送局部取消，
等待这些请求完成连接释放，再撤销 broker 中的可复用连接；关闭期间的新调用在原取消/期限约束下等待，
随后进入新的授权代次。子上下文保留原 ID、generation、期限、交互方式与预算，关闭不取消宿主的兄弟工作，
同一操作的拒绝抑制仍保留。`close() -> Result<(), PermissionOutcome>` 收集在途、缓存和失活连接的关闭
结果：首次未确认错误持续保留，重复关闭或清空缓存不会将其变成成功，该服务实例也不再授权新请求。
只有平台明确标记 `shutdown_unconfirmed` 的建连失败才进入此状态，普通授权拒绝或不可用不被误判。
Linux 临时进程和真实 pkexec/root helper 验证已确认关闭返回时 PID 退出；macOS 显式消费 Stopped，
缺失、错配或超时向调用方返回错误。Windows 的同步 UAC 调用仍须等系统提示结束，原生平台验收继续开放。
授权复用绑定 OS 登录身份；前台操作所属的登录会话被切出、退出或监视失效时，会单向撤销旧绑定，切回后也不能恢复旧操作。非交互任务与确证无登录会话的进程作用域规则见 [本机权限与授权复用](NATIVE-AUTHORIZATION.md)。

Linux 与 Windows 的普通 filesystem 扫描、预览及 CLI 共用 `NativeScanAccess` / `MetadataService`。
先按当前权限扫描，仅结构化 AccessDenied 触发重试；`elevation = never` 和非交互约束保留。
Linux 使用既有 stdio 连接；Windows 通过 `runas` 启动独立组件，以私有目录内的序号帧复用连接。
Windows helper 使用 `asInvoker` 清单，使无权限探针无需 UAC；真正会话要求提升 token、有效代码签名、
实际 Program Files 下的受保护目录和文件。宿主及 helper 持有安装对象句柄，拒绝 reparse 路径与可由普通用户写入的安装。
会话绑定原宿主进程与 OS 登录身份；宿主退出、登录绑定撤销、关闭连接或空闲 15 分钟后结束复用。具体身份与 IPC 约束见 [Windows 原生组件打包](../build/WINDOWS-PACKAGING.md)。请求仍单次消费并保留原操作期限，
完整任务结果经有界 TCP 传输，控制帧不承载大结果。Windows 普通文件扫描接线已完成交叉验证；
签名安装、跨账户 UAC、真实取消与撤销仍需原生验证，未签名构建不会获得该读取能力。
macOS 的普通 Recipe、预览和 CLI 现由同一 facade 处理读取拒绝：BSD/ACL 的 EACCES 进入独立签名 metadata 服务；
EPERM 作为系统策略拒绝，前台可打开隐私设置，并返回 NeedsSystemSettings。EPERM 不足以认定一定是 TCC，
也不代表 FDA 能绕过 sandbox/SIP 等限制；同操作合并设置请求，后台只报告需处理状态。
隐私恢复由原宿主在原路径实际验证，普通结果、原规则和未知大小保留；系统设置不生成授权连接或持久 grant。
管理员 metadata 由签名授权子进程直接向固定服务交接凭证，绑定原宿主、用途与操作；宿主不持有外部凭证。服务逐请求检查应用 right 与内置管理员 right，保留已有 OS 策略。不同 right 名或 NoExtendRights 本身不证明用途隔离。
完整请求和结果均经带 audit token 校验的有界 Mach 分片传输，不信任请求内 PID、角色或权限标志。
Windows 删除现接入 `NativeCleanupAccess`，独立 cleanup helper 经本地命名管道收取原确认计划和索引选择。
管道有显式调用者登录 SID/SYSTEM/Administrators DACL，拒绝远程客户端；两端使用内核报告的对端进程 ID 校验，
helper 另核对宿主创建时间和 SID，客户端连接禁止服务器模拟管理员身份。读取连接不接受清理请求。
Windows 删除通过 NtCreateFile 相对目录句柄逐级打开，通过同一目录句柄枚举并在已验证对象句柄上执行删除，
不再用显示路径递归 read_dir。已有结果、逐项落盘回执、重复选择拒绝、原期限和部分/未知效果协议保持共用。
macOS metadata 与 filesystem cleanup 已有服务/宿主代码接线，OS 权限检查和文件工作运行在受监督的独立进程，停止确认在实际回收后发送。清理只消费原计划索引，逐项执行并在宿主持久回执后允许下一项；进度带原操作和事件序号持续传递。随包组件与签名/公证流程已接线；CLI Terminal 通过系统 security 认证，由签名代理导入/复核凭证，宿主不接触密码，取消后回收进程组并恢复终端。原生安装/取消与文本授权验收仍未完成；三平台签名安装态验收继续开放；Windows 新删除路径只交叉验证，
不能据此关闭原生 UAC、NTFS/junction、跨账户和取消门禁。

文件系统已删除独立提权进程，普通 Handler 仅执行当前权限可完成的内容；受限项由 facade 的统一
CleanupAccess 重试并合并原累计结果。单独使用 Engine/Handler 不再隐式启动文件系统认证。
清理父端的 IPC 写入可取消，关闭时先释放管道，再等待进程树与读写线程退出。特权 Snapper 拒绝自定义
程序路径，系统工具由公共 trust 原语检查固定路径与所有权，特权命令不继承用户配置环境或工作目录。

Engine 在提交最终报告和 journal 前调用 `CleanupAccess::finish()`，显式确认本轮 native 连接停止。
关闭未确认保存为 `ExecutionReport.operation_failures` 中的 `cleanup-stop-unconfirmed`，附结构化授权结果；
该字段旧报告默认为空、空值省略，不改已提交的 group 或逐项结果。最终 journal 失败也作为操作级错误，
不能伪装成最后一个任务失败。`has_errors`、`failure_count` 计入操作级错误，成功项和大小仍只由原 group
汇总；自动化在已请求取消但关闭未确认时报告 Failed，而不是正常 Cancelled。恢复最终报告时逐组证据仍须
与既有 `group_finished` 完全一致，操作级错误随最终报告原样恢复，不重新执行清理。

执行结果和完成事件同时保留已知大小小计与未知处理量的项目数量；部分/不确定效果不能累计成已知零。
单项 `reclaimed_estimate=null` 时，`reclaimed_lower_bound` 保存可确认的处理量小计；完整估计已知时 lower bound 必须为 0。普通与授权阶段合并时累加不同阶段的已知小计，重复累计回执对账取最大已知值，不再次相加。

`ExecutionReport::unknown_reclaimed_items()` 汇总这些未知项目；外部插件必须通过协议 3 协商并提供与逐项回执一致的汇总，旧版本在执行前拒绝。
任务错误与其逐项失败采用统一 `ExecutionReport::failure_count()`，避免重复计数。

公共 facade 的执行日志 schema 4 为每项记录最新累计结果，并在开始记录中固定各任务组的插件计量方式；普通与 Linux/Windows/macOS 通用授权执行使用同一回执入口，
授权 worker 等待宿主落盘确认后才处理下一项。连接中断保留已确认效果，未确认项为 `Uncertain`，需要重新扫描核实。
详细实现与验证见 `docs/zh/development/ITEM-EXECUTION-JOURNAL-2026-10-12.md`。
明确已知零与未知、小计与完整大小分别呈现，失败或取消后的未处理项目不算成功大小。

需要在进程退出后显示已知结果的宿主，可实现 `ProgressSink::execution_journal_started`，持久保存 Engine 提供的
`ExecutionJournalReference`。该回调发生在删除派发之前；参与恢复的宿主无法保存关联时返回错误，Engine 不开始删除。
未实现回调的既有宿主继续使用默认空实现。

之后调用 `spaceprune::api::recover_execution(&reference)` 只读取原日志，不重新扫描、申请权限或执行清理。
提交完整最终报告时 `finished=true`；否则保留已提交前缀和已知大小下界，对未确认效果标为 `Uncertain`。
正常日志结尾前没有启动记录的组保留 `NotAttempted`；若后续记录损坏或截断，则不能据此前缀断言后续选择从未执行。
快照按日志中的插件计量元数据恢复，AlreadyAbsent 与已有卷空间观测分别保留。日志路径和计划 ID 是观察关联，
不能转换为执行能力。schema 3 缺少恢复所需的计量元数据，新读取入口明确返回 Unsupported，不按 Handler 名称猜测，
也不把旧活动按时间或相似路径自动关联到另一份日志。
恢复格式与验证见 `docs/zh/development/EXECUTION-RECOVERY-2026-10-16.md`。
`reclaimed_bytes` 与逐项大小仍为处理内容的估计。HandlerDescriptor 的 `size_accounting` 指定展示计量方式：缺省 `processed_bytes` 保持已有行为，Snapper 使用 `snapshot_groups`。Engine 按注册元数据投影 `ExecutionReport::size_summary()`，普通未知项不包含快照，原始逐项证据与 `unknown_reclaimed_items()` 保持不变。

Snapper 对同一配置合并一次 `delete --sync`，在该命令前后读取同一文件系统的空闲块数。批次 `space_changes` 只放入一个协调任务的结果，随最终 IPC 结果和执行日志保存；不将整批测量摊到每个快照或任务。不同配置按顺序测量，窗口不重叠。零、负变化与无法测量分别保留；文件系统身份改变后不计算差值。测量失败不阻断已有固定快照删除，同卷其他读写可能影响净变化，快照逻辑占用仍不能冒充释放空间。

## 15. filesystem Handler

filesystem Handler 的职责：

- 反序列化 roots、files、directories；
- 编译 include/exclude glob；
- 不跟随符号链接地扫描；
- 计算逻辑容量与最新修改时间；
- 应用未修改时长和容量推荐规则；
- 清理前重新建立 CleanupRoot 并校验每个项目。

执行约束：

- root 和 include 必须非空，目录 root 自身不能成为删除项目；
- 选择绑定扫描时的原生根与对象身份，普通与授权执行调用同一个 `remove_selected`；
- Unix 通过不跟随链接的目录描述符打开路径，选择对象先原子捕获，再核对身份；
- 选中目录的空后代目录直接原子删除，其余运行时后代逐项捕获到同一私有执行目录后遍历和删除，不冻结扫描时的叶子列表；
- 原相对路径用于排除判断，排除内容保留；取消、失败或目录仍有保留项时以 NOREPLACE 恢复，不覆盖新对象；
- Linux/Android 同时比较已持有描述符的设备和 mount ID，保留子卷边界并包含同设备 bind mount；捕获挂载点失败返回边界错误。

Linux 对深层选择增加未选祖先的 inotify 观察及捕获后的 getdents 通知屏障，移动和切走再切回均在删除前拒绝；
Linux 依据本机 VFS 的通知/锁顺序；macOS 已实现父目录 generation 与持续 vnode 事件检查，原生 APFS/HFS 验收仍待完成，见[实现与验收](MACOS-ANCESTRY.md)。恢复冲突或进程退出留下的槽位携带 NativePath
原路径映射，不覆盖同名新对象、不自动续删。实现范围及尚未通过的原生门禁见[捕获与边界审计](../development/VALIDATION.md)。
路径在进入 Handler 前已经完成变量和 `~` 展开。

## 16. Snapper Handler

`builtin.snapper/snapshots` 在 Linux 静态注册。它负责：

- 发现 Snapper 配置；
- 解析 JSON 快照列表；
- 将 pre/post 快照组成一个选择单元；
- 将 `#0`、active 和 default 快照标记为 unavailable；
- 在 `snapshots` 模式按 Recipe 条件推荐；在 `rust-targets` 模式将最新、manual、important 和 pre/post 配对快照标记为 optional；
- 按方案扫描时只读统计快照文件逻辑大小，或计算指定构建产物的引用容量；
- 执行删除并汇总失败。

该能力不在 `official:minimal` 中；配置必须显式引用。

Snapper 列表查询与文件逻辑大小统计分别进行。前者不访问快照内部文件，AI 的只读列表接口仍使用这项元数据查询；Core 的 `snapshots` 扫描使用 GNU du apparent-size 统计所选快照的文件元数据，权限失败后才按配置请求授权。共享数据块仍包含在逻辑大小中，不能计入可释放空间；统计失败保留未知，CLI 分组不把未知小计包装成已知零值。

Snapper 源码按 `inventory`（清单）、`scan`（扫描聚合）、`size`（计量）、`policy`（分组和推荐）、
`identity`（原选择与身份）、`selection`（选择保护）、`validation`（配置验证）、`command`（命令边界）
及 `cleanup`（生命周期执行）拆分。测试单独存放，领域模块不再自行启动 sudo/pkexec。
普通读取失败后由公共 metadata 服务按需授权；清理使用公共 cleanup 服务，保留执行前枚举、原身份、
配对完整性与批量部分失败处理。`api::snapper_configs` 和 `api::snapper_snapshots` 也使用公共服务，
领域插件只暴露不自行授权的清单原语。构建产物计量的原生文件名不经过显示字符串转换。

find/du 失败后，计量适配器在原深度、剪枝与文件系统范围内核对实际元数据访问，以 OS 错误判定是否需要授权，
不以命令文字代替原生依据。实际路径已消失时报告 NotFound，访问正常而命令失败时报告普通命令失败。
系统缓存的部分计量同时保留原生错误，未完整检查的总大小保持未知，可读字节数作为 issue 中的下限。

## 17. Linux 系统 Handler

Core 的 Linux filesystem 清理支持一次批处理中的按需提权：普通执行先返回真实结果，只将权限失败的已选项目交给公共清理服务。
服务启动固定的独立 `spaceprune-cleanup-helper`，不再提权 CLI/GUI，也不由主程序分派特权 worker。
组件不加载外部插件、AI 凭据或用户界面。授权由 sudo／Polkit 完成，私有标准输入输出传递有界请求和回执；
就绪后仍需要父进程确认启动，取消或断连会阻止未开始的操作，并协作停止已经开始的遍历。

提权 worker 对每个选择重新验证规则与扫描时的文件身份，以不跟随符号链接的目录描述符逐层删除，保留排除项并拒绝越过挂载点。成功回执与普通执行结果合并，不重复累计数量或大小；没有取得回执时如实保留未确认结果。主机显式选择图形授权模式，CLI 与 Snapper 可按系统 sudo 认证复用；不存在可以替代系统校验的持久权限 flag。

系统缓存与 Snapper 领域命令都不再自行启动认证器。读取、清理和随包组件自动准备共享公共权限基础设施，
仍保留不同能力与原操作期限；需要权限时自动应用当前随包组件，成功后继续原操作。详见[本机权限](NATIVE-AUTHORIZATION.md)。

`plugins/linux/system-linux` 注册（旧 Go 项为 developer-tools 实现的转发入口）：

```text
builtin.system-linux/package-cache
builtin.system-linux/journal
builtin.system-linux/tmpfiles
builtin.system-linux/coredumps
builtin.system-linux/go-modcache
```

每个 Handler 生成领域清理项，并在 clean 阶段验证项目 ID 仍与当前动作一致。任务 options 控制软件包管理器、
journal 保留时长和提权策略；软件包／systemd Handler 使用固定的系统命令集合，Go 转发入口保持原生身份绑定删除。这些能力同样不属于最小默认范围。

开发工具插件另提供 Linux `go-modcache`、`go-buildcache`、`cargo-registry-cache`、`npm-cache`，以及 Linux/Windows/macOS 桌面的 `docker-build-cache`、`docker-dangling-images`、`docker-unused-images`、`docker-stopped-containers`。Docker 使用本用户 CLI 的精确对象生命周期，不经过原生提权 helper，见 [Docker 清理](../usage/DOCKER.md)。下述目录身份实现仅适用于四个工具缓存。所有缓存操作复用原生目录身份和权限基础设施；Go 模块缓存保留专用同 UID worker，不运行 `go clean`。

## 18. 外部插件

应用层 `spaceprune::plugins` 将内置组与 PluginHost 的安装元数据投影成公共 `PluginCatalogEntry`，不执行插件。
全局偏好保存在用户配置目录的 `plugins.toml`，与可移植 Recipe 分开；实际启停变更递增 revision。
Engine 的独立禁用集合保留 Recipe 原值，扫描跳过禁用任务，明确任务选择和清理拒绝禁用 Handler。
应用会话在扫描、清理入口复核固定 revision，跨进程修改后必须重新加载与扫描；正在执行的调用不中途换策略。
详见 [插件启停实现记录](PLUGIN-PROTOCOL.md)。

外部插件目录包含 `plugin.toml` 与可执行文件。PluginHost：

1. 按平台标准目录发现清单；
2. 验证 Schema、ID、SemVer、平台、唯一 Handler 列表和包内可执行文件；
3. 只启动当前方案需要的插件；
4. 通过 JSON Lines 调用 describe、validate、scan 和 clean；
5. 校验请求 ID 与协议版本；
6. 把进度消息转发到统一 ProgressSink；
7. Ctrl+C 或连续五分钟无响应时终止插件进程。

插件包不允许符号链接。安装命令先复制到同一数据目录下的临时位置并完整校验，再原子重命名到最终目录。

## 19. 事件系统

EventBus 位于 core，围绕操作发出：

```text
scan-started
task-scan-started
task-scan-finished
scan-finished
cleanup-started
cleanup-group-started
cleanup-group-finished
cleanup-finished
```

事件包含 operation ID 和单调 sequence。监听器在发出线程同步执行，不能改变引擎控制流。

## 20. 自动化

配置使用零填充本地时间 `at = "HH:MM"` 定义每日计划。平台 facade 映射为：

- Linux：systemd user service + timer；
- macOS：LaunchAgent；
- Windows：当前用户计划任务。

安装文件只调用 `spaceprune --config <应用入口> automation run <id>`。`ConfigLoader` 会分别保留 Recipe 来源和
规范化后的应用 `config.toml` 来源，调度器只写入后者；这样 Recipe 切换后自动化会在下次运行时加载当前方案，
也不会把 `recipes/<ID>/<ID>.toml` 误传给只接受应用入口的 `--config`。直接通过 `load_recipe` 建立、没有应用
入口的会话不能安装自动化。平台 API 同时要求调用方显式传入支持 `automation run <id>` 的绝对 runner 路径；
CLI 传入自身，其他宿主必须传入随产品部署的 CLI runner，不能隐式使用当前 GUI 进程。运行时重新加载配置、
重新扫描并只执行 recommended。

`official:minimal` 不定义自动化，用户方案显式添加后才可安装。

## 21. CLI 与 TUI

CLI 使用 Clap 的运行时命令树进行两阶段解析：

1. 从应用 `config.toml` 的 `[theme].id` 加载终端主题，解析固定选项和管理命令；
2. 按应用配置的 recipe ID 加载清理方案，再注册并解析动态模块与选择器。

脚本可以绕过动态模块命令名，使用固定接口：

```text
spaceprune scan  [--module ID] [--selector ID ...] [--task ID ...]
spaceprune clean [--module ID] [--selector ID ...] [--task ID ...] --yes
```

`--selector` 与 `--task` 都要求 `--module`；前者按标签 AND 匹配，后者按任务 ID 精确匹配。固定子命令不能和
旧式模式标志组合。人类仍可使用由 Recipe 动态生成的 `spaceprune <module> --<selector> --scan`。

`--all` 只在 `spaceprune module list` 中表示包含禁用模块；普通配置模块仍可将 `all` 定义为动态选择器。

主题使用稳定 ID 持久化在应用配置的 `[theme]` 表中。`spaceprune theme` 使用 Ratatui 实时预览器，候选主题切换后
立即重绘，确认后才写入；`theme set <ID>` 直接设置，`theme list` 只读列出。

recipe ID 持久化在应用配置的 `recipe` 字段中。`spaceprune recipe` 交互选择本地方案，`recipe set <ID>` 直接设置，
`recipe list` 列出名称、路径和活动状态。两个 picker 与清理选择器复用同一个备用屏幕和终端恢复入口。

保留格式的 TOML 读写和 recipe 枚举实现在 `spaceprune-config`；CLI 只把主题调色板显式传入 Clap、文本输出、进度和
TUI。JSON 与非终端输出不添加 ANSI 样式。

CLI 进度订阅 Core 已有的扫描／清理生命周期事件，按实际任务结果更新计数；终端面板在操作期间定时刷新，
`progress/screen.rs` 在实时操作期间独占备用屏幕，用 Crossterm 同步帧重绘；不依赖缩放前的逻辑行数，也不查询光标位置或读取 stdin。系统输入开始、结束事件与 Drop 都恢复原屏幕和光标；嵌套输入结束后才重新进入进度画面。结束事件输出耗时摘要，再允许 stdout 报告或选择界面输出。Handler 的 ProgressEvent 只提供当前
步骤信息和自身已知的计数，不充当整盘覆盖率或猜测的清理百分比。filesystem 在配置 root、候选计量和本地
清理时报告位置与限频计数；Snapper 明确区分授权、逻辑大小统计和同步删除等待。

ProgressSink 新增默认空实现的输入状态通知；ProgressInputGuard 在系统命令读取终端密码期间成对暂停／恢复
实时输出，异常返回也会恢复。它只协调显示，不保存凭据、不授予权限；图形宿主和既有插件接收器兼容原接口。
后台刷新在选择、确认和普通报告期间停用，退出时恢复光标；非终端及禁用进度模式不启动动画线程。

TUI 使用 Ratatui/Crossterm，直接消费 ScanReport：

- 模块、任务和项目树；
- recommended 默认选中；
- optional 可切换；
- unavailable 只读；
- 空格切换、方向键或 `h/l` 展开当前行、`-`/`+` 收缩或展开全部模块、搜索和恢复推荐；
- 执行前再次确认。

进入 TUI 时通过终端标题栈保存原标题并设置交互清理标题，退出时恢复。项目键和模块/任务选择范围在进入
TUI 时缓存；每帧只构造一次可见行。`Ctrl+C` 在所有界面层统一取消。

## 22. 平台层

`spaceprune-platform` 是自动化 facade；`spaceprune-platform-linux` 提供 systemd 自动化；领域逻辑属于 `plugins/linux/system-linux` 与 `plugins/cross-platform/developer-tools`。原平台导出仅保留公共 Rust 调用入口的薄转发。
`spaceprune-engine` 和 `spaceprune-config` 保持平台中立。

移动目标只编译库层，不编译桌面 CLI、进程插件宿主和 Linux 专用能力。

## 23. 外部宿主与方案交换

应用通过 `spaceprune::api` 复用本机能力；服务可独立消费 `spaceprune-contracts` 的纯 bundle 合同，无需依赖执行引擎。

外部输入的完整 Recipe bundle 经过公共路径/大小验证后，仍需在目标设备完成配置解析、Handler/options 校验及扫描。
便携文件布局支持条件 include 和任务平台条件，不代表某个设备上的执行身份可在另一设备复用。
扫描结果、本次计划、权限连接和确认选择都留在本机执行边界内，网络同步不能替代这些步骤。

本机通用授权组件和 CLI 独立运行所需入口归公共仓库维护；各应用负责自身会话和界面交互。
安装协议与已验证的平台范围见 [原生授权](NATIVE-AUTHORIZATION.md)。

## 24. 验证

核心验证命令：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --workspace --release --locked
```

CI 还执行：

- actionlint；
- cargo-audit；
- Android 与 iOS 库目标检查；
- macOS 与 Windows 原生 Clippy 和测试。

当前功能、验证依据和已知限制见[功能核对表](FEATURE-MATRIX.md)。
本机协议与构建检查不能代替各目标平台的签名安装、授权、撤销和原生文件系统验证。

Release 工作流先调用同一份可复用 CI；全部 CI job 成功后，构建 Linux GNU/ARM64/musl、macOS
Intel/Apple Silicon 和 Windows x64 归档。

## 25. 扩展路径

新增普通清理需求：

1. 创建 `recipes/<ID>/<ID>.toml`；
2. 需要逻辑路径时创建用户 variable-map；
3. 使用 filesystem Handler；
4. 先运行 `spaceprune scan`；
5. 用 `recipe set <ID>` 选中并验证。

新增领域能力：

1. 在现有静态插件中增加 Handler，或创建外部进程插件；
2. 定义 options 与 recommend Schema；
3. 实现 validate、scan、clean；
4. 添加配置 recipe；
5. 验证扫描、推荐、选择和清理边界。

公共数据以 `spaceprune-contracts` 为唯一定义源。`spaceprune-config`、`spaceprune-plugin-api` 与 `spaceprune-engine` 重新导出对应类型，
facade 调用方无需维护结构副本。完整 bundle 的编辑通过 `rename_recipe_bundle` 和 `update_bundle_automation`
执行，保留在 config 的 `bundle_edit` 模块；纯合同不引入 `toml_edit`、磁盘 I/O 或运行时依赖。

扫描范围、任务／模块报告、计划观察、执行结果及大小汇总分别在合同的 `report`、`size_summary` 模块定义；
`export-schemas` 从这些 Rust 类型生成公共 JSON Schema。只消费报告的程序可直接使用纯合同，不必链接 Engine 或原生权限服务。
Engine 的 `ScanReport` 在纯报告外持有进程内单次消费标记，`into_report()` 返回不带该标记的数据；
`ScanReport::from_report(ScanReportData { ... })` 是宿主组装报告的入口，不签发 OS 权限。
`CleanupPlanData` 可序列化、反序列化和展示，真正的 `CleanupPlan` 仍由扫描选择构造，内容不可变，不能从 JSON 或数据 DTO 转成可执行计划。
现有 facade、序列化字段和单次执行检查保持不变；授权 lease、执行 grant 和取消对象不进入纯合同。

配置 crate 的入口仅保留常量与导出。`loader` 负责源加载/include 编排，`parse` 负责 fragment 与合并，
`resolve` 负责变量展开，`validate` 负责语义校验，`model` 保留应用和本机解析视图，`paths` 负责平台配置发现。
各领域测试与实现放在相应模块，存储集成回归放在 `store/tests.rs`。

外部插件使用 JSONL 协议 3。`spaceprune-plugin-host::process` 管理有界、可取消的双向进程传输，
`process::receipts` 将逐项结果绑定到原始选择并交给同一个执行日志。插件收到确认后才开始下一项，
最终汇总必须与逐项回执一致。纯结果校验由 `spaceprune-contracts::handler::execution` 提供，
进程内 Handler 和外部宿主共用。断流保留已确认效果，未确认效果要求重扫；详见 [插件协议](PLUGIN-PROTOCOL.md)。
