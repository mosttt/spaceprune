# macOS 原生权限组件打包

普通 `cargo build` 只生成开发程序。可使用管理员文件能力的分发物需要同一 Apple Team 的宿主、授权 App、metadata 服务和 cleanup 服务。构建脚本只准备和签名产物，不安装系统服务；实际前台操作需要权限时，由随包授权 App 使用 ServiceManagement 自动注册或更新固定组件。

macOS 祖先移动防护已接入普通执行与特权 cleanup helper：捕获选中项后、实际删除前检查父目录 generation 与持续保留的失效事件；属性不支持或为零时明确报错，不继续删除。Linux 回归及 Darwin 双架构编译检查已通过，原生 APFS/HFS 验收仍待完成。本阶段在 Mac 上只使用隔离生成的数据进行清理测试，按[实现记录的原生命令](../reference/MACOS-ANCESTRY.md)验证移动、恢复、交换及权限行为；APFS 同步与事件覆盖依据见[祖先移动评估](../reference/MACOS-ANCESTRY-EVIDENCE.md)。

## CLI 分发目录

CLI 可执行文件旁必须保留完整的 `SpacePruneAuthorization.app`，不能只复制 `spaceprune`。授权 App 包含固定的 `Contents/MacOS/spaceprune-helper-installer`、`Contents/Library/LaunchServices/` 下的两个服务以及 `Contents/Library/LaunchDaemons/` 下的声明。旧系统所需 Info/launchd plist 同时嵌入服务 Mach-O 的 `__TEXT` 段。

在有 SDK、Python 3.9+ 和有效 Developer ID Application 身份的 macOS 上，从完整 Core Git checkout 执行。下例使用 Apple Silicon，Intel 则把目标改成 `x86_64-apple-darwin`、最低系统改成 `10.13`：

```bash
export SPACEPRUNE_MACOS_TEAM_ID='YOURTEAMID'
export SPACEPRUNE_MACOS_SIGNING_IDENTITY='Developer ID Application: Your Organization (YOURTEAMID)'
export SPACEPRUNE_MACOS_DISTRIBUTION=cli
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo build --locked --release -p spaceprune-cli --target aarch64-apple-darwin
python3 scripts/macos-native-package.py build --distribution cli \
  --target aarch64-apple-darwin \
  --output target/aarch64-apple-darwin/release/SpacePruneAuthorization.app
python3 scripts/macos-native-package.py sign-cli --distribution cli \
  --output target/aarch64-apple-darwin/release/spaceprune
```

Team ID 必须是实际发布者的 10 位标识。签名构建要求 Core 已跟踪文件与 HEAD 一致。`--unsigned` 仅用于开发产物布局检查，不能取得原生授权。`prepare` 子命令可在其他平台生成 plist；它不需要 Apple 凭据，也不证明 Mach-O 链接或系统安装可用。

CLI 的授权 App ID 为 `com.spaceprune.authorization.cli`，服务为 `com.spaceprune.metadata-v6-cli` 和 `com.spaceprune.cleanup-v4-cli`。独立桌面宿主使用 `desktop` 分发身份；其注册不会替换 CLI 的服务。身份、协议、路径和签名要求统一来自 `spaceprune-access` 的打包描述，不在脚本中重复维护。

## 签名、公证与版本

显式按服务 → 授权 App → 宿主的顺序签名，每个程序使用对应角色的标识及 `runtime,hard,kill,library` 策略。`--deep` 只用于最终验证，不用于递归签名。原生最低版本保持 Intel 10.13、Apple Silicon 11.0。

`CFBundleVersion` 默认由完整 Core 历史提交数量生成。浅克隆必须补齐历史或明确传入单调递增的 `--build-version major.minor.patch`；发行应沿同一版本线推进。代码身份验证仍比较实际 CodeDirectory hash，不把相同版本号当作相同程序。

