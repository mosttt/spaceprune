# Core／CLI 功能与验收范围

实现范围如下，已有证据与剩余平台检查统一见[验证状态](../development/VALIDATION.md)。不能把功能接线或交叉编译当作目标设备验收。

| 能力 | 使用与维护 |
| --- | --- |
| [配置与方案](../usage/RECIPE-GUIDE.md) | 多文件、条件 include、平台、周期规则和一次性选择；重新扫描不沿用旧文件名单。 |
| [文件执行](../reference/ARCHITECTURE.md) | 身份、范围、排除、单次计划消费和部分回执；失败不重放删除。 |
| [快照](../usage/CONFIGURATION.md) | Snapper 配置规则、保护项可见禁选；逻辑大小与卷净变化分别计量。 |
| [Linux 系统资源](../reference/PLUGIN-PROTOCOL.md) | 软件包与 systemd 生命周期由 system-linux 插件维护。 |
| [开发工具](../usage/DOCKER.md) | Go／Cargo／npm 缓存仅 Linux；Docker 为 Linux／Windows／macOS 桌面，缺接口不扫描。 |
| [授权](../reference/NATIVE-AUTHORIZATION.md) | 按需自动准备组件，读取与删除分开，复用有效连接并响应登录撤销。 |
| [CLI](../usage/CLI.md) | 稳定脚本命令、JSON、动态模块、自选、语言和主题。 |
| [插件](../reference/PLUGIN-PROTOCOL.md) | 内置／外部目录、共享启停策略与精确生命周期回执。 |
| [原生文档](MOBILE-DOCUMENTS.md) | 宿主注入原生适配器、可复用资源槽、共享 Engine 与精确文件选择。 |
| [调度](../build/DEPLOYMENT.md) | Linux systemd、Windows Task Scheduler、macOS LaunchAgent；原生验收按系统分别记录。 |
| [发行](../build/BUILDING.md) | 公共仓库独立构建，自有代码 MIT，保留第三方许可与必要 Source Form。 |

macOS 祖先移动的[实现](MACOS-ANCESTRY.md)与[同步依据](MACOS-ANCESTRY-EVIDENCE.md)保留开放问题。Android／iOS 底层库目标不代表已有移动应用。
