# 原生文档适配器

`spaceprune-plugin-mobile-documents` 将 `builtin.mobile-documents/documents` 接入现有 Engine。宿主必须显式注册 `DocumentProvider`；桌面 CLI 不因此宣称具备 Android 能力，也不把 content URI 当作文件系统路径。

适配器将原生存储映射为目录绑定、分页枚举、检查和删除文件操作。观察结果统一使用文档身份、父子关系、可选逻辑大小与修改时间。未知仍为未知，真实零仍为零；规则、推荐、已确认选择计划和逐项执行回执沿用共享 Core 流程。

方案保存可复用的资源槽、文件名匹配、排除、递归及年龄／大小条件；每台设备自行授权并绑定槽位。URI、系统授权、绑定版本、扫描文档 ID 不进入可复用规则。时间与大小都启用时须同时满足，缺少元数据不能视作满足条件。

```toml
[modules.documents.tasks.logs]
handler = "builtin.mobile-documents/documents"
platforms = ["android"]

[modules.documents.tasks.logs.options]
resource = "saved-logs"
relative-directory = ["logs"]
recursive = true
include-names = ["*.log"]
exclude-names = ["keep-*"]

[modules.documents.tasks.logs.recommend]
older-than-days = 30
min-size-bytes = 1048576
```

删除已确认文件前，原生适配器须重新检查绑定、当前祖先关系、文档身份与观察到的元数据。根目录、目录、虚拟文档及只读项不能删除；重新绑定、撤销授权使旧计划失效，新增文件需要重新扫描。计划仅消费一次，不确定或中断的原生操作不能自动重放。停止后不再派发后续删除，已派发操作等待其真实结果。

已处理逻辑大小不保证等于磁盘可用空间变化。Android 文档提供方不一定提供大小、时间、原子比对删除或可取消删除；宿主必须在回执中保留这些区别。其他系统可通过原生适配器复用相同的上层选择与执行语义。

运行 `cargo test --locked -p spaceprune-plugin-mobile-documents` 可验证十项 Engine／Provider 夹具，覆盖匹配、排除、未知元数据、撤销／重绑、祖先变化、精确选择、取消和不确定结果。真实系统界面及文档提供方验收由嵌入宿主完成，这些夹具本身不代表 Android 或 iOS 设备支持。

`relative-directory` 可选，表示授权资源根下的逐级目录名称；不接受绝对路径、`..` 或路径分隔符。扫描只进入该相对范围，清理前重新核对已观察到的每级祖先身份和名称。祖先变化会使旧选择失效，因此子目录规则可以复用和迁移，原生文档标识仍留在本机。
