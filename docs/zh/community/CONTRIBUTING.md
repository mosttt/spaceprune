# 贡献指南

感谢你为 SpacePrune Core 与 CLI 提交改进。本仓库包含可复用核心库、命令行程序、平台适配层和内置插件；
贡献应保持库与界面分层，并维持扫描只读、清理显式确认和路径边界不扩大的安全模型。

## 开始之前

- 先搜索现有 issue 和变更，避免重复工作。
- 涉及公共配置格式、插件协议、删除语义、跨平台行为或跨仓接口的较大变更，请先提交设计说明并等待维护者确认方向。
- 不要在 issue、日志、测试夹具或提交中放入 API key、密码、真实用户路径、个人 Recipe 或其他私密数据。
- 安全漏洞不要通过公开 issue 披露；请使用仓库托管平台提供的私密报告渠道。项目发布正式
  `SECURITY.md` 后，以其中的范围和流程为准。

## 本地环境

- 使用 `rust-toolchain.toml` 固定的 Rust 工具链。
- 使用仓库中的 `Cargo.lock`，不要在无依赖变更时重写锁文件。
- 不提交 `target/`、`dist/`、本机配置、测试报告或临时 Recipe。

## 实现边界

- 业务逻辑放在可复用库 crate；`spaceprune-cli` 只处理参数、终端交互和输出。
- 不恢复已经移除的 `prompt` 命令。脚本入口应继续支持明确的 `scan`、`clean` 和模块筛选参数。
- 扫描必须保持只读；执行清理前必须经过 Core 计划、边界验证和用户确认。
- 配置 include、Recipe bundle、插件输入和 filesystem 路径不得逃逸其声明根目录。
- 行为或公共接口变化应同步更新 `README.md`、相关 `docs/` 正式文档和 `docs/{zh,en}/development/` 过程记录。
- 新增或更新依赖时，应同步更新并验证第三方许可证材料。

## 验证

按改动范围运行最小相关测试；公共基础模块或跨 crate 变更应至少执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

修改移动端共享包时，再执行：

```bash
RUSTFLAGS="-D warnings" cargo check --locked --target aarch64-linux-android \
  -p spaceprune-plugin-api -p spaceprune-config -p spaceprune-engine \
  -p spaceprune-plugin-filesystem -p spaceprune-platform

RUSTFLAGS="-D warnings" cargo check --locked --target aarch64-apple-ios \
  -p spaceprune-plugin-api -p spaceprune-config -p spaceprune-engine \
  -p spaceprune-plugin-filesystem -p spaceprune-platform
```

依赖或发布材料变化时，再执行：

```bash
scripts/rust-licenses.sh check
scripts/rust-stdlib-licenses.sh check
```

交叉 `cargo check` 只证明目标代码可检查编译，不能替代对应系统的安装、签名或运行验收。

## 提交变更

Pull request 应说明：

- 问题、预期行为和实现范围；
- 受影响的 crate、平台、配置或 Recipe schema；
- 已执行的测试及结果；
- 对扫描、清理、权限和路径边界的影响；
- 已同步更新的正式文档和过程记录；
- 尚未执行的目标系统或外部服务验收。

保持变更聚焦，不要把无关格式化、生成产物或本机配置混入同一提交。提交前请确认你有权按照仓库
`LICENSE` 中的 MIT 条款贡献相关内容。
