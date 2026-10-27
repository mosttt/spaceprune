# Windows 原生组件打包

`scripts/windows-native-package.py` 构建独立的 `SpacePruneNative.msi`，包含公开的 metadata/cleanup helper 和许可证材料。工具只生成、签名和验证制品，不安装组件，也不授予读取或删除权限。

当前状态：固定包身份、已安装代次识别、按需自动安装和便携 CLI 发布接线已实现。Windows 原生签名、Installer/UAC、取消/回滚与跨账户安装仍待验证；Linux 交叉检查不代表 Windows 安装验收。

## 构建

从完整 Core Git checkout 构建，需要 Windows、项目固定的 Rust 1.97.0 MSVC 工具链、Python 3、C++ Build Tools／Windows SDK 的 `signtool`，以及兼容 WiX v4 的 `wix build`。工作流使用 WiX 4.0.6；安装全局工具需要适用的 .NET SDK。工具支持 x64 与 ARM64，MSVC helper 使用静态 CRT。

准备工具并选择当前构建账户已有的代码签名身份：

```powershell
rustup target add x86_64-pc-windows-msvc
dotnet tool install --global wix --version 4.0.6
Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
    Select-Object Subject, Thumbprint, NotAfter, HasPrivateKey
$thumbprint = Read-Host 'Paste the Thumbprint from the certificate list above'
$cert = Get-Item "Cert:\CurrentUser\My\$($thumbprint.Trim())" -ErrorAction Stop
if (-not $cert.HasPrivateKey -or $cert.NotAfter -le (Get-Date)) {
    throw 'Select a valid certificate with HasPrivateKey=True'
}
$env:SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1 = $cert.Thumbprint
[Environment]::SetEnvironmentVariable('SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1', $cert.Thumbprint, 'User')
python scripts/windows-native-package.py build --target x86_64-pc-windows-msvc --output target/windows-native-package --with-cli
if ($LASTEXITCODE -ne 0) { throw 'Windows package build failed' }
```

已有 WiX 4.0.6 时无需重复安装。证书应有效、具有可用私钥，且构建机和目标机信任其证书链；指纹只是身份选择器，不是私钥。缺少可信代码签名身份时，普通 `cargo build` 仍能生成开发 CLI，但不能据此完成受保护目录的安装／授权验收。

