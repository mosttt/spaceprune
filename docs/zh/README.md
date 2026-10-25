# Core 与 CLI 文档

[English](../en/README.md) · [项目首页](../../README.zh-CN.md)

从已有二进制开始，先阅读 [CLI 操作](usage/CLI.md)；需要在 Windows、macOS 或 Linux 从源码编译，阅读[构建与打包](build/BUILDING.md)。清理前先扫描，确认范围后再执行。

## 使用与方案

- [CLI 命令、选择界面、语言与主题](usage/CLI.md)
- [编写可复用 Recipe，并进行只读验证](usage/RECIPE-GUIDE.md)
- [配置路径、TOML 字段和各 Handler 选项](usage/CONFIGURATION.md)

## 构建、安装与维护

- [工具链、平台目标和发行归档](build/BUILDING.md)
- [安装、升级、个人配置与自动化](build/DEPLOYMENT.md)
- [本地开发与公共接口](build/DEVELOPMENT.md)
- [Windows 原生组件打包](build/WINDOWS-PACKAGING.md)
- [macOS 签名、公证和授权组件](build/MACOS-PACKAGING.md)
- [GitHub CI 与正式发布](build/CI.md)

## 接口与实现边界

- [架构与数据流](reference/ARCHITECTURE.md)
- [生命周期事件](reference/EVENTS.md)
- [原生权限、授权与复用](reference/NATIVE-AUTHORIZATION.md)
- [外部插件协议](reference/PLUGIN-PROTOCOL.md)
- [Windows 系统路径](reference/WINDOWS-SYSTEM-PATHS.md)
- [功能与验证范围](reference/FEATURE-MATRIX.md)

## 参与开发

[贡献指南](community/CONTRIBUTING.md)与[行为准则](community/CODE_OF_CONDUCT.md)说明协作约定。[当前待办](development/CURRENT-WORK.md)列出未完成工作；[验证状态](development/VALIDATION.md)保留当前验收边界，不再维护逐次修复流水。
