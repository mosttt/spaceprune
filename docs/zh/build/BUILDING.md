# Core 与 CLI 构建、打包和分发

本指南面向开源的 Core 与 CLI。自有代码使用 [MIT License](../../../LICENSE)，只需取得本仓库即可构建；
构建只依赖本仓库与公开依赖。

所有命令从本仓库根目录执行。普通开发、生成本机可执行文件、归档分发和正式发布是不同操作：
`cargo build` 不会自动安装程序、签名或上传 Release。

## 1. 准备构建环境

[rust-toolchain.toml](../../../rust-toolchain.toml) 固定 Rust **1.97.0**，并声明 `rustfmt`、`clippy`。
先安装 rustup，再在仓库根目录执行：

```bash
rustup toolchain install
rustup show active-toolchain
rustc --version
cargo --version
```

构建使用仓库中的 `Cargo.lock`，命令中的 `--locked` 防止自动更新依赖版本。首次构建需要下载 Rust 工具链和
Cargo 依赖；已缓存全部输入后可以使用 `--frozen`，它同时要求锁文件不变且不访问网络。

| 本机构建平台 | 必需的系统工具                                                          | 说明                                                                                                                |
| ------------ | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Linux        | C 编译器、链接器、libc 开发文件                                         | Debian/Ubuntu 通常由 `build-essential` 提供，Arch 通常使用 `base-devel`。CLI 不需要 GTK、WebKit、Node.js 或 Tauri。 |
| Windows x64  | Visual Studio Build Tools 的 C++ 桌面开发工具、Windows SDK、MSVC 链接器 | 使用 MSVC Rust 工具链；必要时从 Developer PowerShell 启动。发行目标是 `x86_64-pc-windows-msvc`。                    |
| macOS        | Xcode Command Line Tools 或完整 Xcode                                   | 可用 `xcode-select --install` 安装命令行工具；用 `xcode-select -p` 检查当前开发工具位置。                           |

Snapper、`pkexec`、systemd 等是对应清理能力的**运行时工具**，不是编译普通 CLI 所需的统一依赖。
是否需要它们由实际 Recipe 和平台决定；普通用户缓存扫描无需先配置 Snapper。

## 2. 本机调试与 release 构建

调试版便于开发，release 版启用优化，适合实际使用和后续分发：

```bash
cargo build --locked -p spaceprune-cli -p spaceprune-readonly-helper
cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper
```

| 构建方式                               | Linux / macOS 产物                   | Windows 产物                             |
| -------------------------------------- | ------------------------------------ | ---------------------------------------- |
| 默认调试版                             | `target/debug/spaceprune`            | `target/debug/spaceprune.exe`            |
| release 版                             | `target/release/spaceprune`          | `target/release/spaceprune.exe`          |
| 指定 `--target <triple>` 的 release 版 | `target/<triple>/release/spaceprune` | `target/<triple>/release/spaceprune.exe` |

设置 `CARGO_TARGET_DIR` 或传入 `--target-dir` 后，以上 `target` 根目录会改变。不要把本机默认构建路径和
显式目标构建路径混用，否则容易运行上一次留下的旧程序。

Linux 源码开发可以直接使用统一启动入口：

```bash
./scripts/run-cli.sh
./scripts/run-cli.sh scan --no-progress
```

它先共同构建 CLI、只读组件、清理组件和安装器，再启动 CLI，参数原样传递。
`cargo run -p spaceprune-cli --` 仍可使用，但 Cargo 只会重建 CLI，不会重建独立的权限程序。
修改原生 Handler 或权限协议后，应先使用上述完整构建命令或统一启动入口，不能仅把旧组件复制到新 CLI 旁边。

Linux / macOS 构建后先验证不删除文件的入口：

```bash
./target/release/spaceprune --version
./target/release/spaceprune --help
./target/release/spaceprune scan --no-progress
```

Windows PowerShell 对应为：

```powershell
.\target\release\spaceprune.exe --version
.\target\release\spaceprune.exe --help
.\target\release\spaceprune.exe scan --no-progress
```

需要交互验证时使用真实终端运行 `spaceprune theme` 或 `spaceprune --select`。主题选择可以按 Esc 取消；
清理选择界面会进入实际清理流程，验收删除动作应使用单独准备的测试目录与 Recipe。
JSON 和重定向行为见 [CLI 使用与配置速查](../usage/CLI.md)。

## 3. 指定平台和架构

当前 [Release 工作流](../../../.github/workflows/release.yml) 定义六种 CLI 归档：