`--with-cli` 在最终 MSI 签名及 pin 写入后构建并签名 `spaceprune.exe`，并将它与 MSI、pin 一起放入输出目录；仅需组件时省略该参数。CLI 归档必须包含该 EXE 与固定名 MSI，不再使用未固定版本的相邻 helper 作为正式分发路径。按 [构建指南](BUILDING.md#5-手工制作归档)归档这些最终文件，不使用 Cargo 缓存中的中间 EXE。

证书选择器也可通过 `SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1` 提供，证书私钥由 Windows 证书存储管理。工具不接收 PFX 密码。`SPACEPRUNE_WINDOWS_SIGNTOOL` 和 `SPACEPRUNE_WINDOWS_WIX` 可指定工具位置，`--timestamp-url` 指定 RFC3161 时间戳服务。

默认时间戳地址为 `http://timestamp.digicert.com`。Windows SDK SignTool 会拒绝该地址的 HTTPS 形式并报告 `Invalid Timestamp URL`；保留服务支持的地址形式，不省略时间戳或签名验证。

顺序为：构建两个 helper → SHA-256 Authenticode 签名并验证 → 计算签名后载荷身份 → 生成 WiX/MSI → 签名并验证 MSI → 写入最终包身份。`package-identity.json` 绑定最终 MSI 字节、两个签名 EXE、许可证材料及 Core revision。正式签名构建要求已跟踪源码与 HEAD 一致。跨架构构建不执行目标程序，结果中的 `nativeProbesExecuted` 如实标记。

开发可显式选择 `--unsigned`，该产物无法通过正式运行时的签名信任检查。生成失败不会覆盖先前完成的输出；仅允许替换本工具标记的输出目录。

构建前会检查 SignTool 和 WiX。SignTool 优先使用显式 `SPACEPRUNE_WINDOWS_SIGNTOOL`，否则从 PATH、`WindowsSdkDir` 和 Windows Kits 标准目录查找适用于构建机的工具，优先原生架构和较新的 SDK。找不到时安装 Windows SDK 的签名工具后重试；不需要重建证书。显式路径失效会报错，不会悄悄替换为另一份工具。

## 本机免费开发证书

本机测试不需要购买商业证书。这里使用 Windows Authenticode 代码签名，不是 AI Prompt 的 Ed25519 签名。普通 `cargo run -p spaceprune-cli --` 不会生成或携带权限组件；要测试 UAC 和受保护目录，使用下面的签名构建输出，而不是 Cargo 缓存中的普通 EXE。

先准备上文的 Windows SDK、WiX 和 .NET。在隔离的测试机／测试账号中，用 Windows PowerShell 一次性创建本地代码签名证书，再明确把其公钥证书加入本测试账号的信任库。下面的信任操作只适用于你自己刚生成的开发证书，不要导入陌生证书：

```powershell
$cert = New-SelfSignedCertificate -Type CodeSigningCert `
    -Subject 'CN=SpacePrune Local Development' `
    -CertStoreLocation Cert:\CurrentUser\My `
    -KeyAlgorithm RSA -KeyLength 3072 -HashAlgorithm SHA256 `
    -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddMonths(6)
$publicCert = Join-Path $env:TEMP "spaceprune-dev-$($cert.Thumbprint).cer"
Export-Certificate -Cert $cert -FilePath $publicCert | Out-Null
Import-Certificate -FilePath $publicCert -CertStoreLocation Cert:\CurrentUser\Root | Out-Null
Import-Certificate -FilePath $publicCert -CertStoreLocation Cert:\CurrentUser\TrustedPublisher | Out-Null
$env:SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1 = $cert.Thumbprint
[Environment]::SetEnvironmentVariable('SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1', $cert.Thumbprint, 'User')
```

私钥留在当前账号证书库，不进入源码、`.cer` 或安装包。命令同时保存当前用户的构建指纹，新开的终端会读取它。以后更新不要重复创建证书；需要切换身份时再按上文选择已有证书。CLI 与 App 使用同一指纹；App 的正常构建入口会自动打包对应组件。

这是同一测试账号下的本机流程。UAC 若使用另一个管理员账号，或把包搬到另一台机器，该身份也必须明确受信任；不自动修改其他账号或整机信任库。正式公开分发的签名方式另行选择，不能要求普通用户导入开发证书。

选定证书并安装上文的 Windows SDK 后，在 Core 根目录构建并运行完整测试 CLI：

```powershell
python scripts/windows-native-package.py build --target x86_64-pc-windows-msvc --profile debug --output target/windows-native-package --with-cli
if ($LASTEXITCODE -ne 0) { throw 'Windows package build failed' }
.\target\windows-native-package\spaceprune.exe
```

保留输出目录中的 `spaceprune.exe` 与 `SpacePruneNative.msi` 一起运行。需要权限时，程序自动校验、按需安装组件并请求 UAC；不要手工把 helper 复制到系统目录，也不要把整个 App 以管理员身份运行。完整签名包提供受保护清理所需的可信组件；仍被其他进程占用或受系统策略保护的内容可能无法删除，应按实际错误保留，不自动结束进程或修改文件 ACL。

只测试普通可访问文件时，无需设置证书，继续使用普通 Cargo CLI 或 App 的无证书 `--debug --no-bundle` 构建即可。开发包只用于隔离测试。测试结束并卸载对应原生组件后，可以在证书管理器按这个明确指纹撤销 Root／TrustedPublisher 中的开发信任；不要批量删除证书。

## 只生成打包元数据

```bash
python3 scripts/windows-native-package.py prepare \
  --target x86_64-pc-windows-msvc --binaries '<helper-directory>' --output '<metadata-directory>'
```

此操作可跨平台执行，不运行 helper，也不调用 WiX 或 Windows Installer。输出包含 `.wxs` 及布局记录；命令返回构建所需的 `PayloadDir`/`LicenseDir` 定义。helper 必须使用固定文件名。

## 安装位置与信任

固定位置是系统 `ProgramFiles64Folder/SpacePruneNative/native-7-5-<payload-digest>/`。代次摘要包含架构、能力角色、协议、签名 EXE 和许可证材料；不接受外部输入选择安装目录。不同代次可以并存，不覆盖或终止仍在运行的旧代次。MSI 不含自定义动作、服务注册或旧代次删除动作。

Windows 的 metadata v7 / cleanup v5 是登录绑定的引导与 IPC 协议；读取 payload 仍为 v6，已确认清理 grant 仍为 v4，Linux/macOS 的能力探针不变。旧 Windows v6/v4 组件无法通过新探针，在原操作中按既有流程自动准备随包新版，再重新验证，不复用旧授权会话。

许可证目录只保留本仓库 LICENSE、Rust 依赖和标准库 notice。打包前用许可证检查脚本核对生成输入和完整文本；保留原始字节，避免 Windows 行尾转换改变签名包身份。无需另行携带许可 `.sha256` 文件。

`SignedFile` 只表示已验签并持有只读句柄的数据，不构成提升执行许可。可提升的 helper 仍须满足 Program Files、管理员 owner/DACL 和有效签名要求。后续安装调用还必须绑定宿主编译时固定的最终包摘要，不能接受任意有效签名的 MSI。

## 宿主固定包身份

在构建 Windows CLI 或应用宿主时，将 `SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY` 设置为上述 `package-identity.json` 的绝对路径。`spaceprune-access/build.rs` 将有界文件内容复制到编译产物，运行时不会读取该环境变量或邻接清单。非 Windows 构建忽略此输入；不提供此变量的开发构建保留现有受保护相邻 helper 查找策略。

读取和清理入口共享 `windows_native::pinned_helper`，只查固定 Program Files 中的精确代次，并依次核对管理员保护、签名、完整载荷摘要和协议。畸形 pin 或架构不匹配明确拒绝，不降级到其他相邻程序。句柄保持到实际 UAC 会话退出。`pinned_package` 对固定相邻 MSI 同样验证完整签名后摘要，但本身不安装或执行它。

构建顺序必须是 helper → 签名 MSI → 最终 pin → 宿主。helper 与布局工具主动清除继承的宿主 pin，避免把先前包身份编入待计算摘要的载荷而形成循环依赖。公开 Release 工作流使用 `--with-cli` 自动编排；Windows runner 需要预先在证书存储中提供签名身份，仓库变量 `SPACEPRUNE_WINDOWS_CERTIFICATE_SHA1` 选择证书。缺少身份明确失败，不静默发布无法通过运行时信任检查的未签名包。

## 按需安装与取消

普通访问成功时不启动安装。需要管理员能力且精确代次缺失/过期时，`windows_native::ensure_helper` 在原操作中自动准备；后台任务返回 `NeedsAuthorization`，不弹隐藏窗口。并发调用串行化后重查状态，同一操作的安装失败会被记住，避免逐目录重复请求。

普通权限的宿主子进程通过 Windows Installer 安装固定的 MSI，系统按需显示 UAC。继承管道绑定请求/回执，子进程复核父进程创建身份、账户 SID、AuthenticationId LUID、TokenSessionId、登录 SID、原操作期限和编译时包身份。安装属性固定，不能传任意目标目录或安装参数；GUI 不整体提权。

取消或父进程失联会关闭控制管道。Installer 的进度回调请求取消，宿主等待系统调用和工作进程真实退出，不能杀掉 msiexec 后就宣称事务已经回滚。系统 UAC 安全桌面仍由 Windows 控制，应用不能替用户关闭；等待期间不把操作报告为已停止。安装阶段最多使用原操作剩余期限中的 120 秒，取消/回滚收尾不被当成继续业务的许可。

只有安装结束、组件重新通过签名/摘要/ACL/协议检查后才继续原操作。安装批准不等于读取或删除批准；读取与已确认清理仍分别建立原有 UAC 会话。此流程不提供手动升级引导，也不终止正在使用旧代次的清理。

CLI 的 `authorization status/install` 命令目前仅支持 Linux，不是 Windows 的手动准备入口。Windows 使用上述随包 MSI 的按需安装流程。需要卸载测试用原生代次时，先确认没有 CLI 或其他宿主仍在使用它，再通过对应 MSI／Windows“已安装的应用”卸载 `SpacePrune Native Components`；不要手工删除 Program Files 中的代次目录。

## 登录隔离与取消

临时 mailbox、请求文件和清理管道只允许原登录 SID、SYSTEM 和 Administrators 访问。文件 owner 仍是原账户 SID，用于配置和用户目录范围；标准用户在 UAC 输入另一个管理员的凭据，不会把请求改成管理员的用户目录。`OWNER RIGHTS` ACE 去掉同账户其他登录凭 owner 身份隐式重写 DACL 的能力。普通持久配置文件继续使用原账户私有 ACL，不受登录生命周期限制。

helper 持有原宿主进程句柄，并在存活检查中重新查询该进程主 token 的账户、登录、AuthenticationId 与会话 ID。只读连接关闭先移除 session lease，再删除临时目录；即使无关残留文件使目录清理失败，也不能继续原请求。如果 lease 和目录都无法撤销，关闭返回失败，并仍等待工作线程真实结束。Windows UAC 提示尚未结束时，取消或超时只能停止后续业务并等待原生返回，不能提前显示已停止。

## 验证范围

构建工具查找覆盖无 PATH 的 SDK 安装、显式路径失效、构建机架构和工具缺失时保留已有产物；缺失工具的命令行提示不会输出 Python 堆栈。

可移植测试覆盖载荷变化、旧代次保留、固定安装布局、禁止自定义动作、最终 pin 往返、许可证不匹配与签名失败保留旧输出。Windows 专用测试覆盖句柄持有期间的写入/目录替换拒绝；这些测试需在 Windows 原生运行。

实际 WiX 构建、证书链、UAC、Windows Installer 取消与回滚、NTFS/跨账户行为仍需 Windows 验收；当前不把这些项目标记为通过。

Windows 清理管道在取消时返回终止连接错误，避免逐行读取将 `Interrupted` 当作可重试中断而空转。原生回归测试覆盖取消后读取线程退出；这不代表受保护文件删除或完整 UAC 安装流程已经验收。
