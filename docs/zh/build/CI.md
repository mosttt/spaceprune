# SpacePrune CI 与 Release

本文说明开源 Core/CLI 的 `.github/workflows` 定义、发布目标和维护方式。本仓库的 Release 工作流只构建和发布 Core／CLI。各平台本地编译与手工归档步骤见 [构建、打包和分发](BUILDING.md)。

这些工作流面向 GitHub.com 与 GitHub-hosted runners。只有仓库位于 GitHub、启用 Actions 并将 tag
推送到该 GitHub 仓库后才会运行。

## CI 工作流

文件：[ci.yml](../../../.github/workflows/ci.yml)

触发条件：

- `main` 分支 push；
- pull request；
- 手动 `workflow_dispatch`；
- 由 Release 工作流通过 `workflow_call` 调用；
- 每周一 03:23 UTC 的依赖安全审计。

内部功能分支通过 pull request 触发检查，不会同时因为分支 push 重复运行一套工作流。

### Quality

Ubuntu 24.04 上使用项目声明的 Rust 1.97.0 执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --workspace --release --locked
```

该任务还会用固定版本的 actionlint 检查 GitHub Actions 语法与表达式，并对下列移动端库层目标执行
`cargo check`：

```text
aarch64-linux-android
aarch64-apple-ios
```

移动端检查只包含 `spaceprune-plugin-api`、`spaceprune-config`、`spaceprune-engine`、filesystem Handler 和平台 facade，
不构建 Android 或 iOS 可执行文件。

Quality 还会安装 `cargo-about 0.9.2`，执行 `scripts/rust-licenses.sh check` 与
`scripts/rust-stdlib-licenses.sh check`，核对归档需要携带的 Cargo 依赖和 Rust 标准库材料；
`python3 -m unittest discover -s scripts/tests` 验证可移植的原生打包／签署生命周期，不替代目标系统安装。

### Dependency security

依赖安全审计是独立 job，执行 `cargo audit --deny warnings`。它不会因格式、Clippy、测试或构建提前失败而被
短路；每周定时触发也会独立刷新 RustSec 数据库。

### macOS 与 Windows 原生检查

非定时触发时还会在真实 GitHub-hosted 系统上运行 Clippy 和测试：

工作流中的 `desktop-platforms` job 在 Linux、Windows 和 macOS 上检查 Core/CLI workspace。

| Runner         | 用途                 |
| -------------- | -------------------- |
| `macos-15`     | macOS ARM64 原生测试 |
| `windows-2025` | Windows x64 原生测试 |

每周定时任务只运行 Ubuntu quality 和 security，避免为了更新 RustSec 数据库重复消耗全部桌面矩阵时间。

## Release 工作流

文件：[release.yml](../../../.github/workflows/release.yml)

推送 `v*` tag 时触发。执行顺序为：

1. 调用完整 CI 工作流；
2. CI 的 Ubuntu 质量、独立安全审计、移动目标检查、macOS 和 Windows 原生检查必须全部通过；
3. tag 必须以 `v` 开头；
4. 去掉 `v` 后必须与 `spaceprune-cli` Cargo package version 完全一致；
5. 构建发布矩阵；
6. 所有构建成功后，先创建草稿 Release，上传全部归档与校验和，再公开发布；已有 Release 不会被覆盖。

CI 失败时，tag 校验、`build` 和 `publish` job 都会被跳过。Release 不依赖另一次历史 CI run 的状态，而是
在 tag 对应提交上调用同一个可复用 CI 工作流，因此发布检查与日常 CI 使用同一份定义。

例如 Cargo 版本为 `0.1.3` 时，正确 tag 是：

```bash
git tag -a v0.1.3 -m "SpacePrune v0.1.3"
git push origin v0.1.3
```

### 发布二进制

| 系统                      | Rust target                 | 文件格式  |
| ------------------------- | --------------------------- | --------- |
| Linux x86-64, glibc       | `x86_64-unknown-linux-gnu`  | `.tar.gz` |
| Linux ARM64, glibc        | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| Linux x86-64, static musl | `x86_64-unknown-linux-musl` | `.tar.gz` |
| macOS Intel               | `x86_64-apple-darwin`       | `.tar.gz` |
| macOS Apple Silicon       | `aarch64-apple-darwin`      | `.tar.gz` |
| Windows x86-64            | `x86_64-pc-windows-msvc`    | `.zip`    |

每个压缩包包含：

```text
spaceprune / spaceprune.exe
README.md
LICENSE
THIRD-PARTY-LICENSES-RUST.txt
THIRD-PARTY-LICENSES-RUST-STDLIB.html
docs/README.md
docs/zh/
docs/en/
```

各平台还必须包含自己的原生运行材料：

- Linux：`spaceprune-readonly-helper`、`spaceprune-cleanup-helper`、`spaceprune-helper-installer`；musl 归档另含实际 Ubuntu 构建环境的 `THIRD-PARTY-LICENSES-MUSL.txt`。
- macOS：完整签名的 `SpacePruneAuthorization.app`，在公证 Accepted 后 staple，再生成最终 tar.gz。
- Windows：签名的 `SpacePruneNative.msi` 与 `package-identity.json`；归档中的 CLI 在最终 MSI 签署并生成 pin 后构建／签署。

工作流不包含用户 `config.toml`、个人 Recipe、凭据、Cargo 构建缓存或私有产品源码。

Release 额外发布 `SHA256SUMS`。Linux ARM64 使用 Ubuntu 22.04 的 GNU 交叉工具链；macOS 两种架构使用对应
官方 runner；Windows 使用 MSVC。Linux glibc 选择 Ubuntu 22.04 构建，以避免无意提高运行时 glibc 下限。

### 发布前准备签署环境

macOS 的两个架构已接入代码签名、公证和临时 keychain 清理。仓库变量为
`SPACEPRUNE_MACOS_TEAM_ID`、`SPACEPRUNE_MACOS_SIGNING_IDENTITY`；Secret 为
`APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_API_KEY_ID`、`APPLE_API_ISSUER`、`APPLE_API_PRIVATE_KEY`。
具体格式、签署顺序与本机入口见 [macOS 原生组件打包](MACOS-PACKAGING.md)。

Windows 已接入 Authenticode：工作流安装 WiX 4.0.6，定位 Windows SDK 的 `signtool`，再由
`windows-native-package.py build --with-cli` 构建固定 MSI 和 CLI。仓库变量
`SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1` 选择 runner 当前账户证书存储中已有的代码签名身份。
当前工作流没有 Windows 证书导入步骤，默认 GitHub-hosted `windows-2025` 镜像不会自带项目私钥；
启用正式发布前，维护者还须准备可用的签署环境／凭据接入，不能只设置一个指纹就期待签署成功。
本机有证书时的独立入口见 [Windows 原生组件打包](WINDOWS-PACKAGING.md)。

缺少身份、验签失败或公证未获 Accepted 都使对应构建失败，不降级为未签名发行物。
`SHA256SUMS` 来自最终归档，用于完整性核对；当前没有另加独立归档签名。某个 tag 是否已经完成签名、
公证和目标系统运行验收，应查看该次实际日志与制品，不能以工作流定义代替。

## Release 重跑

第一次成功执行会创建 GitHub Release，并使用 GitHub 自动生成 release notes。发布步骤先创建草稿，再逐个上传资产，
全部上传成功后才将草稿发布。如果对应 tag 已有 Release，流程明确失败，不使用 `--clobber` 替换同名资产。

发布中途失败时，仅在能够确认本轮创建的 Release 仍为草稿时尝试清理；不会删除已经发布的 Release。
已发布版本需要更正时，使用新的版本和 tag，不通过重跑改变原版本的内容。`publish` job 在 fork 仓库中会跳过。

## 权限和供应链边界

- 默认 workflow 权限只有 `contents: read`；
- 只有最终 publish job 获得 `contents: write`；
- checkout 不保留 Git 凭据；
- GitHub 官方 Actions 固定到完整 commit SHA，旁边注释对应主版本；
- Release 使用 runner 预装的 GitHub CLI，不引入第三方 Release Action；
- actionlint 使用固定 Go module 版本，下载内容由 Go checksum database 校验；
- Cargo 构建统一使用 `--locked`；
- `rust-toolchain.toml` 是 CI、Release 和本地默认 Rust 工具链的唯一声明，缓存键包含该文件哈希；
- cargo-audit 版本固定，RustSec 数据库在运行时更新；
- artifact 已经压缩，因此 Actions 上传时关闭二次压缩。

升级 Action 时，应先查看对应 GitHub 官方仓库的当前主版本和 Node runtime 要求，再把完整 commit SHA 与
注释中的版本一起更新。不要只修改注释。

## 本地发布前检查

先准备工作流同版本的辅助检查工具；这些命令安装检查工具，不会发布 SpacePrune：

```bash
cargo install cargo-about --version 0.9.2 --locked --features cli
cargo install cargo-audit --version 0.22.2 --locked
```

然后执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --workspace --release --locked
cargo audit --deny warnings
scripts/rust-licenses.sh check
scripts/rust-stdlib-licenses.sh check
```

本地检查只能证明本次实际运行的目标。跨平台构建、目标机器运行验证、签名与发布记录应分别保留；
工作流文件存在或本机检查通过，不等于已经有公开可下载的正式版本。