发布工作流为两个 macOS 架构接入签名、公证和凭据清理。仓库变量为 `SPACEPRUNE_MACOS_TEAM_ID`、`SPACEPRUNE_MACOS_SIGNING_IDENTITY`；Secret 为 `APPLE_CERTIFICATE`（base64 P12）、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_API_KEY_ID`、`APPLE_API_ISSUER`、`APPLE_API_PRIVATE_KEY`（P8 正文）。不要把这些 Secret 写入 Git。CI 创建独立短期 keychain，成功或失败都执行清理；导入失败不输出含密码的命令或原始输出。

CLI 的完整临时 ZIP 提交公证并获得 Accepted 后，给内部授权 App staple，再生成最终 tar.gz。CLI 可执行文件自身不支持 stapling，公证票据由 Gatekeeper 按已提交代码身份查询。凭据缺失、公证拒绝、签名不符都使 macOS 发布构建失败，不降级为未签名发行物。

### 在本机完成归档

前一节完成 Apple Silicon 签名构建后，可沿用工作流的打包顺序。Intel 构建相应替换 `release_target`。
通过现有凭据管理方式给当前构建进程提供 `APPLE_API_KEY_ID`、`APPLE_API_ISSUER`、`APPLE_API_PRIVATE_KEY`，
不要把 P8 内容写入命令历史或仓库。已有本机签名身份无需再运行 CI 专用的 P12 导入步骤。

```bash
set -euo pipefail
release_target=aarch64-apple-darwin
package_id="$(cargo pkgid --locked -p spaceprune-cli)"
package_name="spaceprune-v${package_id##*#}-${release_target}"
package_dir="dist/${package_name}"
test ! -e "$package_dir"
mkdir -p "$package_dir/docs"
cp "target/${release_target}/release/spaceprune" "$package_dir/spaceprune"
ditto "target/${release_target}/release/SpacePruneAuthorization.app" "$package_dir/SpacePruneAuthorization.app"
cp README.md LICENSE \
  licenses/rust/THIRD-PARTY-LICENSES-RUST.txt \
  licenses/rust/THIRD-PARTY-LICENSES-RUST-STDLIB.html \
  "$package_dir/"
cp -R docs/. "$package_dir/docs/"
ditto -c -k --keepParent "$package_dir" "dist/${package_name}.notary.zip"
python3 scripts/macos-ci.py notarize "dist/${package_name}.notary.zip"
xcrun stapler staple "$package_dir/SpacePruneAuthorization.app"
xcrun stapler validate "$package_dir/SpacePruneAuthorization.app"
tar -C dist -czf "dist/${package_name}.tar.gz" "$package_name"
(cd dist && shasum -a 256 "${package_name}.tar.gz" > "${package_name}.tar.gz.sha256")
```

脚本保留 staging 和公证 ZIP 便于检查，不安装组件。使用 `CARGO_TARGET_DIR` 时同步调整两个输入路径。
分发最终 `.tar.gz` 及其校验文件；保持 CLI 与完整授权 App 相邻，不只复制 CLI。

## 按需更新与验收边界

连接前先验证随包组件和实际服务的签名发布者、角色与代码身份。已注册可信文件或实际服务代码过期时，前台操作自动刷新一次，再完整验证新连接；旧系统即使 daemon 无法启动，也能从已安装文件识别需更新状态；13+ 使用 SMAppService 注销/注册，旧系统使用原生授权的 SMJobRemove/SMJobBless。后者由系统提供单元素启动参数，因此服务使用固定文件名的无参数入口。取消、拒绝或系统批准尚未完成时不继续文件操作，也不循环安装。

系统要求的管理员授权和 Login Items 批准仍由 macOS 控制。自动应用新版不等于绕过这些要求；只读权限也不代表已确认删除。更新打断已有工作时，保留原回执和未确认状态，不自动重放清理。

`spaceprune authorization status/install` 目前仅支持 Linux；macOS 没有对应手动命令，也没有用户可调用的完整原生服务卸载入口。删除 CLI／授权 App 文件不等于 ServiceManagement 注销，不能把手工删除文件记录为组件卸载成功。验证注销／卸载前应使用可恢复的测试环境，保留实际系统注册状态和返回结果。

已有验证包括可移植状态机/子进程/打包测试和 Darwin 双架构检查；没有真实 Apple 签名安装证据。签名、公证、旧版本升级、TCC/撤销/取消、跨账户及 APFS 必须在对应系统实测。CLI Terminal 模式现已接入系统 security 文本认证、凭证复核和终端控制权交接（授权代理 v6）；图形交互、可移植 PTY 或交叉检查均不能替代真实 macOS 文本终端验收，见 [实现与验证记录](MACOS-PACKAGING.md)。