| 目标                        | 工作流构建环境                          | 归档格式  |
| --------------------------- | --------------------------------------- | --------- |
| `x86_64-unknown-linux-gnu`  | Ubuntu 22.04 x64                        | `.tar.gz` |
| `aarch64-unknown-linux-gnu` | Ubuntu 22.04 x64 + GNU ARM64 交叉链接器 | `.tar.gz` |
| `x86_64-unknown-linux-musl` | Ubuntu 22.04 x64 + musl 工具链          | `.tar.gz` |
| `x86_64-apple-darwin`       | macOS Intel runner                      | `.tar.gz` |
| `aarch64-apple-darwin`      | macOS Apple Silicon runner              | `.tar.gz` |
| `x86_64-pc-windows-msvc`    | Windows x64 runner                      | `.zip`    |

先安装目标标准库，再构建。安装 Rust target 只提供 Rust 所需的目标组件，不会自动提供系统 SDK、交叉链接器或模拟器。

### Linux x64 GNU

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper --target x86_64-unknown-linux-gnu
```

发布工作流使用 Ubuntu 22.04 构建 GNU 产物，以控制 glibc 构建基线。用更新发行版在本机生成的同名目标，
不等于自动具备相同运行时兼容性。应在实际目标系统验证；发行前还可用 `file` 和 `ldd` 检查架构及动态库需求。

### Linux ARM64 GNU

ARM64 Linux 本机使用正常的本机 C 工具链。若按工作流在 Debian/Ubuntu x64 上交叉构建，先安装
`gcc-aarch64-linux-gnu`，然后执行：

```bash
rustup target add aarch64-unknown-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper --target aarch64-unknown-linux-gnu
```

交叉构建成功只证明生成了 ARM64 二进制；仍需在 ARM64 Linux 上验证启动、扫描和终端交互。

### Linux x64 musl

按工作流在 Debian/Ubuntu 安装 `musl-tools`，然后执行：

```bash
rustup target add x86_64-unknown-linux-musl
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --locked -p spaceprune-cli -p spaceprune-readonly-helper --target x86_64-unknown-linux-musl
```

该归档用于 musl 静态构建；仍要检查实际产物及目标机器行为。归档中必须包含构建环境提供的 musl notice，
不能只复制 GNU 归档中的文件清单。

### macOS Intel 与 Apple Silicon

完整管理员权限分发物需额外打包同签名的授权 App 和服务，见 [macOS 原生组件打包](MACOS-PACKAGING.md)。普通 Cargo 构建不会准备或安装这些组件。

分别在相应架构的 macOS 上执行其中一组：

```bash
rustup target add x86_64-apple-darwin
cargo build --release --locked -p spaceprune-cli --target x86_64-apple-darwin
```

```bash
rustup target add aarch64-apple-darwin
cargo build --release --locked -p spaceprune-cli --target aarch64-apple-darwin
```

当前工作流分别生成两份归档，不合成 universal binary。CLI 本身仍是命令行程序，完整归档另含
`SpacePruneAuthorization.app`；工作流对 CLI 和授权 App 签名，并在公证后生成最终 `.tar.gz`，不生成 DMG。
上述普通 Cargo 命令不执行这些打包步骤；完整分发按 macOS 专用指南操作。

### Windows x64 MSVC

在已准备 MSVC 构建环境的 PowerShell 中执行：

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --locked -p spaceprune-cli --target x86_64-pc-windows-msvc
.\target\x86_64-pc-windows-msvc\release\spaceprune.exe --version
```

正式 CLI ZIP 包含已内嵌签名 MSI 的 `spaceprune.exe`；运行时只需该 EXE。工作流通过 `windows-native-package.py build --with-cli` 先固定最终 MSI，再嵌入并签名宿主。直接 Cargo 构建仍用于普通开发，不会构建/安装系统组件。Windows runner 必须提供签名证书；详见 [Windows 原生组件打包](WINDOWS-PACKAGING.md)。

需要权限且随包代次尚未安装时，CLI 自动调用系统 Installer 准备，再继续原授权流程。Windows GNU 交叉编译仅作为开发检查，不能代替 MSVC 制品及原生安装/UAC 验收。

### Core 库与移动目标

只需要 Core API 时：

```bash
cargo build --locked -p spaceprune --lib
cargo doc --locked -p spaceprune --no-deps
```

Core 是 Rust library，不会生成另一个可以安装运行的 `core` 程序。其他 Rust 应用通过 `spaceprune::api` 接入，
具体接口见 [开发指南](DEVELOPMENT.md)。

Linux 嵌入宿主启用 Go 模块缓存清理时，推荐随宿主附带同一源码构建的 cleanup helper：

```bash
cargo build --release --locked -p spaceprune-readonly-helper --bin spaceprune-cleanup-helper
```

