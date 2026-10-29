# 当前状态与待办

Docker 镜像扫描优先使用原生 `UniqueSize` 作为独占空间估算；构建缓存仅在原生明确标记不共享时使用其大小，停止容器使用可写层大小。原生缺失保留未知，已知零保留为零；逻辑引用量与释放估算分开。部分清理回执提供已处理量与未处理量，目录项说明改为文件或子文件夹。17 项 Docker 回归和 3 项回执投影测试在 Windows 通过。

Core／CLI 可独立构建，自有代码采用 MIT。开发入口见[本地开发](../build/DEVELOPMENT.md)，当前验证范围见[验证状态](VALIDATION.md)。

## 已接入的能力

文件系统插件只处理通用文件规则。Snapper 提供配置级快照规则、保护项与快照计量；Linux 系统插件处理软件包和 systemd 生命周期；开发工具插件负责 Go／Cargo／npm 与 Docker。领域发现、计量和清理后果由插件提供，宿主不按目录名称猜测领域。Go／Cargo／npm 缓存当前仅 Linux；Docker 支持 Linux／Windows／macOS 桌面，不在移动端注册。各原生接口先探测，工具缺失不扫描，工具存在但不可用保留诊断。Docker 不删除卷或运行容器，执行绑定端点、Engine 和精确对象 ID，不能把不再可选误当成已删除，见[Docker 使用](../usage/DOCKER.md)。

CLI 的方案、语言配置与本机权限服务可供 Core 宿主复用。普通扫描先用当前用户权限；需要时自动准备匹配的随包组件，成功后继续原操作。读取与删除授权分开，连接仅在有效范围内复用；权限失败不能隐藏原因或循环弹窗。Windows 普通 Cargo EXE 不含完整签名组件；受保护清理按[免费本机签名流程](../build/WINDOWS-PACKAGING.md#本机免费开发证书)构建。缺组件时明确报告清理未启动，不冒充已执行但未确认。

## 仍需目标环境完成的工作

Windows 的签名安装、UAC、跨账号及 NTFS／junction 组合仍待原生验收。macOS 已实现父目录 generation＋持续 vnode 失效观察；APFS／HFS 运行、TCC、签名服务、取消及自动化仍须验证。APFS 在途 rename／属性读取的同步点和计数完整变更域也未解决，见[实现](../reference/MACOS-ANCESTRY.md)与[源码依据](../reference/MACOS-ANCESTRY-EVIDENCE.md)。不能用 Darwin 编译通过代替这一结论。

Android／iOS 目前只有部分底层库目标，不提供已验收的移动应用。
