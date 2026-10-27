# Core 与 CLI 开发指南

本仓库是独立的 Rust workspace，提供 MIT 许可的 Core、CLI 与内置插件。构建和开发仅需本仓库及公开依赖。

## 1. 源码与工具链

```text
spaceprune/
├── crates/
│   ├── spaceprune/                 Core application API
│   ├── spaceprune-cli/             CLI and terminal UI
│   ├── spaceprune-config/          Configuration loading and storage
│   ├── spaceprune-engine/          Scan, planning and execution
│   ├── spaceprune-plugin-api/      Handler contract
│   ├── spaceprune-plugin-host/     External plugin host
│   └── spaceprune-platform*/      Platform services
├── plugins/                       Built-in filesystem and Snapper handlers
├── recipes/                       Embedded recipe resources
└── mappings/                      Platform variable mappings
```

工具链以 [rust-toolchain.toml](../../../rust-toolchain.toml) 为准，当前固定为 Rust 1.97.0；依赖版本以
`Cargo.lock` 为准。平台前置工具、调试／release 构建、目标目录和安装方法统一见
[构建、打包和分发](BUILDING.md)，本页不维护第二份平台构建命令。

## 2. Core API 与 CLI 的职责

其他 Rust 应用应依赖 `crates/spaceprune` 的 library，通过 `spaceprune::api` 使用 `SpacePrune`、
`ScanRequest`、`ConfigStore` 和结构化报告。CLI crate 负责 Clap、终端输入输出和 TUI，不是供其他 Rust 宿主复用的 API。

开发修改优先保持「读取配置 → 扫描 → 生成计划 → 确认后执行」闭环。只读扫描不删除文件，已取消或未完成的扫描
不能直接作为完整清理依据。Recipe 负责描述清理范围与推荐规则；Core 负责校验与执行，不承担 AI 的扫描规划。
详细模型见 [架构](../reference/ARCHITECTURE.md)、[事件系统](../reference/EVENTS.md) 和 [插件协议](../reference/PLUGIN-PROTOCOL.md)。

### Linux Go 缓存的 worker 分发

普通嵌入继续使用 `spaceprune::api`，不依赖 CLI。启用 `builtin.developer-tools/go-modcache`（或旧 `builtin.system-linux/go-modcache`）清理时，推荐将同一 Core 源码构建的 `spaceprune-cleanup-helper` 放在宿主可执行文件旁边，保留该文件名、可执行权限，且不使用 symlink。Core 自动发现并探测其能力，再以当前用户启动；普通只读 Go 模块清理无需先安装系统组件。具体构建和完整权限组件清单见[构建指南](BUILDING.md#core-库与移动目标)。CLI 已包含下述早期入口，单独 `cargo run` 也能处理本人 UID/GID 的只读 Go 模块。

能够控制进程 `main` 的 Rust 宿主，也可复用已有的窄 worker 入口，避免为普通用户 Go 清理另外分发 helper。在创建异步运行时、线程、GUI 或解析应用参数之前调用：

```rust
fn main() {
    if let Some(code) = spaceprune::run_go_cache_worker_if_requested() {
        std::process::exit(code);
    }
    // Initialize the host and use spaceprune::api normally.
}
```

该入口只处理本人的 Go 缓存 worker，不启动管理员清理会话。没有随包 helper、可用早期入口或受信任的新版已安装 helper 时，只读 Go 模块清理明确返回组件不可用；不会为了补齐文件而请求管理员，也不会回退 `go clean`。缺少 user namespace 能力或目录 GID 未映射等真实权限缺口，按 Recipe 的 `elevation` 策略处理；`never` 不请求授权。需要管理员能力的完整分发仍须附带匹配的读取、清理和安装三个组件，由既有授权流程按需安装或更新。

## 3. 按改动范围验证

修改局部模块时先运行相关 package 或具体测试，例如：

```bash
cargo test --locked -p spaceprune-config
cargo test --locked -p spaceprune-cli
```

修改公共配置、Engine 或跨 crate API 后，再扩大到完整 workspace：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

完整发布前检查、依赖审计和平台矩阵统一见 [CI 与 Release](CI.md)。测试通过不替代真实终端交互，
也不替代目标平台的运行验证。不要用真实用户资料验证不可逆删除；为清理测试创建隔离文件和独立 Recipe。

Android/iOS 当前只检查若干底层库目标，不提供移动应用构建结果。某个目标通过 `cargo check`，不能记录为该平台
可执行文件、安装包或系统授权功能已经验收。

## 4. 许可证材料

第三方清单和 `about.hbs` 位于 `licenses/rust/`；工具配置 `about.toml` 留在根目录，检查命令仍从仓库根目录运行。贡献指南与行为准则位于 `docs/zh/community/`。

Core/CLI 自有源码继续使用 [MIT License](../../../LICENSE)。Cargo 依赖 notice 由 `cargo-about 0.9.2` 与
`scripts/rust-licenses.sh` 维护；Rust 1.97.0 标准库材料由 `scripts/rust-stdlib-licenses.sh` 单独核对。
依赖、工具链或目标变化后应审阅材料差异；归档清单和检查命令见 [构建与分发材料](BUILDING.md#6-许可证签名和正式发布)。

分发要求按本仓库的实际依赖和产物维护。

## 5. 嵌入其他应用

使用公共 `spaceprune::api` 集成 Core；宿主自己的界面、服务和发布流程不属于本 workspace。更改公共 API 时验证受影响的调用约定。依赖声明保持可移植，不写开发机绝对路径；发行归档只包含本仓库及其必要分发材料。

## 6. 文档与进度

正式文档先按 `docs/zh/`、`docs/en/` 分语言，再按 `usage/`、`build/`、`reference/` 分类，开发计划、决策、问题排查与验证记录放在 `docs/{zh,en}/development/`。
修改命令、配置 Schema、执行边界或打包方式时同步更新对应文档。
代码标识符和新增注释使用英文；当前先维护中文文档，项目完成后补齐英文版本。

## 平台编译边界

系统专属实现与依赖按目标系统选择。Snapper、Linux 系统清理和 systemd 集成仅编译到 Linux；Windows 授权仅编译到 Windows。文件系统规则、Docker、配置、协议和打包描述在适用平台共用。Snapper 原生查询 API 仅在 Linux 提供。显式检查整个 workspace 时可能检查空的平台 crate，但不会编译其实现和依赖。

运行 `python scripts/check-platform-boundaries.py` 检查 Windows、Linux、macOS 的 CLI 依赖边界。构建脚本依据 `CARGO_CFG_TARGET_OS` 选择资源，不能根据构建主机判断目标；可移植的资源编译器仍作为宿主构建工具支持交叉编译，非 Windows 制品不会嵌入 MSI。