将 `target/release/spaceprune-cleanup-helper` 放在最终宿主可执行文件同目录，保持可执行且不是 symlink。
如果使用 `--target`，从对应的目标目录取匹配架构产物。Core 不搜索任意 PATH 或构建目录；只随库复制 `.rlib`
不会分发 worker。App 已通过随包 helper 接入，CLI 自带早期入口；其他 Rust 宿主也可按
[开发指南](DEVELOPMENT.md#linux-go-缓存的-worker-分发)接入早期函数，处理普通用户 Go 清理。

普通 worker 直接以当前用户运行，不需要安装到 `/usr/libexec`。需要管理员访问时，完整分发应使用本页的共同构建
命令，同时附带 `spaceprune-readonly-helper`、`spaceprune-cleanup-helper` 和 `spaceprune-helper-installer`。
真实权限不足时，前台授权流程会验证随包能力并应用缺失或过旧的系统副本；后台不弹安装或授权提示。
缺失或旧的随包 worker 应通过更新宿主分发修复，不能依赖提权使用旧 Go 遍历。

CI 还对 Android/iOS 的若干底层库执行 `cargo check`，没有对应的移动 CLI、APK 或 IPA 发行物；检查范围见
[CI 与 Release](CI.md)。

## 4. 安装本地 CLI

从已检出的源码构建并安装到 Cargo 的用户安装目录：

```bash
cargo install --path crates/spaceprune-cli --locked --bin spaceprune
spaceprune --version
```

Linux 若需要访问或清理受限目录，还需将同版本的读取、清理和安装组件安装在 CLI 旁边：

```bash
cargo install --path crates/spaceprune-privileged-helper --locked --bins
```

自定义 `--root` 时 CLI 与三个组件使用同一安装根。确需权限的前台操作会自动应用缺失、过旧或与随包内容不同的系统组件，
完成后继续原操作；编译和 `cargo install` 本身不改系统副本。需要提前准备时，可显式运行
`spaceprune authorization install`；`spaceprune authorization status` 只检查、不请求授权。后台任务不会弹窗安装。
这两个管理命令目前仅支持 Linux。Windows／macOS 的完整权限分发物分别按专用打包指南准备，不能用普通
`cargo install` 代替固定 MSI 或签名授权 App。

`cargo install` 默认使用 release 配置。默认安装位置通常为 `~/.cargo/bin/spaceprune`，Windows 为
`%USERPROFILE%\.cargo\bin\spaceprune.exe`；自定义 `CARGO_HOME` 或 Cargo 安装根配置会改变位置。
确保实际安装目录已进入 `PATH`。

如果需要独立安装根目录，可使用：

```bash
cargo install --path crates/spaceprune-cli --locked --bin spaceprune --root ./local-install
./local-install/bin/spaceprune --version
```

Windows 的相对运行路径为 `.\local-install\bin\spaceprune.exe`。确定要替换已有安装时，再给
`cargo install` 加 `--force`；这只替换安装的程序，不是用户 Recipe 迁移命令。
以上使用本地源码，不表示该 crate 已经发布到 crates.io。

直接分发归档时，用户也可以解压后运行，或把完整运行目录放到自己管理的位置并加入 `PATH`；
同时保留 Linux 的三个相邻组件、Windows 的固定 MSI、macOS 的完整授权 App，不只搬运 CLI 文件。
二进制和个人配置分开保存，升级及自动化注意事项见 [部署指南](DEPLOYMENT.md)。

## 5. 手工制作归档

先完成对应目标的 release 构建。手工归档用于内部交付或验证；正式 GitHub 发布优先使用仓库工作流，
以免版本、目标和文件清单分别维护。归档名称采用 `spaceprune-v<version>-<target>`。

### Linux `.tar.gz`

以下 Bash 示例打包 Linux x64 GNU。其他 Linux 目标可修改 `release_target`，并确认已经构建 CLI 和三个组件。
macOS 的签名授权 App 与公证步骤不同，不使用这段脚本。
示例读取 Cargo package version；如果 staging 目录已存在，会停止，避免把旧文件混进新归档。

```bash
set -e
release_target="x86_64-unknown-linux-gnu"
package_id="$(cargo pkgid --locked -p spaceprune-cli)"
release_version="${package_id##*#}"
package_name="spaceprune-v${release_version}-${release_target}"
package_dir="dist/${package_name}"
test ! -e "$package_dir"
mkdir -p "$package_dir/docs"
cp "target/${release_target}/release/spaceprune" "$package_dir/spaceprune"
chmod 755 "$package_dir/spaceprune"
for helper in spaceprune-readonly-helper spaceprune-cleanup-helper spaceprune-helper-installer; do
  cp "target/${release_target}/release/$helper" "$package_dir/"
done
cp README.md LICENSE \
  licenses/rust/THIRD-PARTY-LICENSES-RUST.txt \
  licenses/rust/THIRD-PARTY-LICENSES-RUST-STDLIB.html \
  "$package_dir/"
cp -R docs/. "$package_dir/docs/"
if [[ "$release_target" == *-musl ]]; then
  cp -L /usr/share/doc/musl/copyright "$package_dir/THIRD-PARTY-LICENSES-MUSL.txt"
fi
tar -C dist -czf "dist/${package_name}.tar.gz" "$package_name"
```

musl notice 路径与 Ubuntu 发布工作流一致；在其他发行版打包时，应找到本次实际使用的 musl 许可证文件，
不能静默省略。手工示例保留 staging 目录，便于检查内容。可用 `tar -tzf dist/<archive>.tar.gz` 核对归档结构。

Linux 生成独立校验文件并立即核对：

```bash
cd dist
sha256sum "${package_name}.tar.gz" > "${package_name}.tar.gz.sha256"
sha256sum --check "${package_name}.tar.gz.sha256"
```

此步进入了 `dist`，继续执行其他仓库命令前先回到仓库根目录。

### macOS `.tar.gz`

按 [macOS 原生组件打包](MACOS-PACKAGING.md)生成同一发布身份的 CLI 和完整授权 App，再按该指南完成
签名、公证及归档。CLI、`SpacePruneAuthorization.app` 和许可证材料必须属于同一构建；只替换 Linux 示例的
目标名会漏掉授权 App。最终归档完成后再生成校验和，不沿用签名／staple 之前的值。

### Windows `.zip`

先按 [Windows 原生组件打包](WINDOWS-PACKAGING.md)的 `--with-cli` 示例，在
`target/windows-native-package` 生成最终签名 CLI、MSI 和 pin，再从仓库根目录执行：

```powershell
$ErrorActionPreference = "Stop"
$releaseTarget = "x86_64-pc-windows-msvc"
$packageId = cargo pkgid --locked -p spaceprune-cli
if ($LASTEXITCODE -ne 0) { throw "Could not read the CLI version" }
$releaseVersion = ($packageId -split '#')[-1]
$packageName = "spaceprune-v$releaseVersion-$releaseTarget"
$packageDir = "dist/$packageName"
if (Test-Path $packageDir) { throw "The staging directory already exists" }
New-Item -ItemType Directory -Force "$packageDir/docs" | Out-Null
Copy-Item target/windows-native-package/spaceprune.exe "$packageDir/"
Copy-Item README.md, LICENSE, licenses/rust/THIRD-PARTY-LICENSES-RUST.txt, licenses/rust/THIRD-PARTY-LICENSES-RUST-STDLIB.html "$packageDir/"
Copy-Item docs/* -Recurse "$packageDir/docs/"
Compress-Archive -Path $packageDir -DestinationPath "dist/$packageName.zip"
Get-FileHash -Algorithm SHA256 "dist/$packageName.zip"
```

比较接收方计算出的 SHA-256 与发布方提供的值。校验和用于核对文件完整性，不能代替发布者签名。
ZIP 中的 CLI 直接运行；相邻 MSI 安装的是独立原生权限组件，不是 CLI 的图形安装器。需要权限时由宿主按需安装该固定 MSI。

## 6. 许可证、签名和正式发布

CLI 归档包含 Core/CLI 自有代码的 MIT 许可证、Cargo 依赖 notice、Rust 标准库 notice；
musl 构建还包含 musl notice。修改依赖或工具链后，要重新核对这些材料。Linux 发布环境中的现有检查入口为：

```bash
scripts/rust-licenses.sh check
scripts/rust-stdlib-licenses.sh check
```

前者要求 `cargo-about 0.9.2`，后者要求当前 Rust sysroot 中存在标准库版权文件。`generate` 会更新仓库材料，
应审阅结果后再提交；安装检查工具和完整 CI 命令见 [CI 与 Release](CI.md)。


当前 Release 工作流已接入 macOS 代码签名／公证和 Windows Authenticode，缺少所需身份时构建失败，
不会自动改成 unsigned。签署环境与仓库变量／Secret 的要求见 [CI 与 Release](CI.md)。
最终归档另提供 `SHA256SUMS`；它用于完整性核对，不是独立制品签名。校验和必须来自所有签名／公证处理后的最终文件。

推送发布 tag 会启动远端构建和发布，不是普通本地验证步骤。tag 校验、六目标矩阵、权限及不可变资产规则，统一见
[CI 与 Release](CI.md)。本指南的命令根据当前源码和工作流核对；未在对应平台运行的构建、签名和安装测试，
不得仅因文档列出了命令就记为通过。
