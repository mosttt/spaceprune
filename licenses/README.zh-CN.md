# 第三方许可证材料

[English](README.md)

`rust/` 保存 Rust 依赖和标准库的许可清单，根目录的 `about.toml` 与本目录的 `rust/about.hbs` 是依赖清单的生成配置及模板。Core 自有源码的 MIT 许可证仍在根目录 [`LICENSE`](../LICENSE)。

从仓库根目录核对材料：

```sh
scripts/rust-licenses.sh check
scripts/rust-stdlib-licenses.sh check
```

依赖或工具链变化后，以 `generate` 重新生成并审阅差异。发布脚本会把这些材料复制到最终 CLI 归档或 Windows MSI 的既有位置；源码目录分类不改变分发义务。完整打包步骤见[构建指南](../docs/zh/build/BUILDING.md)。
