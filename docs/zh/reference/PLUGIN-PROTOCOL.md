# SpacePrune 外部插件协议

外部插件使用 `protocol-version = 3` 的 JSON Lines 进程协议。普通路径规则优先使用
`builtin.filesystem/paths`；外部插件用于配置无法表达的领域扫描和清理逻辑。

需要只读查看内置能力时，可使用 `spaceprune_contracts::plugin::BUILTIN_PLUGIN_SPECS`：声明包含版本、支持系统与该系统的完整处理器 ID，不启动插件。实际启停、安装状态和授权仍由宿主管理，外部插件从其真实 Manifest 读取元数据。

## 1. 插件目录

```text
spaceprune-plugin-example/
├─ plugin.toml
└─ spaceprune-plugin-example
```

`plugin.toml`：

```toml
schema-version = 1
id = "community.example"
name = "Example SpacePrune"
version = "1.0.0"
protocol-version = 3
executable = "spaceprune-plugin-example"
platforms = ["linux", "macos", "windows"]
handlers = ["cache", "history"]
```

| 字段               | 说明                           |
| ------------------ | ------------------------------ |
| `schema-version`   | 固定为 `1`                     |
| `id`               | 全局插件 ID                    |
| `name`             | 显示名称                       |
| `version`          | SemVer                         |
| `protocol-version` | 固定为 `3`                     |
| `executable`       | 插件目录内的相对可执行文件路径 |
| `platforms`        | 可省略，默认空数组表示平台中立 |
| `handlers`         | 局部 Handler ID 数组           |

ID 只允许小写 ASCII 字母、数字，以及分隔用的点和连字符；分隔符不能位于首尾或连续出现。
平台只接受 `linux`、`macos`、`windows`、`android`、`ios`。

`name` 不能为空，`version` 必须是 SemVer，Handler ID 必须唯一。`executable` 必须是插件目录内的
普通文件，不能是绝对路径、包含 `..` 或通过符号链接逃出插件目录。`describe` 返回的完整 Handler 集合
必须与 manifest 严格一致。

## 2. 搜索与安装

搜索目录按优先级：

