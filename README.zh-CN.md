# SpacePrune

[English](README.md)

SpacePrune 是以清理方案驱动的开源 CLI，同时提供可复用的 Rust Core 库与插件。用 TOML 定义清理规则，先查看只读扫描结果，再选择并确认要删除的内容。新增文件后，仍可重复使用同一份方案。

本地扫描与清理可离线运行，无需账号或 AI 服务。本仓库自有代码采用 MIT 许可证。

## 快速开始

需要 Rust 1.97。从仓库根目录构建 CLI 和三个配套授权组件：

```bash
cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper
```

先扫描；扫描不会删除文件：

```bash
./target/release/spaceprune scan
```

进入交互清单，自行选择清理内容：

```bash
./target/release/spaceprune --select
```

也可以仅清理推荐项，并在提示中确认：

```bash
./target/release/spaceprune clean
```

未创建个人方案时，内置基础方案在 Linux/macOS 检查用户缓存目录，推荐 30 天未修改的直接子项；Windows 检查用户临时目录，推荐 7 天未修改的直接子项。目录的未修改时长取决于其中最近修改的内容。其他清理能力需要在方案中明确配置。

Linux 源码调试可使用 `./scripts/run-cli.sh`，每次启动前会一起构建 CLI 与匹配组件。Windows/macOS 原生授权还需要打包与签名；平台依赖、目标架构和分发命令见[构建与打包](docs/zh/build/BUILDING.md)。

## 主要功能

- **先检查再清理**：按模块、文件夹和文件浏览，查看大小、诊断与推荐理由，支持按组或逐项选择。
- **方案可复用**：用 TOML 配置范围、排除项、时间与大小条件，支持 include、平台条件和变量映射。
- **终端与脚本**：交互选择、稳定的 `scan`/`clean` 命令、JSON 输出、取消操作、中英文语言偏好及四套终端主题。
- **内置插件**：通用文件规则、Snapper 快照、Linux 系统资源，以及开发工具缓存／Docker。各 Handler 的适用平台不同，也可通过外部进程插件扩展。
- **定时清理**：通过 Linux systemd 用户服务、macOS LaunchAgent 或 Windows 计划任务执行已配置的推荐规则。
- **Core 可嵌入**：其他程序可通过 `spaceprune::api` 复用能力，也可独立消费纯数据合同，无需终端界面。

保存方案不会删除文件。手动清理需要确认；已启用的定时任务会按配置执行推荐规则，不会每次重新询问。对于共享存储等无法直接推算释放空间的情况，逻辑大小与实际回收空间分别报告。

## 源码布局

- `crates/`：CLI、Core 库、数据合同与原生授权组件。
- `plugins/`：文件系统、Snapper、Linux 系统和开发工具插件。
- `recipes/` 与 `mappings/`：内置方案及平台变量映射。
- `scripts/` 与 `.github/workflows/`：构建、打包和发布工具。
- `docs/en/` 与 `docs/zh/`：使用、架构、构建与贡献指南。
- `licenses/`：第三方许可及分发材料。

## 文档

从[文档索引](docs/zh/README.md)开始，也可直接查看：

- [CLI 命令与终端操作](docs/zh/usage/CLI.md)
- [Recipe 编写指南](docs/zh/usage/RECIPE-GUIDE.md)与[配置参考](docs/zh/usage/CONFIGURATION.md)
- [架构](docs/zh/reference/ARCHITECTURE.md)与[插件协议](docs/zh/reference/PLUGIN-PROTOCOL.md)
- [构建与打包](docs/zh/build/BUILDING.md)、[安装与自动化](docs/zh/build/DEPLOYMENT.md)、[CI 与发布](docs/zh/build/CI.md)
- [开发指南](docs/zh/build/DEVELOPMENT.md)、[功能与平台范围](docs/zh/reference/FEATURE-MATRIX.md)、[验证状态](docs/zh/development/VALIDATION.md)

参与开发请阅读[贡献指南](docs/zh/community/CONTRIBUTING.md)与[行为准则](docs/zh/community/CODE_OF_CONDUCT.md)。

## 许可证

自有源码采用 [MIT License](LICENSE)。第三方依赖遵循各自许可证，见[许可材料](licenses/README.zh-CN.md)与[分发要求](docs/zh/build/DEPLOYMENT.md)。
