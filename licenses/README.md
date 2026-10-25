# Third-party license materials

[简体中文](README.zh-CN.md)

`rust/` contains the Rust dependency and standard-library notices. The root `about.toml` and `rust/about.hbs` configure dependency-notice generation. The MIT License for first-party Core source remains at the repository root: [`LICENSE`](../LICENSE).

Check materials from the repository root:

```sh
scripts/rust-licenses.sh check
scripts/rust-stdlib-licenses.sh check
```

After dependency or toolchain changes, run the same commands with `generate` and review the differences. Release scripts copy these materials to their existing locations in CLI archives or Windows MSI packages. The source-directory reorganization does not change distribution obligations. See [Building](../docs/en/build/BUILDING.md) for complete packaging steps.