| 平台           | 目录                                                                                                                                                        |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 通用最高优先级 | `$XDG_DATA_HOME/spaceprune/plugins/`                                                                                                                        |
| Linux          | `$HOME/.local/share/spaceprune/plugins/`，`/usr/lib/spaceprune/plugins/`                                                                                    |
| macOS          | `$HOME/Library/Application Support/SpacePrune/plugins/`，`/Library/Application Support/SpacePrune/plugins/`                                                 |
| Windows        | `%LOCALAPPDATA%\SpacePrune\plugins\`；LOCALAPPDATA 不可用时使用 `%USERPROFILE%\AppData\Local\SpacePrune\plugins\`；然后 `%PROGRAMDATA%\SpacePrune\plugins\` |
| Android        | `$HOME/.local/share/spaceprune/plugins/`                                                                                                                    |
| iOS            | `$HOME/Library/Application Support/SpacePrune/plugins/`                                                                                                     |

缺失对应环境变量时跳过该目录。靠前目录中的同 ID 插件优先。

安装只接受本地路径：

```bash
spaceprune plugin install ./spaceprune-plugin-example
spaceprune plugin list
spaceprune plugin inspect community.example
spaceprune plugin remove community.example
```

插件包只允许普通文件和目录，不允许符号链接。安装会先复制到用户插件根目录中的临时目录并验证完整清单，
然后原子移动到最终 ID 目录；失败不会留下可发现的半安装目录。

安装会复制完整插件目录到当前用户数据目录。已有 ID 不会被覆盖。

`spaceprune plugin list --output=json` 返回已发现插件列表，`plugin inspect <id> --output=json`
返回其中一个插件。两者使用公共 `spaceprune_contracts::plugin::InstalledPlugin`，包含 `manifest`、
`manifest_path` 和 `executable`；清单字段定义为同模块的 `PluginManifest`。原有
`spaceprune_plugin_host::{PluginManifest, InstalledPlugin}` 导入路径继续可用。

`schemas/installed-plugin.schema.json` 从上述 Rust 类型导出，可用于生成宿主消费类型。
源清单中省略 `platforms` 仍合法，发现结果序列化时会包含 `platforms: []`；路径保持原有原生路径序列化形式。
Schema 描述数据形状，版本、SemVer、路径边界和 Handler 一致性仍由宿主校验。读取元数据不会启动插件进程。

### 统一能力目录与启停

`spaceprune plugin catalog --output=json` 返回 `PluginCatalogEntry[]`，包含内置文件系统、Snapper、Linux 系统资源与已安装外部插件。
内置组不能单独卸载；不支持当前平台的能力仍展示，但不能启用。`supported` 描述平台支持，不代表系统工具已安装或已经获得权限。
`enabled` 是设备本地偏好；有效能力还要求 `supported=true`。`conflicts_with_builtin` 标识冒用内置保留 ID 的外部安装，
该条目始终 `enabled=false`，但平台字段仍如实显示。完整形状从 `schemas/plugin-catalog-entry.schema.json` 生成。

`builtin.filesystem`、`builtin.snapper`、`builtin.system-linux`、`builtin.developer-tools` 是应用保留 ID，新的外部安装在创建目标目录前即被拒绝。
已经存在或手工放入的冲突包保留在目录中，与真实内置能力分别展示，不注册或启动其 Handler。
可以用 `plugin remove <id>` 删除冲突外部安装，不会删除内置能力；`plugin enable/disable <id>` 始终调整同名真实内置能力。
删除先使用用户插件根下的规范 `<id>` 目录；该路径不存在时，按 manifest ID 找到用户根下实际发现的普通子目录，
每次仅删除一个。系统级安装不由这个用户卸载入口移除。
宿主应使用 `kind` 与 `id` 的组合标识目录条目。

使用 `spaceprune plugin disable <id>` 或 `enable <id>` 保存偏好。缺省全部启用；停用不会卸载文件、改写 Recipe、移除版本声明或同步到其他设备。
停用插件不参与后续扫描与清理，也不启动其进程。安装本地插件仍只安装文件；真正执行只发生于使用该插件的已启用任务。

## 3. 进程传输

主程序启动 manifest 指定的可执行文件：

- stdin：主程序写请求；
- stdout：插件写进度事件和最终响应；
- stderr：插件诊断输出，继承到 SpacePrune；
- 每行一个 UTF-8 JSON 对象；
- 一个 SpacePrune 运行周期复用同一插件进程；
- 同一插件请求串行发送；
- stdout 不允许输出协议外文本。

所有消息包含：

```json
{ "protocol": 3 }
```

协议值不为 `3`、JSON 无效、请求 ID 不一致、缺少最终结果或进程提前退出都会使当前调用失败。协议不进行版本协商。

## 4. 请求

```json
{
  "protocol": 3,
  "id": 1,
  "method": "describe",
  "params": {}
}
```

- `id` 是该进程内单调递增的无符号整数；
- `method` 是 `describe`、`validate`、`scan` 或 `clean`；
- `params` 是方法参数对象。

## 5. 最终响应

成功：

```json
{
  "protocol": 3,
  "id": 2,
  "result": {
    "items": [],
    "issues": []
  }
}
```

失败：

```json
{
  "protocol": 3,
  "id": 2,
  "error": {
    "code": "scan-failed",
    "message": "Could not read the target directory"
  }
}
```

成功必须包含 `result`；失败必须包含稳定的 `error.code` 和英文 `error.message`。响应 ID 必须等于请求 ID。

## 6. 进度事件

进程内宿主可通过 `ScanAccess::manages_authorization` 接管某个任务的读取授权。Engine 此时调用 Handler 的 `scan_unprivileged`，再由同一宿主策略决定是否重试；普通 `scan` 和未接管的 Handler 保持原行为。Snapper 的无提权入口保持任务配置及选择身份，权限不足作为扫描问题返回，不单独显示认证窗口。最终重试结果仍经过 Engine 的项目 ID／层级校验。这是进程内 Rust API，不修改外部插件协议 3。

插件可以在最终响应前发送：

```json
{
  "protocol": 3,
  "event": "progress",
  "request_id": 3,
  "data": {
    "phase": "scan",
    "module_id": "example",
    "task_id": "cache",
    "handler_id": "community.example/cache",
    "current": 3,
    "total": 10,
    "message": "Scanning directory 3"
  }
}
```

`phase`：

```text
load  scan  measure  plan  authorize  clean  complete
```

总量未知时 `current` 和 `total` 可以为 null。`request_id` 必须引用当前请求。

## 7. describe

请求：

```json
{ "protocol": 3, "id": 1, "method": "describe", "params": {} }
```

结果：

```json
{
  "handlers": [
    {
      "id": "community.example/cache",
      "name": "Example cache",
      "description": "Scan cache produced by Example",
      "platforms": ["linux"],
      "options_schema": {
        "type": "object",
        "properties": {
          "root": { "type": "string" }
        }
      },
      "recommend_schema": {
        "type": "object",
        "properties": {
          "older-than": { "type": "string" }
        }
      }
    }
  ]
}
```

Schema 用于描述和展示。插件仍必须在 validate 中执行真实校验。

Handler 可声明 `size_accounting`：`processed_bytes`（缺省）使用处理大小与未知数；`snapshot_groups` 单独统计成功处理的快照选择组，不混入普通文件未知数。此元数据只改变展示汇总，不改写逐项结果或删除权限。

## 8. TaskRequest

`validate`、`scan` 和 `clean` 共享：

```json
{
  "module_id": "example",
  "task_id": "cache",
  "name": "Example cache",
  "handler_id": "community.example/cache",
  "options": {
    "root": "/home/user/.cache/example"
  },
  "recommend": {
    "older-than": "30d"
  },
  "tags": ["example", "cache"]
}
```

ConfigLoader 在构造 TaskRequest 前已经：

- 合并 include；
- 合并内置与用户变量映射；
- 展开当前平台任务 options 中的 `${NAME}` 和开头 `~`。

因此插件收到最终选项，不应再次把 `${NAME}` 当作进程环境变量解析。`recommend` 保持配置原值。

`tags` 是任务属性；选择器 `match-tags` 不进入 TaskRequest。

## 9. validate

```json
{
  "protocol": 3,
  "id": 2,
  "method": "validate",
  "params": {
    "task": {}
  }
}
```

成功：

```json
{ "protocol": 3, "id": 2, "result": null }
```

插件应验证 options、recommend、平台和自身约束，不在 validate 中扫描或修改资源。

## 10. scan

```json
{
  "protocol": 3,
  "id": 3,
  "method": "scan",
  "params": {
    "task": {},
    "context": {
      "operation": { "operation_id": "scan-operation-id", "generation": 0 },
      "interaction": "terminal",
      "remaining_ms": 300000
    }
  }
}
```

`task` 使用第8节的完整结构；这里省略其字段。`context.operation` 是宿主提供的原操作身份与代次，`interaction` 为 `foreground`、`terminal` 或 `non_interactive`，`remaining_ms` 为请求发出时的剩余期限毫秒数。示例数值不是固定预算，插件不能重置或延长它，也不能把交互模式当成提权许可。宿主仍独立执行取消与时限控制。

结果：

```json
{
  "items": [
    {
      "id": "cache-2026-01",
      "parent_id": null,
      "name": "January cache",
      "location": "/home/user/.cache/example/2026-01",
      "logical_bytes": 1048576,
      "reclaimable_bytes": 1048576,
      "modified_at": 1769904000,
      "recommendation": "recommended",
      "reason": "Older than thirty days",
      "tags": ["example", "cache"],
      "opaque": { "cache_id": "2026-01" }
    }
  ],
  "issues": [
    {
      "level": "warning",
      "message": "One directory is unreadable",
      "location": "/restricted"
    }
  ]
}
```

约束：

- item ID 在当前任务内唯一；
- parent ID 只能引用同一结果；
- 未知容量使用 null；
- modified_at 是 Unix 秒或 null；
- recommendation 为 recommended、optional 或 unavailable；
- opaque 由插件定义，核心原样交回；
- 局部问题进入 issues，不阻止其他结果；error 级问题会使本次扫描返回非零状态；
- 不可执行资源标记为 unavailable。

核心将没有子项引用的项目视为叶子选择单元。

`CleanupItem.logical_size_applicability` 是可选的通用逻辑大小语义字段：缺省 `applicable`，默认序列化不写此字段；`not_applicable` 表示领域明确声明该对象不适用逻辑大小。`logical_bytes` 仍保留原合同：已测量值（包括 `0`）或 `null`。不能用 `0` 冒充不适用。

宿主只按公开字段呈现，不按处理器 ID、名称、编号或 `opaque` 猜测。不适用项不计入逻辑大小未知数；非空且全部不适用的分组显示 N/A，空组仍为 `0 B`。混合分组继续统计适用项的已知容量与未知数。`LogicalSizeSummary::from_items` 与任务/模块的 `logical_size_summary()` 提供一致统计。

此字段仅声明逻辑大小，不改变 `reclaimable_bytes`、推荐状态或选择权限。例如当前系统是不可选的 N/A 项；真实快照即使受保护，逻辑大小仍适用，未完成测量时仍为未知。

## 11. clean

```json
{
  "protocol": 3,
  "id": 4,
  "method": "clean",
  "params": {
    "task": {},
    "items": [
      {
        "id": "cache-2026-01",
        "opaque": { "cache_id": "2026-01" }
      },
      {
        "id": "cache-2026-02",
        "opaque": { "cache_id": "2026-02" }
      }
    ]
  }
}
```

实际 task 和 items 使用完整公共结构。插件只解释自己生成的 opaque，并重新验证资源仍可安全操作。

结果必须包含全部选中项的 outcomes，且与已确认回执完全一致：

```json
{
  "outcomes": [
    {"item_id":"cache-2026-01","status":"succeeded","reclaimed_estimate":1048576,"failure":null},
    {"item_id":"cache-2026-02","status":"failed","reclaimed_estimate":0,"failure":{"item_id":"cache-2026-02","message":"Permission denied"}}
  ],
  "attempted": 2,
  "succeeded": 1,
  "reclaimed_bytes": 1048576,
  "unknown_reclaimed_items": 0,
  "failures": [
    {
      "item_id": "cache-2026-02",
      "message": "Permission denied"
    }
  ]
}
```

单项失败进入 failures；只有整个调用无法继续时使用顶层 error。必须满足
`attempted = succeeded + failures.length`。未发生取消时，`attempted` 还必须等于请求中的项目数；发生取消时，
未处理项目不计入 `attempted`。

`reclaimed_bytes` 保留原字段名，表示已知处理内容的大小估计小计，包括可确认的部分处理；不是实际磁盘空闲增量。
`unknown_reclaimed_items` 表示成功、部分处理或效果未确认项目中大小未知的数量，不得大于 attempted。
不存在和未尝试项不增加空间；已知零与未知大小必须区分。协议 3 返回显式未知数量，并与逐项 outcomes 相符。
`reclaimed_estimate` 是完整处理量估计；为 null 时，`reclaimed_lower_bound` 保留已经确认的大小小计（缺省为 0）。
完整估计已知时 lower bound 必须为 0，不能同时累加两字段。成功、部分完成或效果未确认都可以有未知总量与已知小计；
例如先确认处理 6 字节、重试结果未确认，记录 estimate=null、lower_bound=6，汇总为至少 6 字节及 1 个大小未知项目。
同一原始选择跨阶段仍只算一个未知项目，重传回执不得重复计量。协议 3 与旧版本在启动阶段拒绝互用。
Rust 插件可使用 `HandlerCleanResult::record_outcome` 从每项结果计算一致汇总。

清理结果可附带 `space_changes`（缺省空数组），记录实际批次所在卷的空间变化。每条包含 `scope`、`freeBytesBefore`、`freeBytesAfter` 和 `error`；无法读取的测量值为 null，原因放入 error。两个值均为同一文件系统的空闲字节数，不是快照逻辑大小。每个不重叠批次只记录一次；共享批次不能在每个任务或项目重复附加。宿主单独显示这一观测，不累加到 reclaimed_bytes。旧结果没有该字段时不推定释放了 0 B。

进程内 Rust Handler 还可调用 `ProgressSink::record_cleanup(task, succeeded, reclaimed_bytes, unknown_size_item_count)`，发布同一任务到当前时刻的累计成功小计。宿主按任务覆盖旧小计后汇总，不能将每次回调重复相加；只有确认处理效果后才增加数值。该方法默认不执行操作，不改变协议 3 的 JSON 进度帧或已有插件合同。没有实时累计回调的插件，仍在最终任务结果经过 Engine 校验后更新大小；不能从普通进度百分比推算已清理空间。

协议 3 不接受旧插件只返回累计成功数。每处理完一个选中项，插件必须发送：

```json
{"protocol":3,"event":"item_finished","request_id":4,"data":{"item_id":"cache-2026-01","status":"succeeded","reclaimed_estimate":1048576,"failure":null}}
```

宿主核对请求、原始选择、结果状态和大小，并将逐项结果交给执行日志，然后回复：

```json
{"protocol":3,"event":"item_receipt","request_id":4,"data":{"item_id":"cache-2026-01","accepted":true}}
```

插件必须等待相同请求和项目的 `accepted=true` 后才可处理下一项。拒收、EOF 或连接失败立即停止。
不能把原始 progress 文本当回执，也不能补报未选项目、重复项目，或用最终汇总改写已确认结果。

`status` 支持 `succeeded`、`already_absent`、`partially_applied`、`failed`、`uncertain`、`not_attempted`。
不存在和未尝试项的 `reclaimed_estimate` 必须为 0；无法确认的实际处理大小使用 null。
失败、部分处理和未确认效果带同一项目的 failure；成功不带 failure。

进程中途退出时，Engine 保留已确认的逐项结果，其余未确认项标为 uncertain，要求重扫而非自动重试。
已确认成功不会被旧失败汇总覆盖。清理不是事务回滚：取消不能恢复已经删除的内容。

协议 3 没有 cancel 帧，取消会终止整棵插件进程树，并等待进程及 stdin/stdout 线程收尾。调用使用共享
OperationContext 的取消信号和截止时间：插件总时限上限 15 分钟，首次响应与响应间隔上限各 5 分钟；
与调用者的剩余期限和预算逐项取较小值，持续 progress 不延长总时限。基本 native/tool 操作的默认预算为
首次/空闲输出 60 秒、累计输出 8 MiB；完整应用工作流采用 5 分钟、64 MiB。无论调用者如何配置，
插件仍受下述独立上限约束。
stdout 每行上限 1 MiB，每次调用累计上限 64 MiB，接收队列最多 16 行；请求最多 16 MiB。
stdin 写入及串行调用等待同样受取消和截止时间约束。协议错误或调用失败后连接停止，后续调用不会复用旧响应。

进程内 Rust Handler 的 scan/clean/batch、ScanAccess/CleanupAccess 入口现在接收 `&OperationContext`，
原生循环在操作边界检查取消和 deadline。纯配置 `validate` 保留；需要 I/O 的验证器必须覆盖
`validate_in_context`，插件 describe/validate 与注册启动使用装配者的原期限。JSONL 协议为 3；scan
参数既有 `context` 对象携带 operation identity、interaction、remaining_ms，插件不能借此自行提权。

这是有版本的破坏性协议变更：manifest 的 `protocol-version=1` 或 `2` 在执行前明确拒绝，插件需同时实现
协议 3 的逐项确认和最终 outcomes，不能只修改版本号。元数据 helper 协议 6、清理授权 helper 协议 4 保持独立；
helper 升级用于跨进程保留操作身份和单调期限，不改变外部插件协议。旧只读组件通过版本探针明确拒绝，
需使用同一 Core 修订的组件；不会降级使用旧协议。

## 12. 生命周期

```text
启动进程
  -> describe
  -> validate（每个活动任务）
  -> scan
  -> SpacePrune 生成计划
  -> clean（仅有选中项时）
  -> SpacePrune 退出并终止进程
```

同一插件的多个 Handler 共享进程。插件可以缓存只读发现结果，但 clean 必须以本次 task 和 items 为准。

进度事件用于展示。插件不能发送核心 `scan-finished` 等生命周期事件；这些事件由 spaceprune-engine 统一产生。

## 13. 信任与失败边界

- 外部插件必须由用户显式本地安装；
- 安装表示信任其可执行代码；
- 插件崩溃不会破坏 SpacePrune 主进程内存，但当前调用失败；
- 缺失插件或版本不匹配在执行前报错；
- SpacePrune 不为失败 Handler 猜测替代命令；
- 插件不读取终端确认，不决定全局方案范围，不解析根 include。

核心事件契约见 [事件系统](EVENTS.md)。

Handler 请求、描述、推荐状态、扫描/清理结果和进度 DTO 的唯一定义已迁至
`spaceprune-contracts::handler`，`spaceprune-plugin-api` 继续重导出这些类型并保留 Handler、
ScanAccess、ProgressSink 与 CancellationToken 等运行时接口。外部进程协议为 3，包含逐项回执与明确的未知大小要求；未知大小始终保持未知。
